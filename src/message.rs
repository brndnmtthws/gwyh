use std::ops::Range;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::peer_manager::PeerRequest;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Message {
    pub(crate) id: Uuid,
    pub(crate) body: Body,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    len: usize,
    idx: usize,
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Body {
    Broadcast(Vec<u8>),
    PeerRequest(Box<PeerRequest>),
    Frame(Frame),
    Nak(Range<u32>),
}
