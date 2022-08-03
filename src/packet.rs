use dryoc::dryocbox::{Nonce, VecBox};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::handshake::Handshake;
use crate::sequence::Seq32;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Payload {
    // handshakes, unencrypted
    H(Handshake),
    // packet body, always encrypted
    B(VecBox, Nonce),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Packet {
    pub(crate) s: Seq32,
    pub(crate) id: Uuid,
    pub(crate) p: Payload,
}
