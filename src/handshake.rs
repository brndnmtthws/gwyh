use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dryoc::dryocbox::PublicKey;
use dryoc::generichash::GenericHash;
use dryoc::rng::randombytes_buf;
use dryoc::types::StackByteArray;
use genserver::GenServer;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::error;
use uuid::Uuid;

use crate::and_then::AndThen;
use crate::delayed::Delayed;
use crate::message::{Body, Message};
use crate::packet::Payload;
use crate::packet_handler::PacketHandlerMessage;
use crate::peer::Peer;
use crate::peer_manager::{PeerManagerRequest, PeerRequest, PeerState, PeerStatus};
use crate::registry::Registry;

type Hash = StackByteArray<64>;
type Key = StackByteArray<64>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HandshakeStep {
    Hello {
        pk: PublicKey,
        zone: Option<String>,
        hmacs: Vec<Hmac>,
        timestamp: DateTime<Utc>,
    },
    Ohai {
        pk: PublicKey,
        zone: Option<String>,
        hmacs: Vec<Hmac>,
        challenge: Vec<u8>,
    },
    OkBoss {
        response: Vec<Vec<u8>>,
    },
    GoTeam,
    Dead,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Handshake {
    pub(crate) id: Uuid,
    pub(crate) step: HandshakeStep,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Hmac {
    hmac: Hash,
    message: Vec<u8>,
}

impl Hmac {
    pub fn new(hashed_key: &Hash, timestamp: &DateTime<Utc>, len: usize) -> Self {
        let message = randombytes_buf(len);
        let hmac = Self::calculate_hmac(hashed_key, &message, timestamp);
        Self { hmac, message }
    }

    pub fn is_valid_timestamp_for(timestamp: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
        // timestamp must be within handshake window
        let difference = now.signed_duration_since(*timestamp);
        if let Some(nanos) = difference.num_nanoseconds() {
            nanos.abs() < 1000 * HANDSHAKE_TIMEOUT_MILLIS as i64
        } else {
            false
        }
    }

    pub fn valid_for_key(&self, timestamp: &DateTime<Utc>, hashed_key: &Hash) -> bool {
        let hmac = Self::calculate_hmac(hashed_key, &self.message, timestamp);
        hmac == self.hmac
    }

    fn calculate_hmac(hashed_key: &Hash, message: &Vec<u8>, timestamp: &DateTime<Utc>) -> Hash {
        let mut hasher = GenericHash::new(Some(hashed_key)).unwrap();
        hasher.update(message.as_slice());
        hasher.update(timestamp.timestamp_nanos().to_string().as_bytes());
        hasher.finalize().expect("HMAC failed")
    }
}

struct HandshakeState<State> {
    id: Uuid,
    peer: Peer,
    challenge: Option<Vec<u8>>,
    response: Option<Vec<Vec<u8>>>,
    _timer: Option<Delayed>,
    and_then: Option<AndThen<()>>,
    timestamp: DateTime<Utc>,
    phantom: PhantomData<State>,
}

#[derive(Debug)]
struct Hello;
#[derive(Debug)]
struct Ohai;
#[derive(Debug)]
struct OkBoss;

const HANDSHAKE_TIMEOUT_MILLIS: u64 = 60_000;
const HANDSHAKE_BOOTSTRAP_DELAY_MILLIS_MIN: u64 = 10;
const HANDSHAKE_BOOTSTRAP_DELAY_MILLIS_MAX: u64 = 150;

impl<S> HandshakeState<S> {
    fn start_timer(registry: Registry, id: Uuid, sockaddr: SocketAddr) -> Delayed {
        Delayed::new(
            Duration::from_millis(HANDSHAKE_TIMEOUT_MILLIS),
            async move {
                registry
                    .cast_handshaker(HandshakerMessage::Packet(
                        sockaddr,
                        Handshake {
                            id,
                            step: HandshakeStep::Dead,
                        },
                        Uuid::nil(),
                    ))
                    .await
                    .ok();
            },
        )
    }
}

impl HandshakeState<Hello> {
    #[tracing::instrument(skip(registry))]
    async fn new(registry: Registry, sockaddr: SocketAddr, and_then: Option<AndThen<()>>) -> Self {
        let id = Uuid::new_v4();
        let timestamp = Utc::now();
        Self {
            id,
            peer: Peer::new(None, sockaddr, Uuid::nil(), Some(timestamp), None),
            challenge: None,
            response: None,
            _timer: Some(Self::start_timer(registry, id, sockaddr)),
            and_then,
            timestamp,
            phantom: PhantomData,
        }
    }
}

impl HandshakeState<Ohai> {
    #[tracing::instrument(skip(registry))]
    async fn from(
        registry: Registry,
        sockaddr: SocketAddr,
        node_id: Uuid,
        id: Uuid,
        their_public_key: PublicKey,
        timestamp: DateTime<Utc>,
        zone: Option<String>,
    ) -> Self {
        Self {
            id,
            peer: Peer::new(
                Some(their_public_key),
                sockaddr,
                node_id,
                Some(timestamp),
                zone,
            ),
            challenge: Some(randombytes_buf(69)),
            response: None,
            _timer: Some(Self::start_timer(registry, id, sockaddr)),
            and_then: None,
            timestamp,
            phantom: PhantomData,
        }
    }
}

fn compute_response(
    id: &Uuid,
    public_key: &PublicKey,
    challenge: &Vec<u8>,
    keys: &[String],
) -> Vec<Vec<u8>> {
    keys.iter()
        .map(|key| {
            let mut hasher: GenericHash<64, 64> =
                GenericHash::new::<Key>(None).expect("new failed");
            hasher.update(key.as_bytes());
            hasher.update(challenge);
            hasher.update(public_key);
            hasher.update(id.as_bytes());
            hasher.finalize_to_vec().expect("finalize failed")
        })
        .collect()
}

impl HandshakeState<Hello> {
    fn got_ohai(
        self,
        node_id: Uuid,
        public_key: &PublicKey,
        challenge: &Vec<u8>,
        keys: &[String],
        zone: Option<String>,
    ) -> HandshakeState<OkBoss> {
        let mut peer = self.peer;
        peer.set_their_public_key(public_key.clone());
        peer.set_id(node_id);
        peer.set_zone(zone);
        let response = compute_response(&self.id, public_key, challenge, keys);
        HandshakeState {
            id: self.id,
            peer,
            response: Some(response),
            challenge: self.challenge,
            _timer: None,
            and_then: self.and_then,
            timestamp: self.timestamp,
            phantom: PhantomData,
        }
    }
}

impl HandshakeState<Ohai> {
    fn got_okboss(self, response: &[Vec<u8>], keys: &[String]) -> Result<Self, Self> {
        // check response matches what we expected
        let pk = &self.peer.our_keypair().unwrap().public_key;
        let expected_response =
            compute_response(&self.id, pk, self.challenge.as_ref().unwrap(), keys);

        let expected: HashSet<Vec<u8>> = HashSet::from_iter(expected_response.iter().cloned());
        let received: HashSet<Vec<u8>> = HashSet::from_iter(response.iter().cloned());

        let intersection: HashSet<_> = expected.intersection(&received).collect();

        if intersection.is_empty() {
            Err(self)
        } else {
            Ok(self)
        }
    }
}

pub struct Handshaker {
    registry: Registry,
    hellos: HashMap<(Uuid, SocketAddr), HandshakeState<Hello>>,
    ohais: HashMap<(Uuid, SocketAddr), HandshakeState<Ohai>>,
    okboss: HashMap<(Uuid, SocketAddr), HandshakeState<OkBoss>>,
    inflight: HashSet<SocketAddr>,
    rng: StdRng,
    hashed_keys: Option<Vec<Hash>>,
    bootstrap: Option<Delayed>,
}

#[derive(Debug)]
pub enum HandshakerMessage {
    Bootstrap,
    Packet(SocketAddr, Handshake, Uuid),
    SendHello(SocketAddr, Option<AndThen<()>>),
}

impl GenServer for Handshaker {
    type Message = HandshakerMessage;
    type Registry = Registry;
    type Response = ();

    type CallResponse<'a> = impl Future<Output = Self::Response> + 'a;
    type CastResponse<'a> = impl Future<Output = ()> + 'a;

    fn new(registry: Self::Registry) -> Self {
        Self {
            registry,
            hellos: HashMap::new(),
            ohais: HashMap::new(),
            okboss: HashMap::new(),
            inflight: HashSet::new(),
            rng: SeedableRng::from_entropy(),
            hashed_keys: None,
            bootstrap: None,
        }
    }

    fn handle_call(&mut self, message: Self::Message) -> Self::CallResponse<'_> {
        async { self.handle_message(message).await }
    }

    fn handle_cast(&mut self, message: Self::Message) -> Self::CastResponse<'_> {
        async { self.handle_message(message).await }
    }
}

impl Handshaker {
    async fn handle_message(&mut self, message: HandshakerMessage) {
        match message {
            HandshakerMessage::Bootstrap => self.bootstrap().await,
            HandshakerMessage::Packet(sockaddr, handshake, node_id) => {
                self.handle_handshake(sockaddr, handshake, node_id).await;
            }
            HandshakerMessage::SendHello(sockaddr, and_then) => {
                self.send_hello(sockaddr, and_then).await
            }
        }
    }

    fn has_valid_hmac(&self, timestamp: &DateTime<Utc>, hmacs: &[Hmac]) -> bool {
        let now = Utc::now();
        if let Some(hashed_keys) = &self.hashed_keys {
            Hmac::is_valid_timestamp_for(timestamp, &now)
                && hmacs.iter().any(|hmac| {
                    hashed_keys
                        .iter()
                        .any(|hashed_key| hmac.valid_for_key(timestamp, hashed_key))
                })
        } else {
            false
        }
    }

    fn compute_hmacs(&mut self, timestamp: &DateTime<Utc>) -> Vec<Hmac> {
        if let Some(hashed_keys) = &self.hashed_keys {
            hashed_keys
                .iter()
                .map(|hashed_key| Hmac::new(hashed_key, timestamp, self.rng.gen_range(69..420)))
                .collect()
        } else {
            vec![]
        }
    }

    #[tracing::instrument(skip(self))]
    async fn handle_handshake(
        &mut self,
        sockaddr: SocketAddr,
        handshake: Handshake,
        node_id: Uuid,
    ) {
        match &handshake.step {
            HandshakeStep::Hello {
                pk,
                zone,
                hmacs,
                timestamp,
            } => {
                // ignore messages if we're trying to handshake with ourselves.
                // first, validate the HMACs to see if any are valid. at least
                // one needs to match or else we ignore this hello.
                if &node_id != self.registry.nodeinfo().id()
                    && self.has_valid_hmac(timestamp, hmacs)
                {
                    let handshake = HandshakeState::from(
                        self.registry.clone(),
                        sockaddr,
                        node_id,
                        handshake.id,
                        pk.clone(),
                        *timestamp,
                        zone.clone(),
                    )
                    .await;

                    self.send_ohai(
                        handshake.id,
                        sockaddr,
                        handshake.peer.our_keypair().unwrap().public_key.clone(),
                        handshake.challenge.as_ref().unwrap().clone(),
                        &handshake.timestamp,
                        self.inflight.contains(&sockaddr),
                    )
                    .await;
                    self.inflight.insert(sockaddr);
                    self.ohais.insert((handshake.id, sockaddr), handshake);
                }
            }
            HandshakeStep::Ohai {
                pk,
                zone,
                hmacs,
                challenge,
            } => {
                if let Some(handshake) = self.hellos.remove(&(handshake.id, sockaddr)) {
                    if self.has_valid_hmac(&handshake.timestamp, hmacs) {
                        let handshake = handshake.got_ohai(
                            node_id,
                            pk,
                            challenge,
                            self.registry.nodeinfo().keys(),
                            zone.clone(),
                        );
                        self.send_okboss(
                            handshake.id,
                            sockaddr,
                            handshake.response.as_ref().unwrap().clone(),
                        )
                        .await;

                        assert!(handshake.peer.is_handshaken());
                        self.okboss.insert((handshake.id, sockaddr), handshake);
                    }
                }
            }
            HandshakeStep::OkBoss { response } => {
                if let Some(handshake) = self.ohais.remove(&(handshake.id, sockaddr)) {
                    if let Ok(handshake) =
                        handshake.got_okboss(response, self.registry.nodeinfo().keys())
                    {
                        self.send_peer_up(handshake.peer).await;
                        self.send_goteam(handshake.id, sockaddr).await;
                        if let Some(and_then) = handshake.and_then {
                            and_then.call().await;
                        }
                    } else {
                        error!("bad okboss from {sockaddr}");
                    }
                }
            }
            HandshakeStep::GoTeam => {
                if let Some(handshake) = self.okboss.remove(&(handshake.id, sockaddr)) {
                    self.send_peer_up(handshake.peer.clone()).await;
                    if let Some(and_then) = handshake.and_then {
                        and_then.call().await;
                    }
                }
            }
            HandshakeStep::Dead => {
                self.hellos.remove(&(handshake.id, sockaddr));
                self.ohais.remove(&(handshake.id, sockaddr));
                self.okboss.remove(&(handshake.id, sockaddr));
                self.inflight.remove(&sockaddr);
            }
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_hello(&mut self, sockaddr: SocketAddr, and_then: Option<AndThen<()>>) {
        let handshake = HandshakeState::new(self.registry.clone(), sockaddr, and_then).await;
        let payload = Payload::H(Handshake {
            id: handshake.id,
            step: HandshakeStep::Hello {
                pk: handshake.peer.our_keypair().unwrap().public_key.clone(),
                zone: self.registry.nodeinfo().zone().clone(),
                hmacs: self.compute_hmacs(&handshake.timestamp),
                timestamp: handshake.timestamp,
            },
        });
        self.inflight.insert(sockaddr);
        self.hellos.insert((handshake.id, sockaddr), handshake);
        self.registry
            .cast_packet_handler(PacketHandlerMessage::Send(sockaddr, payload))
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_ohai(
        &mut self,
        id: Uuid,
        sockaddr: SocketAddr,
        public_key: PublicKey,
        challenge: Vec<u8>,
        timestamp: &DateTime<Utc>,
        inflight: bool,
    ) {
        let payload = Payload::H(Handshake {
            id,
            step: HandshakeStep::Ohai {
                pk: public_key,
                zone: self.registry.nodeinfo().zone().clone(),
                hmacs: self.compute_hmacs(timestamp),
                challenge,
            },
        });

        let registry = self.registry.clone();
        let millis = if inflight {
            self.rng.gen_range(
                HANDSHAKE_BOOTSTRAP_DELAY_MILLIS_MIN..HANDSHAKE_BOOTSTRAP_DELAY_MILLIS_MAX,
            )
        } else {
            0
        };
        tokio::spawn(async move {
            sleep(Duration::from_millis(millis)).await;
            registry
                .call_packet_handler(PacketHandlerMessage::Send(sockaddr, payload))
                .await
                .ok();
        });
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_okboss(&mut self, id: Uuid, sockaddr: SocketAddr, response: Vec<Vec<u8>>) {
        let payload = Payload::H(Handshake {
            id,
            step: HandshakeStep::OkBoss { response },
        });
        self.registry
            .call_packet_handler(PacketHandlerMessage::Send(sockaddr, payload))
            .await
            .unwrap();
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_goteam(&mut self, id: Uuid, sockaddr: SocketAddr) {
        let payload = Payload::H(Handshake {
            id,
            step: HandshakeStep::GoTeam,
        });
        self.registry
            .call_packet_handler(PacketHandlerMessage::Send(sockaddr, payload))
            .await
            .unwrap();
    }

    #[tracing::instrument(skip(self))]
    pub async fn request_peerlist(&mut self, id: Uuid, sockaddr: SocketAddr) {
        self.registry
            .call_packet_handler(PacketHandlerMessage::SendMessage(
                id,
                Message {
                    id: Uuid::new_v4(),
                    body: Body::PeerRequest(Box::new(PeerRequest::PeerList)),
                },
            ))
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_peer_up(&self, peer: Peer) {
        self.registry
            .call_peer_manager(PeerManagerRequest::Request {
                from_id: *self.registry.nodeinfo().id(),
                request: Box::new(PeerRequest::PeerStatus(PeerStatus::new(
                    peer,
                    PeerState::Up,
                ))),
            })
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    pub async fn bootstrap(&mut self) {
        if self.hashed_keys.is_none() {
            // hash pre-shared keys for handshaking
            self.hashed_keys = Some(
                self.registry
                    .nodeinfo()
                    .keys()
                    .iter()
                    .flat_map(|key| GenericHash::hash::<_, Key, _>(key.as_bytes(), None))
                    .collect(),
            );
        }

        let peers: Vec<_> = self.registry.nodeinfo().peers().to_vec();
        let registry = self.registry.clone();
        let millis = self
            .rng
            .gen_range(HANDSHAKE_BOOTSTRAP_DELAY_MILLIS_MIN..HANDSHAKE_BOOTSTRAP_DELAY_MILLIS_MAX);
        self.bootstrap = Some(Delayed::new(Duration::from_millis(millis), async move {
            use std::net::ToSocketAddrs;
            for peer in peers.iter() {
                match peer.to_socket_addrs() {
                    Ok(mut sockaddr_iter) => {
                        if let Some(sockaddr) = sockaddr_iter.next() {
                            registry
                                .call_handshaker(HandshakerMessage::SendHello(sockaddr, None))
                                .await
                                .ok();
                        }
                    }
                    Err(err) => error!("failed to resolve {peer}: {:?}", err),
                }
            }
        }));
    }
}
