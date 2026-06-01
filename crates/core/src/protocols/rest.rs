//! REST execution over `reqwest` (rustls, json). The Phase 1 MVP, now with
//! Phase 3 auth: header schemes (bearer/basic/oauth2), mTLS, and SigV4 (B).

use crate::auth::{self, AppliedAuth};
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
    let applied = auth::prepare(&req.auth, scope, resolver, client).await?;

    if matches!(applied, AppliedAuth::Sigv4(_)) {
        return Err(Error::Auth(
            "aws_sigv4 signing arrives in the next increment".into(),
        ));
    }

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

    let mut rb = client.request(method, &url);
    for (name, value) in &headers {
        rb = rb.header(name.as_str(), value.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn rest(url: String, auth: Option<protoglot_format::Auth>) -> RestRequest {
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
            Some(protoglot_format::Auth::Bearer {
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
    async fn sigv4_is_not_yet_supported() {
        let req = rest(
            "http://example.invalid/".into(),
            Some(protoglot_format::Auth::AwsSigv4 {
                access_key_id: "AKID".into(),
                secret_access_key: "secret".into(),
                session_token: None,
                region: "us-east-1".into(),
                service: "execute-api".into(),
            }),
        );
        let err = execute(&req, &Scope::new(), &Client::new(), &Resolver::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
    }
}
