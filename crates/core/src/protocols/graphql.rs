//! GraphQL over HTTP (Phase 2). POST `{query, variables, operationName?}`,
//! reusing the REST headers + variable resolution. A non-empty `errors` field
//! in the JSON response is a protocol-level failure even on HTTP 200 (§spec F2).

use crate::auth::{self, AppliedAuth};
use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use protoglot_format::GraphqlRequest;
use reqwest::Client;
use serde_json::{Map, Value};

pub async fn execute(
    req: &GraphqlRequest,
    scope: &Scope,
    client: &Client,
    resolver: &Resolver,
) -> Result<ExecOutcome> {
    let url = resolver.resolve(&req.url, scope).await?;
    let query = resolver.resolve(&req.query, scope).await?;

    let mut body = Map::new();
    body.insert("query".into(), Value::String(query));

    if !req.variables.is_empty() {
        let mut vars = Map::with_capacity(req.variables.len());
        for (k, v) in &req.variables {
            vars.insert(k.clone(), resolve_json(v, scope, resolver).await?);
        }
        body.insert("variables".into(), Value::Object(vars));
    }
    if let Some(op) = &req.operation_name {
        body.insert(
            "operationName".into(),
            Value::String(resolver.resolve(op, scope).await?),
        );
    }

    let applied = auth::prepare(&req.auth, scope, resolver, client).await?;
    if matches!(applied, AppliedAuth::Sigv4(_) | AppliedAuth::Mtls(_)) {
        return Err(Error::Auth(
            "aws_sigv4 / mtls auth is only supported on REST requests".into(),
        ));
    }

    let mut headers: Vec<(String, String)> = Vec::with_capacity(req.headers.len() + 1);
    for (name, value) in &req.headers {
        headers.push((name.clone(), resolver.resolve(value, scope).await?));
    }
    auth::merge_header(&mut headers, &applied);

    let mut rb = client.post(&url).json(&Value::Object(body));
    for (name, value) in &headers {
        rb = rb.header(name.as_str(), value.clone());
    }

    let resp = rb.send().await?;
    let response = RawResponse::from_response(resp).await?;
    let protocol_failure = detect_errors(&response);
    Ok(ExecOutcome {
        response,
        protocol_failure,
    })
}

/// Resolve `{{...}}` inside string-valued GraphQL variables; non-strings pass
/// through untouched. (Templating into nested objects is not supported yet.)
async fn resolve_json(value: &Value, scope: &Scope, resolver: &Resolver) -> Result<Value> {
    match value {
        Value::String(s) => Ok(Value::String(resolver.resolve(s, scope).await?)),
        other => Ok(other.clone()),
    }
}

/// Returns a failure message if the GraphQL response carries a non-empty
/// `errors` array.
pub fn detect_errors(response: &RawResponse) -> Option<String> {
    let json = response.json()?;
    match json.get("errors") {
        Some(Value::Array(errs)) if !errs.is_empty() => {
            let first = errs
                .first()
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            Some(format!(
                "GraphQL returned {} error(s); first: {first}",
                errs.len()
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(url: String) -> GraphqlRequest {
        GraphqlRequest {
            name: "q".into(),
            url,
            query: "{ user { id } }".into(),
            operation_name: None,
            variables: Map::new(),
            headers: Default::default(),
            assertions: vec![],
            capture: vec![],
            auth: None,
            data: None,
            snapshot: None,
            pre_script: None,
            post_script: None,
        }
    }

    #[tokio::test]
    async fn ok_when_no_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"user": {"id": "1"}}})))
            .mount(&server)
            .await;

        let out = execute(
            &req(format!("{}/graphql", server.uri())),
            &Scope::new(),
            &Client::new(),
            &Resolver::new(),
        )
        .await
        .unwrap();

        assert_eq!(out.response.status, 200);
        assert!(out.protocol_failure.is_none());
    }

    #[tokio::test]
    async fn errors_field_flags_failure_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"errors": [{"message": "boom"}]})),
            )
            .mount(&server)
            .await;

        let out = execute(
            &req(format!("{}/graphql", server.uri())),
            &Scope::new(),
            &Client::new(),
            &Resolver::new(),
        )
        .await
        .unwrap();

        assert_eq!(out.response.status, 200);
        let msg = out.protocol_failure.expect("errors should flag failure");
        assert!(msg.contains("boom"), "msg was: {msg}");
    }
}
