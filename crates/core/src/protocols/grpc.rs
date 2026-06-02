//! gRPC — dynamic unary invocation (Phase 6, "the boss"). No compile-time
//! codegen: a `.proto` is compiled at runtime with `protox`, descriptors are
//! built with `prost-reflect`, and a **custom tonic `Codec`** ferries
//! `DynamicMessage`s over the wire (§7). The JSON `message` becomes the request
//! via prost-reflect's serde support; the reply is serialized back to JSON so
//! the normal jsonpath assertions apply.
//!
//! This increment implements the **`.proto` path** for **unary** calls over
//! plaintext h2 (`http://`). Server reflection, streaming, and TLS are deferred.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protoglot_format::GrpcRequest;
use serde_json::Value;
use std::path::Path;
use tonic::client::Grpc;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::transport::Channel;
use tonic::{Request, Status};

pub async fn execute(
    req: &GrpcRequest,
    scope: &Scope,
    resolver: &Resolver,
    base_dir: &Path,
) -> Result<ExecOutcome> {
    // Resolve the schema source. Only the .proto path is implemented; server
    // reflection is the next increment.
    let proto = match (req.proto.as_deref(), req.schema.as_deref()) {
        (Some(p), _) => p.to_string(),
        (None, Some("reflection")) | (None, None) => {
            return Err(Error::NotImplemented(
                "grpc server reflection (set `proto = \"path.proto\"` for now)",
            ))
        }
        (None, Some(path)) => path.to_string(),
    };

    let proto_path = base_dir.join(&proto);
    let include = proto_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let fds = protox::compile([&proto_path], [&include])
        .map_err(|e| Error::Request(format!("compiling {}: {e}", proto_path.display())))?;
    let pool = DescriptorPool::from_file_descriptor_set(fds)
        .map_err(|e| Error::Request(format!("building descriptors: {e}")))?;

    let service_name = resolver.resolve(&req.service, scope).await?;
    let method_name = resolver.resolve(&req.method, scope).await?;
    let service = pool
        .get_service_by_name(&service_name)
        .ok_or_else(|| Error::Request(format!("service `{service_name}` not found in proto")))?;
    let method = service
        .methods()
        .find(|m| m.name() == method_name)
        .ok_or_else(|| {
            Error::Request(format!("method `{method_name}` not found on `{service_name}`"))
        })?;
    if method.is_client_streaming() || method.is_server_streaming() {
        return Err(Error::NotImplemented("grpc streaming (unary only for now)"));
    }
    let input = method.input();
    let output = method.output();

    // Build the request message from the JSON map, resolving {{...}} in string
    // values (shallow, like GraphQL variables).
    let mut obj = serde_json::Map::with_capacity(req.message.len());
    for (k, v) in &req.message {
        let resolved = match v {
            Value::String(s) => Value::String(resolver.resolve(s, scope).await?),
            other => other.clone(),
        };
        obj.insert(k.clone(), resolved);
    }
    let msg_json = Value::Object(obj);
    let request_msg = DynamicMessage::deserialize(input.clone(), &msg_json)
        .map_err(|e| Error::Request(format!("building request message: {e}")))?;

    // Connect (plaintext h2) and invoke.
    let target = resolver.resolve(&req.target, scope).await?;
    let endpoint = if target.starts_with("http://") || target.starts_with("https://") {
        target
    } else {
        format!("http://{target}")
    };
    let channel = Channel::from_shared(endpoint.clone())
        .map_err(|e| Error::Request(format!("invalid grpc target `{endpoint}`: {e}")))?
        .connect()
        .await
        .map_err(|e| Error::Request(format!("grpc connect to `{endpoint}`: {e}")))?;

    let mut client = Grpc::new(channel);
    client
        .ready()
        .await
        .map_err(|e| Error::Request(format!("grpc service not ready: {e}")))?;

    let path: http::uri::PathAndQuery = format!("/{}/{}", service.full_name(), method.name())
        .parse()
        .map_err(|e| Error::Request(format!("bad grpc path: {e}")))?;
    let codec = DynamicCodec {
        output: output.clone(),
    };

    match client.unary(Request::new(request_msg), path, codec).await {
        Ok(response) => {
            let reply = response.into_inner();
            let json = serde_json::to_value(&reply).unwrap_or(Value::Null);
            let body = serde_json::to_vec(&json).unwrap_or_default();
            Ok(ExecOutcome::ok(RawResponse {
                status: 200,
                headers: Vec::new(),
                body,
                content_type: Some("application/json".to_string()),
            }))
        }
        Err(status) => Ok(ExecOutcome {
            response: RawResponse {
                status: 0,
                headers: Vec::new(),
                body: Vec::new(),
                content_type: None,
            },
            protocol_failure: Some(format!("gRPC {:?}: {}", status.code(), status.message())),
        }),
    }
}

/// A tonic codec that speaks `DynamicMessage` in both directions. The decoder
/// carries the response `MessageDescriptor` so it knows how to parse bytes.
#[derive(Clone)]
struct DynamicCodec {
    output: MessageDescriptor,
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynEncoder;
    type Decoder = DynDecoder;

    fn encoder(&mut self) -> DynEncoder {
        DynEncoder
    }
    fn decoder(&mut self) -> DynDecoder {
        DynDecoder {
            output: self.output.clone(),
        }
    }
}

struct DynEncoder;

impl Encoder for DynEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(
        &mut self,
        item: DynamicMessage,
        dst: &mut EncodeBuf<'_>,
    ) -> std::result::Result<(), Status> {
        item.encode(dst)
            .map_err(|e| Status::internal(format!("grpc encode: {e}")))
    }
}

struct DynDecoder {
    output: MessageDescriptor,
}

impl Decoder for DynDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(
        &mut self,
        src: &mut DecodeBuf<'_>,
    ) -> std::result::Result<Option<DynamicMessage>, Status> {
        let msg = DynamicMessage::decode(self.output.clone(), src)
            .map_err(|e| Status::internal(format!("grpc decode: {e}")))?;
        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ECHO_PROTO: &str = r#"
        syntax = "proto3";
        package echo;
        message Ping { string msg = 1; int32 n = 2; }
        service Echo { rpc Say(Ping) returns (Ping); }
    "#;

    /// Exercises the hard machinery without a server: runtime .proto compile →
    /// descriptor resolution → JSON → DynamicMessage → wire bytes → back to
    /// JSON. (The same encode/decode the DynamicCodec runs, minus tonic's
    /// buffer wrappers.)
    #[test]
    fn proto_compile_descriptor_and_dynamic_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pg-grpc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proto = dir.join("echo.proto");
        std::fs::write(&proto, ECHO_PROTO).unwrap();

        let fds = protox::compile([&proto], [&dir]).unwrap();
        let pool = DescriptorPool::from_file_descriptor_set(fds).unwrap();

        let service = pool.get_service_by_name("echo.Echo").expect("service");
        assert_eq!(service.full_name(), "echo.Echo");
        let method = service.methods().find(|m| m.name() == "Say").expect("method");
        assert!(!method.is_client_streaming() && !method.is_server_streaming());

        let input = method.input();
        let json = serde_json::json!({ "msg": "hi", "n": 7 });
        let msg = DynamicMessage::deserialize(input.clone(), &json).unwrap();

        // encode → bytes → decode (what DynEncoder/DynDecoder do)
        let bytes = msg.encode_to_vec();
        let decoded = DynamicMessage::decode(input, bytes.as_slice()).unwrap();

        let back = serde_json::to_value(&decoded).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back["msg"], "hi");
        assert_eq!(back["n"], 7);
    }

    #[test]
    fn unknown_method_errors() {
        let dir = std::env::temp_dir().join(format!("pg-grpc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proto = dir.join("echo.proto");
        std::fs::write(&proto, ECHO_PROTO).unwrap();
        let fds = protox::compile([&proto], [&dir]).unwrap();
        let pool = DescriptorPool::from_file_descriptor_set(fds).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let service = pool.get_service_by_name("echo.Echo").unwrap();
        assert!(service.methods().find(|m| m.name() == "Nope").is_none());
    }
}
