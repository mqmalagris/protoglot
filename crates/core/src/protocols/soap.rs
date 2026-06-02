//! SOAP over HTTP (Phase 2). POST an XML envelope. SOAP 1.1 by default
//! (`text/xml; charset=utf-8` + quoted `SOAPAction`); `soap_version = "1.2"`
//! switches to `application/soap+xml`. A `<Fault>` in the response (any
//! namespace prefix) is a protocol-level failure.

use crate::auth::{self, AppliedAuth};
use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::{ExecOutcome, RawResponse};
use crate::xml;
use protoglot_format::SoapRequest;
use reqwest::Client;

pub async fn execute(
    req: &SoapRequest,
    scope: &Scope,
    client: &Client,
    resolver: &Resolver,
) -> Result<ExecOutcome> {
    let url = resolver.resolve(&req.url, scope).await?;
    let body = resolver.resolve(&req.body, scope).await?;

    let content_type = match req.soap_version.as_deref() {
        Some("1.2") | Some("1_2") | Some("soap12") => "application/soap+xml; charset=utf-8",
        _ => "text/xml; charset=utf-8",
    };

    let applied = auth::prepare(&req.auth, scope, resolver, client).await?;
    if matches!(applied, AppliedAuth::Sigv4(_) | AppliedAuth::Mtls(_)) {
        return Err(Error::Auth(
            "aws_sigv4 / mtls auth is only supported on REST requests".into(),
        ));
    }

    let mut rb = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body);

    // SOAP 1.1 carries the action in a (quoted) SOAPAction header.
    if let Some(action) = &req.soap_action {
        let action = resolver.resolve(action, scope).await?;
        rb = rb.header("SOAPAction", format!("\"{action}\""));
    }

    let mut headers: Vec<(String, String)> = Vec::with_capacity(req.headers.len() + 1);
    for (name, value) in &req.headers {
        headers.push((name.clone(), resolver.resolve(value, scope).await?));
    }
    auth::merge_header(&mut headers, &applied);
    for (name, value) in &headers {
        rb = rb.header(name.as_str(), value.clone());
    }

    let resp = rb.send().await?;
    let response = RawResponse::from_response(resp).await?;
    let protocol_failure = if xml::has_soap_fault(&response.text()) {
        Some("SOAP Fault in response".to_string())
    } else {
        None
    };
    Ok(ExecOutcome {
        response,
        protocol_failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(url: String) -> SoapRequest {
        SoapRequest {
            name: "GetRate".into(),
            url,
            soap_action: Some("http://tempuri.org/GetRate".into()),
            soap_version: None,
            body: "<soap:Envelope/>".into(),
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

    const OK_BODY: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><GetRateResult>3.5</GetRateResult></soap:Body></soap:Envelope>"#;
    const FAULT_BODY: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><soap:Fault><faultstring>boom</faultstring></soap:Fault></soap:Body></soap:Envelope>"#;

    #[tokio::test]
    async fn ok_sends_soap11_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("content-type", "text/xml; charset=utf-8"))
            .and(header("soapaction", "\"http://tempuri.org/GetRate\""))
            .respond_with(ResponseTemplate::new(200).set_body_string(OK_BODY))
            .mount(&server)
            .await;

        let out = execute(&req(server.uri()), &Scope::new(), &Client::new(), &Resolver::new())
            .await
            .unwrap();
        assert_eq!(out.response.status, 200);
        assert!(out.protocol_failure.is_none());
    }

    #[tokio::test]
    async fn fault_flags_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string(FAULT_BODY))
            .mount(&server)
            .await;

        let out = execute(&req(server.uri()), &Scope::new(), &Client::new(), &Resolver::new())
            .await
            .unwrap();
        assert!(out.protocol_failure.is_some());
    }
}
