//! Live gRPC integration test: drive the dynamic client (runtime `.proto` →
//! descriptors → custom codec → tonic unary) against a real echo server. This
//! is the end-to-end verification the unit tests can't give — the custom codec
//! framing a `DynamicMessage` over an actual h2 connection.

use protoglot_core::environment::{Resolver, Scope};
use protoglot_core::format::GrpcRequest;
use protoglot_core::protocols::grpc;

const ECHO_PROTO: &str = r#"
syntax = "proto3";
package echo;
message Ping { string msg = 1; int32 n = 2; }
service Echo { rpc Say(Ping) returns (Ping); }
"#;

#[tokio::test]
async fn dynamic_unary_via_proto_against_live_server() {
    let (addr, _shutdown) = grpc_testserver::start().await;

    let dir = std::env::temp_dir().join(format!("pg-grpc-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("echo.proto"), ECHO_PROTO).unwrap();

    let mut message = serde_json::Map::new();
    message.insert("msg".into(), serde_json::Value::String("live".into()));
    message.insert("n".into(), serde_json::Value::from(5));

    let req = GrpcRequest {
        name: "Say".into(),
        target: format!("http://{addr}"),
        service: "echo.Echo".into(),
        method: "Say".into(),
        schema: None,
        proto: Some("echo.proto".into()),
        message,
        assertions: vec![],
        pre_script: None,
        post_script: None,
    };

    let out = grpc::execute(&req, &Scope::new(), &Resolver::new(), &dir)
        .await
        .expect("grpc execute");
    std::fs::remove_dir_all(&dir).ok();

    assert!(out.protocol_failure.is_none(), "{:?}", out.protocol_failure);
    let body: serde_json::Value = serde_json::from_slice(&out.response.body).unwrap();
    assert_eq!(body["msg"], "live");
    assert_eq!(body["n"], 5);
}

#[tokio::test]
async fn dynamic_unary_via_reflection_against_live_server() {
    let (addr, _shutdown) = grpc_testserver::start().await;

    let mut message = serde_json::Map::new();
    message.insert("msg".into(), serde_json::Value::String("reflected".into()));
    message.insert("n".into(), serde_json::Value::from(9));

    // No proto file — descriptors come from server reflection.
    let req = GrpcRequest {
        name: "Say".into(),
        target: format!("http://{addr}"),
        service: "echo.Echo".into(),
        method: "Say".into(),
        schema: Some("reflection".into()),
        proto: None,
        message,
        assertions: vec![],
        pre_script: None,
        post_script: None,
    };

    let out = grpc::execute(&req, &Scope::new(), &Resolver::new(), std::path::Path::new("."))
        .await
        .expect("grpc execute via reflection");

    assert!(out.protocol_failure.is_none(), "{:?}", out.protocol_failure);
    let body: serde_json::Value = serde_json::from_slice(&out.response.body).unwrap();
    assert_eq!(body["msg"], "reflected");
    assert_eq!(body["n"], 9);
}
