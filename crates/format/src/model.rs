//! Data model for a collection as stored on disk.
//!
//! Each variant struct is deserialized from a single TOML file. The `kind`
//! field selects the variant; `rest` is the default and may be omitted (see
//! [`crate::parse_request_str`]). Unknown fields are ignored so the `kind`
//! discriminator does not need to appear on every struct.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A map of variable name -> string value. `BTreeMap` keeps diffs stable.
pub type VarMap = BTreeMap<String, String>;

/// Root collection config (`protoglot.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollectionConfig {
    #[serde(default)]
    pub name: Option<String>,
    /// Collection-scoped variables (lowest precedence).
    #[serde(default)]
    pub variables: VarMap,
}

/// Which protocol a request speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Rest,
    Graphql,
    Grpc,
    Websocket,
    Soap,
}

/// A request, parsed from one TOML file. Heterogeneous by protocol — modeled as
/// an enum so the runner matches on it (type-safe, no `Box<dyn Trait>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Request {
    Rest(RestRequest),
    Graphql(GraphqlRequest),
    Grpc(GrpcRequest),
    Websocket(WebsocketRequest),
    Soap(SoapRequest),
}

impl Request {
    pub fn name(&self) -> &str {
        match self {
            Request::Rest(r) => &r.name,
            Request::Graphql(r) => &r.name,
            Request::Grpc(r) => &r.name,
            Request::Websocket(r) => &r.name,
            Request::Soap(r) => &r.name,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Request::Rest(_) => Kind::Rest,
            Request::Graphql(_) => Kind::Graphql,
            Request::Grpc(_) => Kind::Grpc,
            Request::Websocket(_) => Kind::Websocket,
            Request::Soap(_) => Kind::Soap,
        }
    }

    /// Assertions declared on this request (empty for protocols that don't
    /// carry HTTP-style assertions yet).
    pub fn assertions(&self) -> &[Assertion] {
        match self {
            Request::Rest(r) => &r.assertions,
            Request::Graphql(r) => &r.assertions,
            Request::Soap(r) => &r.assertions,
            _ => &[],
        }
    }

    /// Declarative captures (§10) declared on this request.
    pub fn captures(&self) -> &[Capture] {
        match self {
            Request::Rest(r) => &r.capture,
            Request::Graphql(r) => &r.capture,
            Request::Soap(r) => &r.capture,
            _ => &[],
        }
    }
}

fn default_method() -> String {
    "GET".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestRequest {
    pub name: String,
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: VarMap,
    #[serde(default)]
    pub query: VarMap,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    #[serde(default)]
    pub capture: Vec<Capture>,
    #[serde(default)]
    pub auth: Option<Auth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphqlRequest {
    pub name: String,
    pub url: String,
    pub query: String,
    #[serde(default)]
    pub operation_name: Option<String>,
    #[serde(default)]
    pub variables: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub headers: VarMap,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    #[serde(default)]
    pub capture: Vec<Capture>,
    #[serde(default)]
    pub auth: Option<Auth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcRequest {
    pub name: String,
    pub target: String,
    pub service: String,
    pub method: String,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub proto: Option<String>,
    #[serde(default)]
    pub message: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebsocketRequest {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub steps: Vec<WsStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsStep {
    #[serde(default)]
    pub send: Option<String>,
    #[serde(default)]
    pub expect_contains: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoapRequest {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub soap_action: Option<String>,
    #[serde(default)]
    pub soap_version: Option<String>,
    pub body: String,
    #[serde(default)]
    pub headers: VarMap,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    #[serde(default)]
    pub capture: Vec<Capture>,
    #[serde(default)]
    pub auth: Option<Auth>,
}

/// Authentication for a request. Header-style schemes (`bearer`, `basic`,
/// `oauth2_client_credentials`) work on any HTTP protocol; `aws_sigv4` and
/// `mtls` apply to REST. The engine that applies these lives in `core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    Oauth2ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        #[serde(default)]
        scopes: Vec<String>,
        #[serde(default)]
        audience: Option<String>,
    },
    AwsSigv4 {
        access_key_id: String,
        secret_access_key: String,
        #[serde(default)]
        session_token: Option<String>,
        region: String,
        service: String,
    },
    /// Mutual TLS. Provide either a combined `pem` (cert + private key) or both
    /// `cert` and `key` paths.
    Mtls {
        #[serde(default)]
        cert: Option<String>,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        pem: Option<String>,
    },
}

/// A declarative assertion. The engine that evaluates these lives in `core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Assertion {
    Status {
        #[serde(default)]
        equals: Option<u16>,
        /// inclusive `[min, max]`
        #[serde(default)]
        in_range: Option<[u16; 2]>,
    },
    Jsonpath {
        path: String,
        #[serde(default)]
        exists: Option<bool>,
        #[serde(default)]
        equals: Option<serde_json::Value>,
        #[serde(default)]
        matches: Option<String>,
    },
    /// XPath — engine arrives in Phase 2 (SOAP); parsed here for forward-compat.
    Xpath {
        path: String,
        #[serde(default)]
        exists: Option<bool>,
        #[serde(default)]
        equals: Option<String>,
        #[serde(default)]
        namespaces: VarMap,
    },
    Header {
        name: String,
        #[serde(default)]
        exists: Option<bool>,
        #[serde(default)]
        equals: Option<String>,
    },
    ResponseTime {
        max_ms: u64,
    },
    BodyContains {
        value: String,
    },
}

/// Declarative response capture (Phase 2 wiring; parsed now).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    pub var: String,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}
