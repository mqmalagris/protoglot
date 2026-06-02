//! A minimal echo gRPC server for protoglot's live integration tests.

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{transport::Server, Request, Response, Status};

pub mod pb {
    tonic::include_proto!("echo");
}

const ECHO_FDS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/echo_descriptor.bin"));

use pb::echo_server::{Echo, EchoServer};
use pb::Ping;

#[derive(Default)]
struct EchoSvc;

#[tonic::async_trait]
impl Echo for EchoSvc {
    async fn say(&self, request: Request<Ping>) -> Result<Response<Ping>, Status> {
        Ok(Response::new(request.into_inner())) // echo it back
    }
}

/// Start the echo server on an ephemeral localhost port. Returns the bound
/// address and a shutdown sender (drop or send to stop).
pub async fn start() -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let incoming = TcpListenerStream::new(listener);
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(ECHO_FDS)
        .build_v1()
        .expect("reflection service");
    tokio::spawn(async move {
        Server::builder()
            .add_service(EchoServer::new(EchoSvc))
            .add_service(reflection)
            .serve_with_incoming_shutdown(incoming, async {
                let _ = rx.await;
            })
            .await
            .ok();
    });
    (addr, tx)
}
