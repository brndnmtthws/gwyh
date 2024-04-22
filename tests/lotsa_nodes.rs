use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use gwyh::{Gwyh, GwyhHandler};
use test_log::test;
use tokio::time::sleep;

#[test(tokio::test(flavor = "multi_thread", worker_threads = 10))]
async fn many_nodes() {
    let _ = env_logger::builder().is_test(true).try_init();

    #[derive(Clone)]
    struct MyHandler {
        pub response: Arc<RwLock<String>>,
        pub counter: Arc<AtomicUsize>,
    }

    impl MyHandler {
        fn new() -> Self {
            Self {
                response: Arc::new(RwLock::new(String::new())),
                counter: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl GwyhHandler for MyHandler {
        fn handle_broadcast<'a>(
            &self,
            message: Vec<u8>,
        ) -> Pin<Box<dyn 'a + Send + Future<Output = std::io::Result<()>>>> {
            println!("handle_broadcast {}", self.counter.load(Ordering::Relaxed));
            let response = self.response.clone();
            let counter = self.counter.clone();
            Box::pin(async move {
                let message = String::from_utf8(message).unwrap();
                let mut s = response.write().unwrap();
                s.clone_from(&message);
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
        }
    }

    let handler = MyHandler::new();

    use gwyh::GwyhBuilder;
    let mut g1 = GwyhBuilder::new()
        .with_bind_addr("0.0.0.0:42069")
        .with_peers(vec![])
        .build()
        .unwrap();

    let notify = g1.start().await.unwrap();
    let mut notifies = vec![notify];
    let mut gwyhs = Vec::<Gwyh>::new();

    let num_peers = 10;

    for i in 0..num_peers {
        sleep(Duration::from_millis(1)).await;
        let mut gwyh = GwyhBuilder::new()
            .with_bind_addr(&format!("0.0.0.0:{}", 42070 + i))
            .with_peers(vec!["127.0.0.1:42069".into()])
            .with_handler(handler.clone())
            .build()
            .unwrap();

        let notify = gwyh.start().await.unwrap();
        notifies.push(notify);

        gwyhs.push(gwyh);
    }

    let notified: Vec<_> = notifies.iter().map(|n| n.notified()).collect();
    futures::future::join_all(notified).await;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let s = String::from("why hello there my fren");
    g1.broadcast(s.as_bytes().to_vec()).await.expect("err");

    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(*handler.response.read().unwrap(), s);
    assert_eq!(handler.counter.load(Ordering::Relaxed), num_peers);

    tokio::time::sleep(Duration::from_secs(1)).await;
}
