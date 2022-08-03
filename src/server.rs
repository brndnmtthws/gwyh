use std::net::SocketAddr;
use tokio::net::UdpSocket;

pub struct Server {
    socket: UdpSocket,
}

impl Server {
    pub async fn new() -> std::io::Result<Server> {
        let socket = UdpSocket::bind("0.0.0.0:8080").await?;
        println!("bound");

        Ok(Self { socket })
    }
    pub async fn run(&self) -> std::io::Result<()> {
        let sock = &self.socket;
        let mut buf = [0; 1024];
        loop {
            let (len, addr) = sock.recv_from(&mut buf).await?;
            println!("{:?} bytes received from {:?}", len, addr);

            let len = sock.send_to(&buf[..len], addr).await?;
            println!("{:?} bytes sent", len);
        }
    }
}
