use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

pub type PinnedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + Sync + 'static>>;

pub struct AndThen<T> {
    fut: PinnedFuture<T>,
}

impl<T> AndThen<T> {
    pub fn new<Fut>(fut: Fut) -> Self
    where
        Fut: Future<Output = T> + Send + Sync + 'static,
    {
        Self { fut: Box::pin(fut) }
    }

    pub async fn call(self) {
        self.fut.await;
    }
}

impl<T> Debug for AndThen<T> {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_and_then() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_yes = counter.clone();
        let nuts = AndThen::new(async move { counter.fetch_add(1, Ordering::Relaxed) });

        nuts.call().await;

        assert_eq!(1, counter_yes.load(Ordering::Relaxed));
    }
}
