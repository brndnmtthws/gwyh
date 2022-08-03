use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

pub struct Interval {
    joinhandle: JoinHandle<()>,
}

impl Drop for Interval {
    fn drop(&mut self) {
        self.joinhandle.abort();
    }
}

impl Interval {
    pub fn new<F, Fut>(duration: Duration, f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future + Send + Sync + 'static,
    {
        let joinhandle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(duration);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                f().await;
            }
        });
        Self { joinhandle }
    }
}
