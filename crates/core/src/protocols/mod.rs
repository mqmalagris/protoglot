//! One module per protocol. Only `rest` is implemented in Phase 1; the rest are
//! stubs returning [`Error::NotImplemented`](crate::error::Error::NotImplemented)
//! so the dispatch wiring is real and ready to extend.

pub mod graphql;
pub mod grpc;
pub mod rest;
pub mod soap;
pub mod websocket;

/// What a protocol execution yields: the transport response plus an optional
/// protocol-level failure (GraphQL `errors`, SOAP `Fault`) that flips the
/// result to `Failed` even when HTTP succeeded. Detected inside `protocols/` so
/// the runner stays free of protocol logic (§2).
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub response: RawResponse,
    pub protocol_failure: Option<String>,
}

impl ExecOutcome {
    /// A clean outcome — no protocol-level failure.
    pub fn ok(response: RawResponse) -> Self {
        Self {
            response,
            protocol_failure: None,
        }
    }
}

/// Protocol-agnostic response: raw bytes plus the metadata assertions need.
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

impl RawResponse {
    /// Drain a `reqwest::Response` into a `RawResponse`. Shared by every
    /// HTTP-based protocol (REST/GraphQL/SOAP).
    pub async fn from_response(resp: reqwest::Response) -> crate::error::Result<Self> {
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or_default().to_string()))
            .collect();
        let body = resp.bytes().await?.to_vec();
        Ok(Self {
            status,
            headers,
            body,
            content_type,
        })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }

    /// Case-insensitive header lookup (HTTP header names are case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}
