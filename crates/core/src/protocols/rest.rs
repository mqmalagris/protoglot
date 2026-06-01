//! REST execution over `reqwest` (rustls, json). The Phase 1 MVP, now with
//! Phase 3 auth: header schemes (bearer/basic/oauth2), mTLS, and AWS SigV4.

use crate::auth::{self, AppliedAuth};
use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use protoglot_format::RestRequest;
use reqwest::{Client, Url};

pub async fn execute(
    req: &RestRequest,
    scope: &Scope,
    client: &Client,
    resolver: &Resolver,
) -> Result<ExecOutcome> {
    let applied = auth::prepare(&req.auth, scope, resolver, client).await?;

    // mTLS needs its own client built with the identity; otherwise share one.
    let mtls_client;
    let client: &Client = match &applied {
        AppliedAuth::Mtls(material) => {
            mtls_client = auth::build_mtls_client(material)?;
            &mtls_client
        }
        _ => client,
    };

    let method_str = resolver.resolve(&req.method, scope).await?;
    let method = reqwest::Method::from_bytes(method_str.trim().to_uppercase().as_bytes())
        .map_err(|e| Error::Request(format!("bad method `{method_str}`: {e}")))?;
    let url = resolver.resolve(&req.url, scope).await?;

    let mut headers: Vec<(String, String)> = Vec::with_capacity(req.headers.len() + 1);
    for (name, value) in &req.headers {
        headers.push((name.clone(), resolver.resolve(value, scope).await?));
    }
    auth::merge_header(&mut headers, &applied);

    let mut query: Vec<(String, String)> = Vec::with_capacity(req.query.len());
    for (k, v) in &req.query {
        query.push((k.clone(), resolver.resolve(v, scope).await?));
    }

    let body = match &req.body {
        Some(b) => Some(resolver.resolve(b, scope).await?),
        None => None,
    };

    // SigV4 must sign the final URI (query included) plus the headers/body, so
    // we resolve the full URL up front and sign before sending.
    if let AppliedAuth::Sigv4(material) = &applied {
        let final_url = build_url(&url, &query)?;
        let body_bytes = body.as_deref().map(str::as_bytes).unwrap_or(&[]);
        let signed = auth::sign_aws(material, method.as_str(), final_url.as_str(), &headers, body_bytes)?;
        headers.extend(signed);

        let mut rb = client.request(method, final_url);
        for (name, value) in &headers {
            rb = rb.header(name.as_str(), value.clone());
        }
        if let Some(body) = &body {
            rb = rb.body(body.clone());
        }
        let resp = rb.send().await?;
        return Ok(ExecOutcome::ok(RawResponse::from_response(resp).await?));
    }

    let mut rb = client.request(method, &url);
    for (name, value) in &headers {
        rb = rb.header(name.as_str(), value.clone());
    }
    if !query.is_empty() {
        rb = rb.query(&query);
    }
    if let Some(body) = &body {
        rb = rb.body(body.clone());
    }

    let resp = rb.send().await?;
    Ok(ExecOutcome::ok(RawResponse::from_response(resp).await?))
}

fn build_url(url: &str, query: &[(String, String)]) -> Result<Url> {
    if query.is_empty() {
        Url::parse(url).map_err(|e| Error::Request(format!("invalid url `{url}`: {e}")))
    } else {
        Url::parse_with_params(url, query)
            .map_err(|e| Error::Request(format!("invalid url `{url}`: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protoglot_format::Auth;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn rest(url: String, auth: Option<Auth>) -> RestRequest {
        RestRequest {
            name: "x".into(),
            method: "GET".into(),
            url,
            headers: Default::default(),
            query: Default::default(),
            body: None,
            assertions: vec![],
            capture: vec![],
            auth,
        }
    }

    #[tokio::test]
    async fn applies_bearer_auth_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(header("authorization", "Bearer xyz"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let req = rest(
            format!("{}/x", server.uri()),
            Some(Auth::Bearer {
                token: "xyz".into(),
            }),
        );
        let out = execute(&req, &Scope::new(), &Client::new(), &Resolver::new())
            .await
            .unwrap();
        // 200 only if the Authorization header matched; otherwise wiremock 404s.
        assert_eq!(out.response.status, 200);
    }

    #[tokio::test]
    async fn sigv4_signs_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let req = rest(
            format!("{}/path", server.uri()),
            Some(Auth::AwsSigv4 {
                access_key_id: "AKIDEXAMPLE".into(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
                session_token: None,
                region: "us-east-1".into(),
                service: "execute-api".into(),
            }),
        );
        let out = execute(&req, &Scope::new(), &Client::new(), &Resolver::new())
            .await
            .unwrap();
        assert_eq!(out.response.status, 200);

        let requests = server.received_requests().await.unwrap();
        let auth = requests[0]
            .headers
            .get("authorization")
            .expect("authorization header present")
            .to_str()
            .unwrap();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
            "unexpected authorization header: {auth}"
        );
        assert!(requests[0].headers.contains_key("x-amz-date"));
    }
}
