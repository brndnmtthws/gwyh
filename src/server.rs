use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tracing::info;

use crate::handshake::HandshakerMessage;
use crate::nodeinfo::NodeInfo;
use crate::packet_handler::PacketHandlerMessage;
use crate::peer_manager::PeerManagerRequest;
use crate::registry::Registry;
use crate::GwyhHandler;

pub struct Server {
    socket: Arc<UdpSocket>,
    registry: Registry,
    notify_ready: Option<Arc<Notify>>,
    broadcast_handler: Option<Arc<dyn GwyhHandler + Send + Sync>>,
    notify_shutdown: Arc<Notify>,
}

impl Server {
    pub async fn new(
        nodeinfo: &NodeInfo,
        registry: Registry,
        notify_ready: Arc<Notify>,
        broadcast_handler: Option<Arc<dyn GwyhHandler + Send + Sync>>,
        notify_shutdown: Arc<Notify>,
    ) -> std::io::Result<Self> {
        let nodeinfo = nodeinfo.clone();
        let socket = Arc::new(UdpSocket::bind(nodeinfo.bind_addr()).await?);
        println!("{} bound to {}", nodeinfo.id(), nodeinfo.bind_addr());

        Ok(Self {
            socket,
            registry,
            notify_ready: Some(notify_ready),
            broadcast_handler,
            notify_shutdown,
        })
    }

    pub async fn run(mut self) {
        let nodeinfo = self.registry.nodeinfo().clone();
        info!("starting server on nid={}", nodeinfo.id());

        self.registry
            .cast_peer_manager(PeerManagerRequest::SetNotify(
                self.notify_ready.take().unwrap(),
            ))
            .await
            .expect("unable to deliver message");
        self.registry
            .cast_packet_handler(PacketHandlerMessage::SetBroadcastHandler(
                self.broadcast_handler.take(),
                self.broadcast_handler.take(),
            ))
            .await
            .expect("unable to deliver message");
        self.registry
            .cast_packet_handler(PacketHandlerMessage::Start(self.socket.clone()))
            .await
            .expect("unable to deliver message");
        self.registry
            .cast_handshaker(HandshakerMessage::Bootstrap)
            .await
            .expect("unable to deliver message");
        self.registry
            .cast_peer_manager(PeerManagerRequest::StartHeartbeats)
            .await
            .expect("unable to deliver message");

        tokio::spawn(async move {
            self.notify_shutdown.notified().await;
            self.registry
                .call_peer_manager(PeerManagerRequest::Shutdown)
                .await
                .ok();
            self.notify_shutdown.notify_one();
        });
    }
}
