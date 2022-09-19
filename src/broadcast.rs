use std::future::Future;

use genserver::GenServer;

use crate::registry::Registry;

pub struct Broadcast {
    registry: Registry,
}

impl Broadcast {
    async fn handle_message(&self, message: Vec<u8>) {}
}

impl GenServer for Broadcast {
    type Message = Vec<u8>;
    type Registry = Registry;
    type Response = ();

    type CallResponse<'a> = impl Future<Output = Self::Response> + 'a;
    type CastResponse<'a> = impl Future<Output = ()> + 'a;

    fn new(registry: Self::Registry) -> Self {
        Self { registry }
    }

    fn handle_call(&mut self, message: Self::Message) -> Self::CallResponse<'_> {
        self.handle_message(message)
    }

    fn handle_cast(&mut self, message: Self::Message) -> Self::CastResponse<'_> {
        self.handle_message(message)
    }
}
