//! One module per protocol. Only `rest` is implemented in Phase 1; the rest are
//! stubs returning [`Error::NotImplemented`](crate::error::Error::NotImplemented)
//! so the dispatch wiring is real and ready to extend.

pub mod graphql;
pub mod grpc;
pub mod rest;
pub mod soap;
pub mod websocket;

/// Protocol-agnostic response: raw bytes plus the metadata assertions need.
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

impl RawResponse {
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
