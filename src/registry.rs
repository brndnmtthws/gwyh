use std::sync::Arc;

use genserver::make_registry;

use crate::handshake::Handshaker;
use crate::nodeinfo::NodeInfo;
use crate::packet_handler::PacketHandler;
use crate::peer_manager::PeerManager;

#[make_registry{
    packet_handler: PacketHandler,
    handshaker: Handshaker,
    peer_manager: PeerManager,
}]
pub struct Registry {
    nodeinfo: Arc<NodeInfo>,
}

impl Registry {
    pub fn nodeinfo(&self) -> &NodeInfo {
        &self.nodeinfo
    }
}
