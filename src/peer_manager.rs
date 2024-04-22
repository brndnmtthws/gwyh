use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use genserver::GenServer;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Duration;
use tracing::error;
use uuid::Uuid;

use crate::and_then::AndThen;
use crate::delayed::Delayed;
use crate::handshake::HandshakerMessage;
use crate::interval::Interval;
use crate::message::{Body, Message};
use crate::packet_handler::PacketHandlerMessage;
use crate::peer::Peer;
use crate::registry::Registry;
use crate::DistributionStrategy;

const REASSIGN_DELAY_HIGH_MILLIS: u64 = 5_000;
const REASSIGN_DELAY_LOW_MILLIS: u64 = 25;
const HEARTBEAT_INTERVAL_MILLIS: u64 = 5_000;
const HEARTBEAT_MISSED_INTERVAL_MILLIS: u64 = 3 * HEARTBEAT_INTERVAL_MILLIS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PeerRequest {
    PeerList,
    Peers(Vec<Peer>),
    PeerStatus(PeerStatus),
    StartSubscription,
    SubscriptionStarted,
    EndSubscription,
    Heartbeat,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum PeerState {
    Up,
    Down,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PeerStatus {
    peer: Peer,
    state: PeerState,
}

impl PeerStatus {
    pub fn new(peer: Peer, state: PeerState) -> Self {
        Self { peer, state }
    }
}

#[derive(Debug)]
pub enum PeerManagerRequest {
    Request {
        from_id: Uuid,
        request: Box<PeerRequest>,
    },
    SetNotify(Arc<Notify>),
    Reassign,
    StartHeartbeats,
    SendHeartbeat,
    Shutdown,
    GetSubscriberCount,
}

#[derive(Debug)]
pub enum PeerManagerResponse {
    SubscriberCount(usize),
    Ok,
}

pub struct PeerManager {
    registry: Registry,
    // all known peers
    known: HashMap<Uuid, Peer>,
    // peers that are subscribed to this node
    subscribers: HashSet<Uuid>,
    // peers that this node is subscribed to
    subscribed_to: HashSet<Uuid>,
    reassign_timer: Option<Delayed>,
    rng: StdRng,
    notify_ready: Option<Arc<Notify>>,
    heartbeat: Option<Interval>,
    heartbeat_watchdogs: HashMap<Uuid, Delayed>,
}

impl GenServer for PeerManager {
    type Message = PeerManagerRequest;
    type Registry = Registry;
    type Response = PeerManagerResponse;

    type CallResponse<'a> = impl Future<Output = Self::Response> + 'a;
    type CastResponse<'a> = impl Future<Output = ()> + 'a;

    fn new(registry: Self::Registry) -> Self {
        Self {
            registry,
            known: HashMap::new(),
            subscribers: HashSet::new(),
            subscribed_to: HashSet::new(),
            reassign_timer: None,
            rng: SeedableRng::from_entropy(),
            notify_ready: None,
            heartbeat: None,
            heartbeat_watchdogs: HashMap::new(),
        }
    }

    fn handle_call(&mut self, message: Self::Message) -> Self::CallResponse<'_> {
        async { self.handle_message(message).await }
    }

    fn handle_cast(&mut self, message: Self::Message) -> Self::CastResponse<'_> {
        async {
            self.handle_message(message).await;
        }
    }
}

impl PeerManager {
    #[tracing::instrument(skip(self))]
    async fn handle_message(&mut self, message: PeerManagerRequest) -> PeerManagerResponse {
        match message {
            PeerManagerRequest::GetSubscriberCount => {
                PeerManagerResponse::SubscriberCount(self.subscribers.len())
            }
            PeerManagerRequest::Reassign => {
                self.reassign().await;
                PeerManagerResponse::Ok
            }
            PeerManagerRequest::SetNotify(notify_ready) => {
                self.notify_ready = Some(notify_ready);
                PeerManagerResponse::Ok
            }
            PeerManagerRequest::Request { from_id, request } => {
                match request.as_ref() {
                    PeerRequest::PeerStatus(peer_status) => {
                        match peer_status.state {
                            PeerState::Up => self.add_peer(peer_status.peer.clone()).await,
                            PeerState::Down => self.remove_peer(peer_status.peer.clone()).await,
                        }
                        // peer status updates need to be forwarded to subscribers,
                        // but only if they originated here
                        if &from_id == self.registry.nodeinfo().id() {
                            self.registry
                                .cast_packet_handler(PacketHandlerMessage::BroadcastPeerStatus(
                                    peer_status.clone(),
                                ))
                                .await
                                .ok();
                        }
                    }
                    PeerRequest::StartSubscription => self.start_subscription(from_id).await,
                    PeerRequest::SubscriptionStarted => {
                        self.subscription_started(from_id).await;
                        // after we have a subscription started, we notify that we're ready
                        if let Some(notify_ready) = &self.notify_ready.take() {
                            notify_ready.notify_one();
                        }
                    }
                    PeerRequest::EndSubscription => self.end_subscription(from_id).await,
                    PeerRequest::Heartbeat => {
                        self.reset_heartbeat_watchdog(&from_id);
                    }
                    PeerRequest::Peers(peers) => self.update_peers(peers).await,
                    PeerRequest::PeerList => self.send_peerlist(from_id).await,
                };
                PeerManagerResponse::Ok
            }
            PeerManagerRequest::StartHeartbeats => {
                self.start_heartbeats().await;
                PeerManagerResponse::Ok
            }
            PeerManagerRequest::SendHeartbeat => {
                self.send_heartbeat().await;
                PeerManagerResponse::Ok
            }
            PeerManagerRequest::Shutdown => {
                self.shutdown().await;
                PeerManagerResponse::Ok
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn send_heartbeat(&self) {
        if self.subscribed_to.is_empty() {
            self.registry
                .cast_handshaker(HandshakerMessage::Bootstrap)
                .await
                .ok();
        }
        for id in self.subscribers.iter() {
            self.registry
                .cast_packet_handler(PacketHandlerMessage::SendMessage(
                    *id,
                    Message {
                        id: Uuid::new_v4(),
                        body: Body::PeerRequest(Box::new(PeerRequest::Heartbeat)),
                    },
                ))
                .await
                .ok();
        }
    }

    #[tracing::instrument(skip(self))]
    async fn send_peerlist(&mut self, to: Uuid) {
        self.registry
            .cast_packet_handler(PacketHandlerMessage::SendMessage(
                to,
                Message {
                    id: Uuid::new_v4(),
                    body: Body::PeerRequest(Box::new(PeerRequest::Peers(
                        self.known.values().cloned().collect(),
                    ))),
                },
            ))
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    async fn update_peers(&mut self, peers: &Vec<Peer>) {
        let added: i32 = peers
            .iter()
            .map(|peer| i32::from(self.known.try_insert(*peer.id(), peer.clone()).is_ok()))
            .sum();

        if added > 0 {
            self.start_reassign_timer();
        }
    }

    #[tracing::instrument(skip(self))]
    async fn start_heartbeats(&mut self) {
        if self.heartbeat.is_none() {
            let registry = self.registry.clone();
            self.heartbeat = Some(Interval::new(
                Duration::from_millis(HEARTBEAT_INTERVAL_MILLIS),
                move || {
                    let registry = registry.clone();
                    async move {
                        registry
                            .cast_peer_manager(PeerManagerRequest::SendHeartbeat)
                            .await
                            .ok();
                    }
                },
            ));
        }
    }

    #[tracing::instrument(skip(self))]
    async fn add_peer(&mut self, peer: Peer) {
        // last write wins for peer keys
        if let Some(existing_peer) = self.known.get(peer.id()) {
            if let (Some(existing_peer_hs_ts), Some(new_peer_hs_ts)) = (
                existing_peer.handshake_timestamp(),
                peer.handshake_timestamp(),
            ) {
                if new_peer_hs_ts > existing_peer_hs_ts {
                    self.known.insert(*peer.id(), peer);
                }
            } else if peer.is_handshaken() {
                self.known.insert(*peer.id(), peer);
            }
        } else {
            self.known.insert(*peer.id(), peer);
        }
        self.update_keys().await;
        self.start_reassign_timer();
    }

    #[tracing::instrument(skip(self))]
    async fn remove_peer(&mut self, peer: Peer) {
        self.update_subscribers().await;
        self.stop_heartbeat_watchdog(peer.id());
        self.subscribed_to.remove(peer.id());
        self.subscribers.remove(peer.id());
        self.known.remove(peer.id());
        self.update_keys().await;
        self.start_reassign_timer();
    }

    fn start_reassign_timer(&mut self) {
        // we only trigger reassignment when we don't have enough subscriptions
        if self.subscribed_to.len() < self.registry.nodeinfo().peer_subscriptions() {
            let millis = self
                .rng
                .gen_range(REASSIGN_DELAY_LOW_MILLIS..REASSIGN_DELAY_HIGH_MILLIS);
            let registry = self.registry.clone();
            self.reassign_timer = Some(Delayed::new(Duration::from_millis(millis), async move {
                registry
                    .cast_peer_manager(PeerManagerRequest::Reassign)
                    .await
                    .expect("cast failed");
            }));
        }
    }

    fn choose_peers(&self) -> HashSet<Uuid> {
        use rand::seq::{IteratorRandom, SliceRandom};
        let (_id_msb, id_lsb) = self.registry.nodeinfo().id().as_u64_pair();
        let mut rng = ChaCha8Rng::seed_from_u64(id_lsb);

        let peers = self
            .known
            .values()
            .filter(|p| p.id() != self.registry.nodeinfo().id());

        match self.registry.nodeinfo().distribution_strategy() {
            DistributionStrategy::Uniform => peers
                .map(|p| p.id())
                .choose_multiple(&mut rng, self.registry.nodeinfo().peer_subscriptions())
                .into_iter()
                .cloned()
                .collect(),
            DistributionStrategy::HubAndSpoke => {
                let (local_peers, remote_peers): (Vec<_>, Vec<_>) =
                    peers.partition(|p| p.zone() == self.registry.nodeinfo().zone());

                let peer_count_remaining = self.registry.nodeinfo().peer_subscriptions();

                // choose 1 peer from each remote peer zone
                let mut remote_peers: Vec<_> = remote_peers
                    .chunk_by(|a, b| a.zone() == b.zone())
                    .flat_map(|zone| {
                        zone.iter()
                            .map(|p| p.id())
                            .choose_multiple(&mut rng, 1)
                            .into_iter()
                    })
                    .cloned()
                    .collect();

                let peer_count_remaining = peer_count_remaining.saturating_sub(remote_peers.len());

                // choose remaining N - 1 peers from local peers
                let mut local_peers: Vec<_> = local_peers
                    .iter()
                    .map(|p| p.id())
                    .choose_multiple(&mut rng, peer_count_remaining)
                    .into_iter()
                    .cloned()
                    .collect();

                local_peers.append(&mut remote_peers);
                HashSet::from_iter(local_peers)
            }
            DistributionStrategy::PreferLocal => peers
                .collect::<Vec<&Peer>>()
                .choose_multiple_weighted(
                    &mut rng,
                    self.registry.nodeinfo().peer_subscriptions(),
                    |p| {
                        if p.zone() == self.registry.nodeinfo().zone() {
                            2
                        } else {
                            1
                        }
                    },
                )
                .expect("weighted choose failed")
                .map(|p| p.id())
                .cloned()
                .collect(),
            DistributionStrategy::PreferRemote => peers
                .collect::<Vec<&Peer>>()
                .choose_multiple_weighted(
                    &mut rng,
                    self.registry.nodeinfo().peer_subscriptions(),
                    |p| {
                        if p.zone() == self.registry.nodeinfo().zone() {
                            1
                        } else {
                            2
                        }
                    },
                )
                .expect("weighted choose failed")
                .map(|p| p.id())
                .cloned()
                .collect(),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn reassign(&mut self) {
        let new_assignments: HashSet<Uuid> = self.choose_peers();

        let added_peers = &new_assignments - &self.subscribed_to;
        let removed_peers = &self.subscribed_to - &new_assignments;

        self.trigger_reassignment(added_peers, removed_peers).await;
    }

    async fn send_start_subscription(registry: &Registry, id: Uuid) {
        registry
            .cast_packet_handler(PacketHandlerMessage::SendMessage(
                id,
                Message {
                    id: Uuid::new_v4(),
                    body: Body::PeerRequest(Box::new(PeerRequest::StartSubscription)),
                },
            ))
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    async fn trigger_reassignment(
        &mut self,
        added_peers: HashSet<Uuid>,
        removed_peers: HashSet<Uuid>,
    ) {
        for id in added_peers.iter() {
            // does the peer already have keys? i.e., did we do a handshake?
            if let Some(peer) = self.known.get(id) {
                if peer.is_handshaken() {
                    // easy peasy
                    Self::send_start_subscription(&self.registry, *id).await;
                } else {
                    // we've not yet handshaken! need to do that, and then we
                    // can start a subscription
                    let registry = self.registry.clone();
                    let id = *id;
                    self.registry
                        .cast_handshaker(HandshakerMessage::SendHello(
                            *peer.sockaddr(),
                            Some(AndThen::new(async move {
                                Self::send_start_subscription(&registry, id).await;
                            })),
                        ))
                        .await
                        .ok();
                }
            }
        }
        for id in removed_peers.iter() {
            self.registry
                .cast_packet_handler(PacketHandlerMessage::SendMessage(
                    *id,
                    Message {
                        id: Uuid::new_v4(),
                        body: Body::PeerRequest(Box::new(PeerRequest::EndSubscription)),
                    },
                ))
                .await
                .ok();
            // cancel heartbeat watchdog for this node
            self.stop_heartbeat_watchdog(id);
            self.subscribed_to.remove(id);
        }
    }

    fn stop_heartbeat_watchdog(&mut self, id: &Uuid) {
        self.heartbeat_watchdogs.remove(id);
    }

    fn reset_heartbeat_watchdog(&mut self, id: &Uuid) {
        if self.subscribed_to.contains(id) {
            if let Some(peer) = self.known.get(id) {
                let registry = self.registry.clone();
                let peer = peer.clone();
                let this_id = *registry.nodeinfo().id();
                let watchdog = Delayed::new(
                    Duration::from_millis(HEARTBEAT_MISSED_INTERVAL_MILLIS),
                    async move {
                        println!("from {this_id} heartbeat watchdog fired for {:?}", peer);
                        registry
                            .cast_peer_manager(PeerManagerRequest::Request {
                                from_id: *registry.nodeinfo().id(),
                                request: Box::new(PeerRequest::PeerStatus(PeerStatus::new(
                                    peer,
                                    PeerState::Down,
                                ))),
                            })
                            .await
                            .ok();
                    },
                );
                self.heartbeat_watchdogs.insert(*id, watchdog);
            } else {
                self.stop_heartbeat_watchdog(id);
            }
        } else {
            self.stop_heartbeat_watchdog(id);
        }
    }

    #[tracing::instrument(skip(self))]
    async fn update_keys(&self) {
        let keys = HashMap::from_iter(self.known.values().filter(|p| p.is_handshaken()).map(|p| {
            (
                *p.id(),
                (
                    *p.sockaddr(),
                    p.our_keypair().unwrap(),
                    p.their_public_key().unwrap(),
                ),
            )
        }));
        self.registry
            .call_packet_handler(PacketHandlerMessage::KeysUpdated(keys))
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    async fn start_subscription(&mut self, id: Uuid) {
        if self.known.contains_key(&id) {
            self.subscribers.insert(id);
        } else {
            error!("unknown peer {id}");
            self.subscribers.remove(&id);
        }
        self.update_subscribers().await;
        // check if we're heartbeating, if not, start
        if self.heartbeat.is_none() {
            self.start_heartbeats().await;
        }
        // acknowledge we've started this subscription
        self.registry
            .cast_packet_handler(PacketHandlerMessage::SendMessage(
                id,
                Message {
                    id: Uuid::new_v4(),
                    body: Body::PeerRequest(Box::new(PeerRequest::SubscriptionStarted)),
                },
            ))
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    async fn end_subscription(&mut self, id: Uuid) {
        self.subscribers.remove(&id);
        self.update_subscribers().await;
    }

    #[tracing::instrument(skip(self))]
    async fn subscription_started(&mut self, id: Uuid) {
        self.subscribed_to.insert(id);
        self.reset_heartbeat_watchdog(&id);
    }

    async fn update_subscribers(&self) {
        let subscribers = self.subscribers.iter().cloned().collect();
        self.registry
            .cast_packet_handler(PacketHandlerMessage::SubscribersUpdated(subscribers))
            .await
            .unwrap();
    }

    #[tracing::instrument(skip(self))]
    async fn shutdown(&mut self) {
        for id in self.subscribed_to.iter() {
            self.registry
                .call_packet_handler(PacketHandlerMessage::SendMessage(
                    *id,
                    Message {
                        id: Uuid::new_v4(),
                        body: Body::PeerRequest(Box::new(PeerRequest::EndSubscription)),
                    },
                ))
                .await
                .ok();
        }
        self.subscribed_to.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose() {
        use rand::seq::SliceRandom;

        let mut rng = ChaCha8Rng::seed_from_u64(69);
        let sample = "Hello, audience!".as_bytes();

        // collect the results into a vector:
        let v: Vec<u8> = sample.choose_multiple(&mut rng, 4).cloned().collect();

        assert_eq!(v, &[101, 99, 33, 32]);
    }
}
