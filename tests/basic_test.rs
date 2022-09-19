use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use gwyh::sequence::Seq32;
use gwyh::GwyhHandler;
use test_log::test;

#[test(tokio::test(flavor = "multi_thread", worker_threads = 4))]
async fn build_and_start() {
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
        .with_peers(vec!["127.0.0.1:42169".into()])
        .with_handler(handler.clone())
        .build()
        .unwrap();

    let mut g2 = GwyhBuilder::new()
        .with_bind_addr("0.0.0.0:42169")
        .with_peers(vec!["127.0.0.1:42069".into()])
        .build()
        .unwrap();

    let notify1 = g1.start().await.unwrap();
    let notify2 = g2.start().await.unwrap();

    let mut c = Seq32::new();
    println!("{}", c.inc());

    notify1.notified().await;
    println!("{}", c.inc());
    notify2.notified().await;

    println!("{}", c.inc());

    let s = String::from("why hello there my fren");
    g2.broadcast(s.as_bytes().to_vec()).await.expect("err");

    println!("{}", c.inc());

    tokio::time::sleep(Duration::from_secs(1)).await;

    assert_eq!(*handler.response.read().unwrap(), s);
    assert_eq!(handler.counter.load(Ordering::Relaxed), 1);

    println!("{}", c.inc());
    tokio::time::sleep(Duration::from_secs(1)).await;

    println!("{}", c.inc());
    g2.shutdown().await;

    tokio::time::sleep(Duration::from_secs(1)).await;
}
