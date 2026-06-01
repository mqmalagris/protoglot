//! WebSocket — stub (Phase 5). Scriptable send/expect roteiro over
//! `tokio-tungstenite`; streaming UI via Tauri events.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::RawResponse;
use protoglot_format::WebsocketRequest;

pub async fn execute(
    _req: &WebsocketRequest,
    _scope: &Scope,
    _resolver: &Resolver,
) -> Result<RawResponse> {
    Err(Error::NotImplemented("websocket"))
}
