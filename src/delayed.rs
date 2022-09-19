use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;

pub struct Delayed {
    joinhandle: JoinHandle<()>,
}

impl Drop for Delayed {
    fn drop(&mut self) {
        self.joinhandle.abort();
    }
}

impl Delayed {
    pub fn new<Fut>(duration: Duration, future: Fut) -> Self
    where
        Fut: Future + Send + 'static,
    {
        let joinhandle = tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            future.await;
        });
        Self { joinhandle }
    }
}
