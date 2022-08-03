use uuid::Uuid;

use crate::DistributionStrategy;

#[derive(Debug, Clone)]
pub struct NodeInfo {
    bind_addr: String,
    keys: Vec<String>,
    peers: Vec<String>,
    id: Uuid,
    peer_subscriptions: usize,
    zone: Option<String>,
    distribution_strategy: DistributionStrategy,
}

impl NodeInfo {
    pub fn new(
        bind_addr: String,
        keys: Vec<String>,
        peers: Vec<String>,
        peer_subscriptions: usize,
        zone: Option<String>,
        distribution_strategy: DistributionStrategy,
    ) -> Self {
        Self {
            bind_addr,
            keys,
            peers,
            id: Uuid::new_v4(),
            peer_subscriptions,
            zone,
            distribution_strategy,
        }
    }

    pub fn bind_addr(&self) -> &str {
        self.bind_addr.as_ref()
    }

    pub fn keys(&self) -> &[String] {
        self.keys.as_ref()
    }

    pub fn peers(&self) -> &[String] {
        self.peers.as_ref()
    }

    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub fn peer_subscriptions(&self) -> usize {
        self.peer_subscriptions
    }

    pub fn zone(&self) -> &Option<String> {
        &self.zone
    }

    pub fn distribution_strategy(&self) -> &DistributionStrategy {
        &self.distribution_strategy
    }
}
