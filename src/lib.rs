pub mod error;
mod server;

use error::Error;
use server::Server;
use tokio::runtime::{Handle, Runtime};

#[derive(Debug)]
pub struct Gwyh {
    runtime: Option<Runtime>,
    handle: Handle,
}

#[derive(Debug)]
pub struct GwyhBuilder {
    handle: Option<Handle>,
}

impl Gwyh {
    pub fn start(&self) -> Result<(), std::io::Error> {
        let handle = self.handle.clone();
        let server = handle.block_on(async { Server::new().await })?;
        handle.spawn(async move {
            server.run();
        });

        Ok(())
    }
}

impl GwyhBuilder {
    pub fn new() -> GwyhBuilder {
        GwyhBuilder { handle: None }
    }
    pub fn with_handle(self, handle: Handle) -> GwyhBuilder {
        GwyhBuilder {
            handle: Some(handle),
            ..self
        }
    }
    pub fn build(self) -> Result<Gwyh, std::io::Error> {
        let g = match self.handle {
            Some(handle) => Gwyh {
                runtime: None,
                handle: handle.clone(),
            },
            None => {
                let runtime = Runtime::new()?;
                let handle = runtime.handle().clone();
                Gwyh {
                    runtime: Some(runtime),
                    handle,
                }
            }
        };

        Ok(g)
    }
}
