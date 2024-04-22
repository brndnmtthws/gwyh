#![feature(map_try_insert, type_alias_impl_trait, impl_trait_in_assoc_type)]

mod and_then;
mod delayed;
mod error;
mod handshake;
mod interval;
mod message;
mod nodeinfo;
mod packet;
mod packet_handler;
mod peer;
mod peer_manager;
mod registry;
mod sequence;
mod server;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use error::Error;
use nodeinfo::NodeInfo;
use packet_handler::PacketHandlerMessage;
use peer_manager::{PeerManagerRequest, PeerManagerResponse};
use registry::Registry;
use server::Server;
use tokio::sync::Notify;
use tracing::error;

/// A distribution strategy defines how gwyh will select peers based on which
/// zone they're in. The default value is `DistributionStrategy::Uniform`.
#[derive(Debug, Clone)]
pub enum DistributionStrategy {
    /// Peers are chosen randomly, with no consideration for which zone a peer
    /// is in.
    Uniform,
    /// One peer is chosen from each zone outside this node's zone, and
    /// the remaining peers are chosen randomly from the same zone as this node.
    /// With HubAndSpoke, you can end up with more peers than the target number
    /// of peers if the number of zones is more than the target peer count + 1.
    HubAndSpoke,
    /// Peers from the same zone as this node are twice as likely to be selected
    /// as peers from other zones. It's possible that this strategy could select
    /// _only_ local peers, which may have unintended consequences.
    PreferLocal,
    /// Peers from other zones are twice as likely to be selected as peers from
    /// the same zone as this node. It's possible that this strategy could
    /// select _only_ remote peers, which may have unintended consequences.
    PreferRemote,
}

pub trait GwyhHandler {
    fn handle_broadcast<'a>(
        &self,
        message: Vec<u8>,
    ) -> Pin<Box<dyn 'a + Send + Future<Output = std::io::Result<()>>>>;
}

pub struct Gwyh {
    nodeinfo: Arc<NodeInfo>,
    broadcast_handler: Option<Arc<dyn GwyhHandler + Send + Sync>>,
    registry: Option<Registry>,
    notify_shutdown: Arc<Notify>,
}

impl Gwyh {
    pub async fn start(&mut self) -> std::io::Result<Arc<Notify>> {
        let notify_ready = Arc::new(Notify::new());
        let registry = Registry::start(self.nodeinfo.clone()).await;
        self.registry = Some(registry.clone());
        let server = Server::new(
            &self.nodeinfo,
            registry,
            notify_ready.clone(),
            self.broadcast_handler.take(),
            self.notify_shutdown.clone(),
        )
        .await?;
        server.run().await;

        Ok(notify_ready)
    }

    pub async fn broadcast(&self, message: Vec<u8>) -> Result<(), Error> {
        if let Some(ref registry) = self.registry {
            registry
                .call_packet_handler(PacketHandlerMessage::SendBroadcast(message))
                .await
                .map_err(|e| {
                    error!("broadcast failed with {:?}", e);
                    match e {
                        genserver::Error::MpscSendError(se) => match se.0.0 {
                            PacketHandlerMessage::SendBroadcast(message) => {
                                Error::BroadcastSend(message)
                            }
                            _ => Error::BroadcastFailed,
                        },
                        _ => Error::BroadcastFailed,
                    }
                })
        } else {
            Err(Error::NoRegistry(
                message,
                "there's no registry, this shouldn't happen".into(),
            ))
        }
    }

    /// Initiates a shutdown, and (most importantly) waits for acknowledgement
    /// before continuing. The `Drop` implementation does _not_ wait for
    /// acknowledgement.
    pub async fn shutdown(&mut self) {
        use genserver::Registry;
        // notify server to shut down
        self.notify_shutdown.notify_one();
        // wait for acknowledgement
        self.notify_shutdown.notified().await;
        // shutdown spawned tasks
        self.registry.as_mut().unwrap().shutdown();
    }

    /// Returns a new builder instance.
    pub fn builder() -> GwyhBuilder {
        GwyhBuilder::new()
    }

    /// Returns the number of nodes in the cluster subscribed to this one. You
    /// can use the value returned by this to determine whether or not a node is
    /// receiving broadcasts. A node operating as an island will have zero
    /// subscribers. You can use this value as a way to determine whether this
    /// node is able to accept requests.
    pub async fn get_subscriber_count(&self) -> Result<usize, Error> {
        self.registry
            .as_ref()
            .unwrap()
            .call_peer_manager(PeerManagerRequest::GetSubscriberCount)
            .await
            .map(|r| match r {
                PeerManagerResponse::SubscriberCount(c) => Ok(c),
                _ => Err(Error::UnexpectedResponse),
            })?
    }
}

impl Drop for Gwyh {
    fn drop(&mut self) {
        use genserver::Registry;
        // notify server to shut down
        self.notify_shutdown.notify_one();
        // shutdown spawned tasks
        self.registry.as_mut().unwrap().shutdown();
    }
}

pub struct GwyhBuilder {
    bind_addr: String,
    keys: Vec<String>,
    peers: Vec<String>,
    peer_subscriptions: usize,
    broadcast_handler: Option<Arc<dyn GwyhHandler + Send + Sync>>,
    zone: Option<String>,
    distribution_strategy: DistributionStrategy,
}

impl GwyhBuilder {
    pub fn new() -> Self {
        Self {
            bind_addr: "127.0.0.1:6969".into(),
            keys: vec!["insecure".into()],
            peers: vec![],
            peer_subscriptions: 5,
            broadcast_handler: None,
            zone: None,
            distribution_strategy: DistributionStrategy::Uniform,
        }
    }

    pub fn with_bind_addr(self, bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            ..self
        }
    }

    pub fn with_keys(self, keys: Vec<String>) -> Self {
        Self { keys, ..self }
    }

    pub fn with_peers(self, peers: Vec<String>) -> Self {
        Self { peers, ..self }
    }

    pub fn with_zone(self, zone: impl Into<String>) -> Self {
        Self {
            zone: Some(zone.into()),
            ..self
        }
    }

    pub fn with_distribution_strategy(self, distribution_strategy: DistributionStrategy) -> Self {
        Self {
            distribution_strategy,
            ..self
        }
    }

    pub fn with_handler<BroadcastHandler>(self, broadcast_handler: BroadcastHandler) -> Self
    where
        BroadcastHandler: GwyhHandler + Send + Sync + 'static,
    {
        Self {
            broadcast_handler: Some(Arc::new(broadcast_handler)),
            ..self
        }
    }

    pub fn build(self) -> std::io::Result<Gwyh> {
        let Self {
            bind_addr,
            keys,
            peers,
            peer_subscriptions,
            broadcast_handler,
            zone,
            distribution_strategy,
        } = self;

        let g = Gwyh {
            nodeinfo: Arc::new(NodeInfo::new(
                bind_addr,
                keys,
                peers,
                peer_subscriptions,
                zone,
                distribution_strategy,
            )),
            broadcast_handler,
            registry: None,
            notify_shutdown: Arc::new(Notify::new()),
        };

        Ok(g)
    }
}

impl Default for GwyhBuilder {
    fn default() -> Self {
        Self::new()
    }
}
