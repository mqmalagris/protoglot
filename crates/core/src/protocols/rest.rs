//! REST execution over `reqwest` (rustls, json). The Phase 1 MVP.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use protoglot_format::RestRequest;
use reqwest::Client;

pub async fn execute(
    req: &RestRequest,
    scope: &Scope,
    client: &Client,
    resolver: &Resolver,
) -> Result<ExecOutcome> {
    let method_str = resolver.resolve(&req.method, scope).await?;
    let method = reqwest::Method::from_bytes(method_str.trim().to_uppercase().as_bytes())
        .map_err(|e| Error::Request(format!("bad method `{method_str}`: {e}")))?;

    let url = resolver.resolve(&req.url, scope).await?;
    let mut rb = client.request(method, &url);

    for (name, value) in &req.headers {
        let value = resolver.resolve(value, scope).await?;
        rb = rb.header(name.as_str(), value);
    }

    if !req.query.is_empty() {
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(req.query.len());
        for (k, v) in &req.query {
            pairs.push((k.clone(), resolver.resolve(v, scope).await?));
        }
        rb = rb.query(&pairs);
    }

    if let Some(body) = &req.body {
        rb = rb.body(resolver.resolve(body, scope).await?);
    }

    let resp = rb.send().await?;
    Ok(ExecOutcome::ok(RawResponse::from_response(resp).await?))
}
