use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::sync::Arc;

use dryoc::dryocbox::{DryocBox, KeyPair, NewByteArray, Nonce, PublicKey, VecBox};
use futures::future::join_all;
use genserver::GenServer;
use lazy_static::lazy_static;
use lru::LruCache;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};
use tracing::error;
use uuid::Uuid;

use crate::handshake::HandshakerMessage;
use crate::interval::Interval;
use crate::message::{Body, Message};
use crate::packet::{Packet, Payload};
use crate::peer_manager::{PeerManagerRequest, PeerRequest, PeerStatus};
use crate::registry::Registry;
use crate::sequence::Seq32;
use crate::GwyhHandler;

impl Debug for dyn GwyhHandler + Send + Sync {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

lazy_static! {
    static ref PAYLOAD_LENGTH_MAX: usize = {
        // encrypt an empty message
        let sender_keypair = KeyPair::gen();
        let recipient_keypair = KeyPair::gen();
        let nonce = Nonce::gen();
        let message = b"";
        let dryocbox = DryocBox::encrypt_to_vecbox(
            message,
            &nonce,
            &recipient_keypair.public_key,
            &sender_keypair.secret_key,
        )
        .expect("unable to encrypt");

        let packet = Packet {
            s: Seq32::new(),
            id: Uuid::new_v4(),
            p: Payload::B(dryocbox, nonce)
        };
        let packet = rmp_serde::to_vec(&packet).expect("serialize failed");

        // UDP packet limit length:
        // 65,507 bytes (65,535 bytes − 8-byte UDP header − 20-byte IP header)
        65507 - packet.len()
    };
}

// we implement a congestion control algo somewhat similar to the one described
// in https://datatracker.ietf.org/doc/html/draft-gg-udt-01.

// approx. 100,000 packets per second
static RC_DEFAULT_PACKET_INTERVAL_NANOS: u64 = 10_000;
static RC_TIMER_MS: u64 = 60_000;
static RC_LOSS_RATE_THRESHOLD: f64 = 0.001; // 0.1%
static RC_DELAY_DECREASE: u64 = 100;
static RC_DELAY_INCREASE: RangeInclusive<f64> = 2.0..=8.0;

#[derive(Debug)]
pub enum PacketHandlerMessage {
    Start(Arc<UdpSocket>),
    Send(SocketAddr, Payload),
    Recv(SocketAddr, Packet),
    KeysUpdated(HashMap<Uuid, (SocketAddr, KeyPair, PublicKey)>),
    SendMessage(Uuid, Message),
    SubscribersUpdated(Vec<Uuid>),
    SendBroadcast(Vec<u8>),
    BroadcastPeerStatus(PeerStatus),
    SetBroadcastHandler(
        Option<Arc<dyn GwyhHandler + Send + Sync>>,
        Option<Arc<dyn GwyhHandler + Send + Sync>>,
    ),
    RateControl,
}

pub struct PacketHandler {
    registry: Registry,
    socket: Option<Arc<UdpSocket>>,
    tx_sequences: HashMap<SocketAddr, Seq32>,
    rx_sequences: HashMap<SocketAddr, Seq32>,
    last_tx: HashMap<SocketAddr, Instant>,
    nak_count: HashMap<SocketAddr, u32>,
    tx_delay: HashMap<SocketAddr, u64>,
    rc_sequences: HashMap<SocketAddr, Seq32>,
    keys: HashMap<Uuid, (SocketAddr, KeyPair, PublicKey)>,
    subscribers: Vec<Uuid>,
    seen: LruCache<Uuid, ()>,
    broadcast_handler_async: Option<Arc<dyn GwyhHandler + Send + Sync>>,
    broadcast_handler: Option<Arc<dyn GwyhHandler + Send + Sync>>,
    rng: StdRng,
    _rate_control: Interval,
}

impl GenServer for PacketHandler {
    type Message = PacketHandlerMessage;
    type Registry = Registry;
    type Response = ();

    type CallResponse<'a> = impl Future<Output = Self::Response> + 'a;
    type CastResponse<'a> = impl Future<Output = ()> + 'a;

    fn new(registry: Self::Registry) -> Self {
        let rc_registry = registry.clone();
        let rate_control = Interval::new(Duration::from_millis(RC_TIMER_MS), move || {
            let rc_registry = rc_registry.clone();
            async move {
                rc_registry
                    .cast_packet_handler(PacketHandlerMessage::RateControl)
                    .await
                    .ok();
            }
        });
        Self {
            registry,
            socket: None,
            tx_sequences: HashMap::new(),
            rx_sequences: HashMap::new(),
            last_tx: HashMap::new(),
            nak_count: HashMap::new(),
            tx_delay: HashMap::new(),
            rc_sequences: HashMap::new(),
            keys: HashMap::new(),
            subscribers: vec![],
            seen: LruCache::new(10_000),
            broadcast_handler_async: None,
            broadcast_handler: None,
            rng: SeedableRng::from_entropy(),
            _rate_control: rate_control,
        }
    }

    fn handle_call(&mut self, message: Self::Message) -> Self::CallResponse<'_> {
        async {
            self.handle_message(message).await;
        }
    }

    fn handle_cast(&mut self, message: Self::Message) -> Self::CastResponse<'_> {
        async {
            self.handle_message(message).await;
        }
    }
}

impl PacketHandler {
    #[tracing::instrument(skip(self))]
    async fn handle_message(&mut self, message: PacketHandlerMessage) {
        match message {
            PacketHandlerMessage::SetBroadcastHandler(async_handler, handler) => {
                self.broadcast_handler_async = async_handler;
                self.broadcast_handler = handler;
            }
            PacketHandlerMessage::Start(socket) => self.start_recv_loop(socket).await,
            PacketHandlerMessage::Send(sockaddr, payload) => self.send(payload, sockaddr).await,
            PacketHandlerMessage::Recv(sockaddr, packet) => self.recv(packet, sockaddr).await,
            PacketHandlerMessage::SendMessage(target_id, message) => {
                self.encrypt_and_send_message(target_id, message).await;
            }
            PacketHandlerMessage::SendBroadcast(data) => {
                self.send_broadcast(data).await;
            }
            PacketHandlerMessage::BroadcastPeerStatus(peer_status) => {
                self.send_peer_status(peer_status).await;
            }
            PacketHandlerMessage::KeysUpdated(keys) => self.keys = keys,
            PacketHandlerMessage::SubscribersUpdated(subscribers) => {
                self.subscribers = subscribers;
            }
            PacketHandlerMessage::RateControl => self.update_rate_control(),
        }
    }

    fn update_rate_control(&mut self) {
        let current_seqs = self.tx_sequences.clone();

        // calculate loss rate for this interval
        current_seqs
            .iter()
            .map(|(sock, seq)| {
                (
                    sock,
                    **seq - **self.rc_sequences.get(sock).unwrap_or(&Seq32::new()),
                )
            })
            .map(|(sock, count)| {
                (
                    sock,
                    if count > 0 {
                        *self.nak_count.get(sock).unwrap_or(&0) as f64 / count as f64
                    } else {
                        0.0
                    },
                )
            })
            .filter(|(_, loss_rate)| *loss_rate < RC_LOSS_RATE_THRESHOLD)
            .for_each(|(sock, _)| {
                // if the loss rate is below 0.1% for this interval,
                self.tx_delay
                    .entry(*sock)
                    .and_modify(|delay| {
                        *delay = if *delay > RC_DELAY_DECREASE {
                            *delay - RC_DELAY_DECREASE
                        } else {
                            0
                        }
                    })
                    .or_insert(RC_DEFAULT_PACKET_INTERVAL_NANOS);
            });
    }

    #[tracing::instrument(skip(self))]
    async fn start_recv_loop(&mut self, socket: Arc<UdpSocket>) {
        self.socket = Some(socket.clone());
        let registry = self.registry.clone();
        tokio::spawn(async move {
            let mut buf = [0; 65536];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((_len, addr)) => {
                        if let Ok(packet) = rmp_serde::from_slice::<Packet>(&buf) {
                            registry
                                .cast_packet_handler(PacketHandlerMessage::Recv(addr, packet))
                                .await
                                .ok();
                        }
                    }
                    Err(err) => error!("{:?}", err),
                }
            }
        });
    }

    #[tracing::instrument(skip(self))]
    pub async fn recv(&mut self, packet: Packet, from_sockaddr: SocketAddr) {
        let prev_rx_seq = self.rx_sequences.insert(from_sockaddr, packet.s);
        match packet.p {
            Payload::H(handshake) => self
                .registry
                .cast_handshaker(HandshakerMessage::Packet(
                    from_sockaddr,
                    handshake,
                    packet.id,
                ))
                .await
                .expect("send failed"),
            Payload::B(vecbox, nonce) => {
                if let Ok(message) = self.decrypt(packet.id, vecbox, nonce) {
                    if !self.seen.contains(&message.id) {
                        match message.body {
                            Body::Broadcast(data) => {
                                self.recv_broadcast(message.id, data).await;
                                self.seen.put(message.id, ());
                            }
                            Body::PeerRequest(request) => {
                                self.recv_peer_request(message.id, packet.id, request).await;
                                self.seen.put(message.id, ());
                            }
                            Body::Frame(_frame) => (),
                            Body::Nak(seq) => {
                                self.nak_count
                                    .entry(from_sockaddr)
                                    .and_modify(|c| *c += seq.end - seq.start)
                                    .or_insert(seq.end - seq.start);
                                // increase the TX delay
                                self.tx_delay
                                    .entry(from_sockaddr)
                                    .and_modify(|d| {
                                        *d = (*d as f64
                                            * self.rng.gen_range(RC_DELAY_INCREASE.clone()))
                                            as u64
                                    })
                                    .or_insert(RC_DEFAULT_PACKET_INTERVAL_NANOS);
                            }
                        }
                    }
                    // handle NAKs if we skipped any sequences
                    if let Some(prev_rx_seq) = prev_rx_seq {
                        let missed_range = *prev_rx_seq + 1..*packet.s;
                        let body = Body::Nak(missed_range);
                        let message = Message {
                            id: Uuid::new_v4(),
                            body,
                        };
                        self.encrypt_and_send_message(packet.id, message).await;
                    }
                } else {
                    error!(
                        "decrypt failed on {} from {}",
                        self.registry.nodeinfo().id(),
                        packet.id,
                    );
                }
            }
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn send(&mut self, payload: Payload, to_sockaddr: SocketAddr) {
        if let Some(socket) = self.socket.clone() {
            let seq = self
                .tx_sequences
                .entry(to_sockaddr)
                .or_insert_with(Seq32::new);
            seq.inc();

            let packet = Packet {
                s: *seq,
                id: *self.registry.nodeinfo().id(),
                p: payload,
            };

            let prev_tx = self.last_tx.insert(to_sockaddr, Instant::now());
            let tx_delay = self
                .tx_delay
                .get(&to_sockaddr)
                .cloned()
                .unwrap_or(RC_DEFAULT_PACKET_INTERVAL_NANOS);

            let next_window = prev_tx.map(|p| p + Duration::from_nanos(tx_delay));
            tokio::spawn(async move {
                if let Some(next_window) = next_window {
                    tokio::time::sleep_until(next_window).await;
                }
                let packet = rmp_serde::to_vec(&packet).unwrap();
                socket
                    .send_to(&packet, &to_sockaddr)
                    .await
                    .expect("send failed"); // should never fail
            });
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn multisend(&mut self, payloads: Vec<(Payload, SocketAddr)>) {
        if let Some(socket) = &self.socket {
            join_all(payloads.into_iter().map(|(payload, to_sockaddr)| {
                let seq = self
                    .tx_sequences
                    .entry(to_sockaddr)
                    .or_insert_with(Seq32::new);
                seq.inc();

                let packet = Packet {
                    s: *seq,
                    id: *self.registry.nodeinfo().id(),
                    p: payload,
                };
                let packet = rmp_serde::to_vec(&packet).unwrap();
                async move { socket.send_to(&packet, &to_sockaddr).await }
            }))
            .await;
        }
    }

    fn encrypt(
        message: Message,
        keypair: &KeyPair,
        public_key: &PublicKey,
    ) -> Result<(VecBox, Nonce), dryoc::Error> {
        let serialized = rmp_serde::to_vec(&message).unwrap();
        let nonce = Nonce::gen();

        // Encrypt the message into a Vec<u8>-based box.
        DryocBox::encrypt_to_vecbox(&serialized, &nonce, public_key, &keypair.secret_key)
            .map(|enc| (enc, nonce))
    }

    fn decrypt(&self, id: Uuid, vecbox: VecBox, nonce: Nonce) -> Result<Message, ()> {
        if let Some((_, keypair, public_key)) = self.keys.get(&id) {
            if let Ok(serialized) = vecbox.decrypt_to_vec(&nonce, public_key, &keypair.secret_key) {
                if let Ok(message) = rmp_serde::from_slice(&serialized) {
                    return Ok(message);
                }
            } else {
                error!("invalid keys");
            }
        } else {
            error!("keys not found");
        }
        Err(())
    }

    #[tracing::instrument(skip(self))]
    pub async fn encrypt_and_send_message(&mut self, target: Uuid, message: Message) {
        self.encrypt_and_send_messages(vec![target], message).await
    }

    #[tracing::instrument(skip(self))]
    pub async fn encrypt_and_send_messages(&mut self, targets: Vec<Uuid>, message: Message) {
        self.seen.put(message.id, ());
        // need keypairs & PK for targets
        let keys = targets
            .into_iter()
            .flat_map(|target| self.keys.get(&target))
            .cloned();
        // perform encryption off the async thread
        let boxes: Vec<_> = join_all(keys.map(|(sockaddr, keypair, public_key)| {
            let message = message.clone();
            tokio::task::spawn_blocking(move || {
                Self::encrypt(message, &keypair, &public_key)
                    .map(|(vecbox, nonce)| (vecbox, nonce, sockaddr))
            })
        }))
        .await;
        // collect into payloads
        let payloads: Vec<_> = boxes
            .into_iter()
            .flatten()
            .flatten()
            .map(|(vecbox, nonce, sockaddr)| (Payload::B(vecbox, nonce), sockaddr))
            .collect();
        // do a concurrent send
        self.multisend(payloads).await;
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_broadcast(&mut self, data: Vec<u8>) {
        self.send_broadcast_to_subscribers(Message {
            id: Uuid::new_v4(),
            body: Body::Broadcast(data),
        })
        .await;
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_peer_status(&mut self, peer_status: PeerStatus) {
        self.send_broadcast_to_subscribers(Message {
            id: Uuid::new_v4(),
            body: Body::PeerRequest(Box::new(PeerRequest::PeerStatus(peer_status))),
        })
        .await;
    }

    #[tracing::instrument(skip(self))]
    pub async fn recv_broadcast(&mut self, id: Uuid, data: Vec<u8>) {
        if let Some(handler) = &self.broadcast_handler_async {
            handler.handle_broadcast(data.clone()).await.ok();
        }
        self.send_broadcast_to_subscribers(Message {
            id,
            body: Body::Broadcast(data),
        })
        .await;
    }

    #[tracing::instrument(skip(self))]
    pub async fn recv_peer_request(
        &mut self,
        message_id: Uuid,
        from_id: Uuid,
        request: Box<PeerRequest>,
    ) {
        // forward the status we just received to subscribers
        self.send_broadcast_to_subscribers(Message {
            id: message_id,
            body: Body::PeerRequest(request.clone()),
        })
        .await;
        self.registry
            .cast_peer_manager(PeerManagerRequest::Request { from_id, request })
            .await
            .ok();
    }

    #[tracing::instrument(skip(self))]
    pub async fn send_broadcast_to_subscribers(&mut self, message: Message) {
        let sub_ids = self.subscribers.to_vec();
        self.encrypt_and_send_messages(sub_ids, message).await;
    }
}
