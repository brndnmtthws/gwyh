use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use dryoc::dryocbox::{KeyPair, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Peer {
    #[serde(skip)]
    our_keypair: Option<KeyPair>,
    #[serde(skip)]
    their_public_key: Option<PublicKey>,
    #[serde(skip)]
    handshake_timestamp: Option<DateTime<Utc>>,
    sockaddr: SocketAddr,
    id: Uuid,
    zone: Option<String>,
}

impl Peer {
    pub fn new(
        their_public_key: Option<PublicKey>,
        sockaddr: SocketAddr,
        id: Uuid,
        handshake_timestamp: Option<DateTime<Utc>>,
        zone: Option<String>,
    ) -> Self {
        Self {
            our_keypair: Some(KeyPair::gen()),
            their_public_key,
            sockaddr,
            id,
            handshake_timestamp,
            zone,
        }
    }

    pub fn is_handshaken(&self) -> bool {
        self.our_keypair.is_some() && self.their_public_key.is_some()
    }

    pub fn our_keypair(&self) -> Option<KeyPair> {
        self.our_keypair.clone()
    }

    pub fn their_public_key(&self) -> Option<PublicKey> {
        self.their_public_key.clone()
    }

    pub fn set_their_public_key(&mut self, public_key: PublicKey) {
        self.their_public_key = Some(public_key);
    }

    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub fn set_id(&mut self, id: Uuid) {
        self.id = id;
    }

    pub fn sockaddr(&self) -> &SocketAddr {
        &self.sockaddr
    }

    pub fn handshake_timestamp(&self) -> &Option<DateTime<Utc>> {
        &self.handshake_timestamp
    }

    pub fn set_zone(&mut self, zone: Option<String>) {
        self.zone = zone;
    }

    pub fn zone(&self) -> &Option<String> {
        &self.zone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_skip() {
        // check that the fields we want to skip (keys) are never serialized
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        let sockaddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);

        let their_keypair = KeyPair::gen();

        let peer = Peer::new(
            Some(their_keypair.public_key.clone()),
            sockaddr,
            Uuid::new_v4(),
            Some(Utc::now()),
            None,
        );
        let serialized = rmp_serde::to_vec(&peer).unwrap();
        let deserialized: Peer = rmp_serde::from_slice(&serialized).unwrap();

        assert_eq!(peer.id, deserialized.id);
        assert_eq!(peer.sockaddr, deserialized.sockaddr);
        assert_eq!(None, deserialized.their_public_key);
        assert_eq!(None, deserialized.our_keypair);
    }
}
