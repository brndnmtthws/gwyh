use std::fmt::Debug;

use tracing::error;

#[derive(Debug)]
pub enum Error {
    GenServerErr,
    NoRegistry(Vec<u8>, String),
    BroadcastFailed,
    BroadcastSend(Vec<u8>),
    UnexpectedResponse,
    Timeout,
}

impl<M, R> From<genserver::Error<M, R>> for Error
where
    M: Debug,
    R: Debug,
{
    fn from(err: genserver::Error<M, R>) -> Self {
        error!("Error from genserver: {err:?}");
        Self::GenServerErr
    }
}
