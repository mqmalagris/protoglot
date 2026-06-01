//! REST execution over `reqwest` (rustls, json). The Phase 1 MVP.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::RawResponse;
use protoglot_format::RestRequest;
use reqwest::Client;

pub async fn execute(
    req: &RestRequest,
    scope: &Scope,
    client: &Client,
    resolver: &Resolver,
) -> Result<RawResponse> {
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

    Ok(RawResponse {
        status,
        headers,
        body,
        content_type,
    })
}
