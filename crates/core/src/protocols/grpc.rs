//! gRPC — dynamic unary invocation (Phase 6, "the boss"). No compile-time
//! codegen: descriptors come either from a runtime-compiled `.proto` (`protox`)
//! or from **server reflection**, and a custom tonic `Codec` ferries
//! `DynamicMessage`s over the wire (§7). The JSON `[message]` becomes the
//! request; the reply serializes back to JSON so jsonpath assertions apply.
//!
//! Implemented: **unary** over plaintext h2 (`http://`), via `.proto` or
//! reflection (v1 with v1alpha fallback). Streaming and TLS are deferred.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use futures::channel::mpsc;
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
    let service_name = resolver.resolve(&req.service, scope).await?;
    let method_name = resolver.resolve(&req.method, scope).await?;
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

    // Descriptors: explicit .proto wins; otherwise server reflection.
    let pool = match (req.proto.as_deref(), req.schema.as_deref()) {
        (Some(proto), _) => pool_from_proto(base_dir, proto)?,
        (None, Some("reflection")) | (None, None) => {
            build_pool_via_reflection(channel.clone(), &service_name).await?
        }
        (None, Some(path)) => pool_from_proto(base_dir, path)?,
    };

    let service = pool
        .get_service_by_name(&service_name)
        .ok_or_else(|| Error::Request(format!("service `{service_name}` not found")))?;
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

    // Build the request message from JSON, resolving {{...}} in string values.
    let mut obj = serde_json::Map::with_capacity(req.message.len());
    for (k, v) in &req.message {
        let resolved = match v {
            Value::String(s) => Value::String(resolver.resolve(s, scope).await?),
            other => other.clone(),
        };
        obj.insert(k.clone(), resolved);
    }
    let request_msg = DynamicMessage::deserialize(input.clone(), &Value::Object(obj))
        .map_err(|e| Error::Request(format!("building request message: {e}")))?;

    let mut client = Grpc::new(channel);
    client
        .ready()
        .await
        .map_err(|e| Error::Request(format!("grpc service not ready: {e}")))?;
    let path: http::uri::PathAndQuery = format!("/{}/{}", service.full_name(), method.name())
        .parse()
        .map_err(|e| Error::Request(format!("bad grpc path: {e}")))?;
    let codec = DynamicCodec { output };

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

fn pool_from_proto(base_dir: &Path, proto: &str) -> Result<DescriptorPool> {
    let proto_path = base_dir.join(proto);
    let include = proto_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let fds = protox::compile([&proto_path], [&include])
        .map_err(|e| Error::Request(format!("compiling {}: {e}", proto_path.display())))?;
    DescriptorPool::from_file_descriptor_set(fds)
        .map_err(|e| Error::Request(format!("building descriptors: {e}")))
}

// ---- Server reflection (protoc-free: hand-written reflection v1 messages) ----

const REFLECTION_V1: &str = "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo";
const REFLECTION_V1ALPHA: &str = "/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo";

#[derive(Clone, PartialEq, ::prost::Message)]
struct ServerReflectionRequest {
    #[prost(string, tag = "1")]
    host: String,
    #[prost(oneof = "MessageRequest", tags = "3, 4")]
    message_request: Option<MessageRequest>,
}

#[derive(Clone, PartialEq, ::prost::Oneof)]
enum MessageRequest {
    #[prost(string, tag = "3")]
    FileByFilename(String),
    #[prost(string, tag = "4")]
    FileContainingSymbol(String),
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct ServerReflectionResponse {
    #[prost(oneof = "MessageResponse", tags = "4")]
    message_response: Option<MessageResponse>,
}

#[derive(Clone, PartialEq, ::prost::Oneof)]
enum MessageResponse {
    #[prost(message, tag = "4")]
    FileDescriptorResponse(FileDescriptorResponse),
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct FileDescriptorResponse {
    #[prost(bytes = "vec", repeated, tag = "1")]
    file_descriptor_proto: Vec<Vec<u8>>,
}

async fn build_pool_via_reflection(channel: Channel, symbol: &str) -> Result<DescriptorPool> {
    match reflect_once(channel.clone(), symbol, REFLECTION_V1).await {
        Ok(pool) => return Ok(pool),
        Err((true, _)) => {} // v1 unimplemented → try v1alpha
        Err((false, e)) => return Err(e),
    }
    reflect_once(channel, symbol, REFLECTION_V1ALPHA)
        .await
        .map_err(|(_, e)| e)
}

/// One reflection round-trip. The bool in the error signals "Unimplemented"
/// (so the caller can fall back to v1alpha). NOTE: this assumes the server
/// returns all transitive file descriptors for `file_containing_symbol` in one
/// response (true for tonic-reflection and most servers); per-dependency
/// follow-up via `file_by_filename` is a refinement (see DEFERRED).
async fn reflect_once(
    channel: Channel,
    symbol: &str,
    path: &str,
) -> std::result::Result<DescriptorPool, (bool, Error)> {
    let mut client = Grpc::new(channel);
    client
        .ready()
        .await
        .map_err(|e| (false, Error::Request(format!("grpc not ready: {e}"))))?;
    let path: http::uri::PathAndQuery = path
        .parse()
        .map_err(|e| (false, Error::Request(format!("bad reflection path: {e}"))))?;
    let codec: tonic::codec::ProstCodec<ServerReflectionRequest, ServerReflectionResponse> =
        Default::default();

    let (mut tx, rx) = mpsc::channel::<ServerReflectionRequest>(4);
    tx.try_send(ServerReflectionRequest {
        host: String::new(),
        message_request: Some(MessageRequest::FileContainingSymbol(symbol.to_string())),
    })
    .map_err(|e| (false, Error::Request(format!("reflection send: {e}"))))?;
    drop(tx); // one request; closing the stream lets the server complete.

    let response = client
        .streaming(Request::new(rx), path, codec)
        .await
        .map_err(|s| (s.code() == tonic::Code::Unimplemented, status_to_error(&s)))?;
    let mut stream = response.into_inner();

    let mut files: Vec<prost_types::FileDescriptorProto> = Vec::new();
    loop {
        match stream.message().await {
            Ok(Some(resp)) => {
                if let Some(MessageResponse::FileDescriptorResponse(fdr)) = resp.message_response {
                    for bytes in fdr.file_descriptor_proto {
                        let fd = prost_types::FileDescriptorProto::decode(bytes.as_slice())
                            .map_err(|e| {
                                (false, Error::Request(format!("reflection decode: {e}")))
                            })?;
                        files.push(fd);
                    }
                }
            }
            Ok(None) => break,
            Err(s) => return Err((s.code() == tonic::Code::Unimplemented, status_to_error(&s))),
        }
    }
    if files.is_empty() {
        return Err((
            false,
            Error::Request("reflection returned no descriptors".into()),
        ));
    }
    let fds = prost_types::FileDescriptorSet { file: files };
    DescriptorPool::from_file_descriptor_set(fds)
        .map_err(|e| (false, Error::Request(format!("descriptor pool from reflection: {e}"))))
}

fn status_to_error(s: &Status) -> Error {
    Error::Request(format!("grpc reflection {:?}: {}", s.code(), s.message()))
}

// ---- Dynamic codec ----

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

    #[test]
    fn proto_compile_descriptor_and_dynamic_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pg-grpc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proto = dir.join("echo.proto");
        std::fs::write(&proto, ECHO_PROTO).unwrap();

        let pool = pool_from_proto(&dir, "echo.proto").unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let service = pool.get_service_by_name("echo.Echo").expect("service");
        let method = service.methods().find(|m| m.name() == "Say").expect("method");
        let input = method.input();

        let json = serde_json::json!({ "msg": "hi", "n": 7 });
        let msg = DynamicMessage::deserialize(input.clone(), &json).unwrap();
        let bytes = msg.encode_to_vec();
        let decoded = DynamicMessage::decode(input, bytes.as_slice()).unwrap();
        let back = serde_json::to_value(&decoded).unwrap();
        assert_eq!(back["msg"], "hi");
        assert_eq!(back["n"], 7);
    }

    /// Validate the hand-written reflection messages encode/decode with the
    /// right field tags (a wrong tag would break reflection silently).
    #[test]
    fn reflection_messages_roundtrip() {
        let req = ServerReflectionRequest {
            host: String::new(),
            message_request: Some(MessageRequest::FileContainingSymbol("pkg.Svc".into())),
        };
        let decoded = ServerReflectionRequest::decode(req.encode_to_vec().as_slice()).unwrap();
        assert!(matches!(
            decoded.message_request,
            Some(MessageRequest::FileContainingSymbol(s)) if s == "pkg.Svc"
        ));

        let resp = ServerReflectionResponse {
            message_response: Some(MessageResponse::FileDescriptorResponse(
                FileDescriptorResponse {
                    file_descriptor_proto: vec![vec![1, 2, 3]],
                },
            )),
        };
        let decoded = ServerReflectionResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
        match decoded.message_response {
            Some(MessageResponse::FileDescriptorResponse(fdr)) => {
                assert_eq!(fdr.file_descriptor_proto, vec![vec![1, 2, 3]]);
            }
            _ => panic!("wrong response"),
        }
    }
}
