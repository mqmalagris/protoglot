//! WebSocket (Phase 5) — the **scriptable** model: a roteiro of send/expect
//! steps, runnable in CI and the desktop alike (real-time streaming UI is a
//! separate, later concern). Connects over `tokio-tungstenite` (rustls for
//! `wss`). The received frames are collected into a transcript body so the
//! existing assertions (`body_contains`, jsonpath) still apply; a failed
//! `expect_contains` step is a protocol-level failure.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use futures::{SinkExt, StreamExt};
use protoglot_format::WebsocketRequest;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type WsClient = WebSocketStream<MaybeTlsStream<TcpStream>>;

const DEFAULT_STEP_TIMEOUT_MS: u64 = 5000;

pub async fn execute(
    req: &WebsocketRequest,
    scope: &Scope,
    resolver: &Resolver,
) -> Result<ExecOutcome> {
    let url = resolver.resolve(&req.url, scope).await?;
    let (mut ws, _resp) = timeout(Duration::from_secs(10), connect_async(&url))
        .await
        .map_err(|_| Error::Request(format!("websocket connect to `{url}` timed out")))?
        .map_err(|e| Error::Request(format!("websocket connect to `{url}`: {e}")))?;

    let mut transcript: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for (i, step) in req.steps.iter().enumerate() {
        if let Some(send) = &step.send {
            let payload = resolver.resolve(send, scope).await?;
            transcript.push(format!("> {payload}"));
            ws.send(Message::Text(payload))
                .await
                .map_err(|e| Error::Request(format!("websocket send: {e}")))?;
        }

        if let Some(expect) = &step.expect_contains {
            let expect = resolver.resolve(expect, scope).await?;
            let dur = Duration::from_millis(step.timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS));
            match recv_until(&mut ws, &expect, dur, &mut transcript).await {
                Ok(true) => {}
                Ok(false) => failures.push(format!(
                    "step {}: did not receive `{expect}` within {}ms",
                    i + 1,
                    dur.as_millis()
                )),
                Err(e) => {
                    failures.push(format!("step {}: {e}", i + 1));
                    break;
                }
            }
        }
    }

    let _ = ws.close(None).await;

    let response = RawResponse {
        status: 101, // Switching Protocols — what a WS handshake returns.
        headers: Vec::new(),
        body: transcript.join("\n").into_bytes(),
        content_type: Some("text/plain; charset=utf-8".to_string()),
    };
    let protocol_failure = (!failures.is_empty()).then(|| failures.join("; "));
    Ok(ExecOutcome {
        response,
        protocol_failure,
    })
}

/// Read frames until one contains `expect` (Ok(true)) or the deadline passes
/// (Ok(false)). A closed connection or transport error is `Err`.
async fn recv_until(
    ws: &mut WsClient,
    expect: &str,
    dur: Duration,
    transcript: &mut Vec<String>,
) -> std::result::Result<bool, String> {
    let deadline = Instant::now() + dur;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        match timeout(remaining, ws.next()).await {
            Err(_) => return Ok(false), // timed out
            Ok(None) => return Err("connection closed".into()),
            Ok(Some(Err(e))) => return Err(format!("recv error: {e}")),
            Ok(Some(Ok(msg))) => {
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => return Err("connection closed".into()),
                    _ => continue, // ping/pong/frame
                };
                transcript.push(format!("< {text}"));
                if text.contains(expect) {
                    return Ok(true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protoglot_format::WsStep;
    use tokio::net::TcpListener;

    async fn echo_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    while let Some(Ok(msg)) = ws.next().await {
                        match msg {
                            Message::Text(_) | Message::Binary(_) => {
                                let _ = ws.send(msg).await;
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            }
        });
        format!("ws://{addr}")
    }

    fn req(url: String, steps: Vec<WsStep>) -> WebsocketRequest {
        WebsocketRequest {
            name: "echo".into(),
            url,
            steps,
        }
    }

    #[tokio::test]
    async fn send_then_expect_matches() {
        let url = echo_server().await;
        let steps = vec![
            WsStep {
                send: Some(r#"{"type":"ping"}"#.into()),
                expect_contains: None,
                timeout_ms: None,
            },
            WsStep {
                send: None,
                expect_contains: Some("ping".into()),
                timeout_ms: Some(2000),
            },
        ];
        let out = execute(&req(url, steps), &Scope::new(), &Resolver::new())
            .await
            .unwrap();
        assert!(out.protocol_failure.is_none(), "{:?}", out.protocol_failure);
        assert!(out.response.text().contains("> {\"type\":\"ping\"}"));
        assert!(out.response.text().contains("< {\"type\":\"ping\"}"));
    }

    #[tokio::test]
    async fn unmet_expectation_is_protocol_failure() {
        let url = echo_server().await;
        let steps = vec![
            WsStep {
                send: Some("ping".into()),
                expect_contains: None,
                timeout_ms: None,
            },
            WsStep {
                send: None,
                expect_contains: Some("pong".into()), // echo never sends "pong"
                timeout_ms: Some(300),
            },
        ];
        let out = execute(&req(url, steps), &Scope::new(), &Resolver::new())
            .await
            .unwrap();
        assert!(out.protocol_failure.is_some());
    }
}
