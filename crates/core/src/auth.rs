//! Authentication (§spec Phase 3). Turns a declarative [`Auth`] into something
//! a protocol can apply: an `Authorization` header (bearer/basic/oauth2), an
//! mTLS client identity, or AWS SigV4 signing material.
//!
//! OAuth2 client-credentials is implemented directly over `reqwest` rather than
//! via the `oauth2` crate — the flow is a single token POST, and avoiding the
//! crate keeps us off its churning builder API. The `oauth2` crate earns its
//! place when authorization-code + PKCE lands (interactive, Phase 3 follow-up).

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use protoglot_format::Auth;
use reqwest::Client;
use serde_json::Value;

/// Auth resolved against the run scope, ready for a protocol to apply.
#[derive(Debug, Clone)]
pub enum AppliedAuth {
    None,
    /// A header to inject (e.g. `Authorization: Bearer ...`). Overrides any
    /// same-named explicit header.
    Header { name: String, value: String },
    /// AWS SigV4 signing material (applied by REST; see increment B).
    Sigv4(Sigv4Material),
    /// mTLS client identity material (REST builds a dedicated client).
    Mtls(MtlsMaterial),
}

#[derive(Debug, Clone)]
pub struct Sigv4Material {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
    pub service: String,
}

#[derive(Debug, Clone)]
pub struct MtlsMaterial {
    pub cert: Option<String>,
    pub key: Option<String>,
    pub pem: Option<String>,
}

/// Resolve `auth` against the scope. May hit the network (OAuth2 token fetch),
/// hence `async` and the `client`.
pub async fn prepare(
    auth: &Option<Auth>,
    scope: &Scope,
    resolver: &Resolver,
    client: &Client,
) -> Result<AppliedAuth> {
    let Some(auth) = auth else {
        return Ok(AppliedAuth::None);
    };
    match auth {
        Auth::Bearer { token } => {
            let token = resolver.resolve(token, scope).await?;
            Ok(header("Authorization", format!("Bearer {token}")))
        }
        Auth::Basic { username, password } => {
            let user = resolver.resolve(username, scope).await?;
            let pass = resolver.resolve(password, scope).await?;
            let encoded = STANDARD.encode(format!("{user}:{pass}"));
            Ok(header("Authorization", format!("Basic {encoded}")))
        }
        Auth::Oauth2ClientCredentials {
            token_url,
            client_id,
            client_secret,
            scopes,
            audience,
        } => {
            let token_url = resolver.resolve(token_url, scope).await?;
            let client_id = resolver.resolve(client_id, scope).await?;
            let client_secret = resolver.resolve(client_secret, scope).await?;
            let audience = opt_resolve(audience, scope, resolver).await?;
            let token = fetch_client_credentials(
                client,
                &token_url,
                &client_id,
                &client_secret,
                scopes,
                audience.as_deref(),
            )
            .await?;
            Ok(header("Authorization", format!("Bearer {token}")))
        }
        Auth::AwsSigv4 {
            access_key_id,
            secret_access_key,
            session_token,
            region,
            service,
        } => Ok(AppliedAuth::Sigv4(Sigv4Material {
            access_key_id: resolver.resolve(access_key_id, scope).await?,
            secret_access_key: resolver.resolve(secret_access_key, scope).await?,
            session_token: opt_resolve(session_token, scope, resolver).await?,
            region: resolver.resolve(region, scope).await?,
            service: resolver.resolve(service, scope).await?,
        })),
        Auth::Mtls { cert, key, pem } => Ok(AppliedAuth::Mtls(MtlsMaterial {
            cert: opt_resolve(cert, scope, resolver).await?,
            key: opt_resolve(key, scope, resolver).await?,
            pem: opt_resolve(pem, scope, resolver).await?,
        })),
    }
}

fn header(name: &str, value: String) -> AppliedAuth {
    AppliedAuth::Header {
        name: name.to_string(),
        value,
    }
}

async fn opt_resolve(
    value: &Option<String>,
    scope: &Scope,
    resolver: &Resolver,
) -> Result<Option<String>> {
    match value {
        Some(v) => Ok(Some(resolver.resolve(v, scope).await?)),
        None => Ok(None),
    }
}

/// Inject a resolved `Header` auth into a header list, overriding any existing
/// same-named header (so auth wins over an explicit `Authorization`).
pub fn merge_header(headers: &mut Vec<(String, String)>, applied: &AppliedAuth) {
    if let AppliedAuth::Header { name, value } = applied {
        headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        headers.push((name.clone(), value.clone()));
    }
}

async fn fetch_client_credentials(
    client: &Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    scopes: &[String],
    audience: Option<&str>,
) -> Result<String> {
    let scope_str = scopes.join(" ");
    let mut form: Vec<(&str, &str)> = vec![("grant_type", "client_credentials")];
    if !scope_str.is_empty() {
        form.push(("scope", &scope_str));
    }
    if let Some(aud) = audience {
        form.push(("audience", aud));
    }

    let resp = client
        .post(token_url)
        .basic_auth(client_id, Some(client_secret))
        .form(&form)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(Error::Auth(format!(
            "oauth2 token endpoint returned {status}: {body}"
        )));
    }
    let json: Value = serde_json::from_str(&body)
        .map_err(|e| Error::Auth(format!("oauth2 token response was not JSON: {e}")))?;
    json.get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Auth("oauth2 token response missing `access_token`".into()))
}

/// Build a reqwest client carrying the mTLS client identity. Reads the PEM
/// bundle (or concatenates `cert` + `key`) and pins the rustls backend.
pub fn build_mtls_client(material: &MtlsMaterial) -> Result<Client> {
    let pem = if let Some(path) = &material.pem {
        std::fs::read(path)?
    } else {
        let cert = material
            .cert
            .as_ref()
            .ok_or_else(|| Error::Auth("mtls needs `pem`, or both `cert` and `key`".into()))?;
        let key = material
            .key
            .as_ref()
            .ok_or_else(|| Error::Auth("mtls needs `key` alongside `cert`".into()))?;
        let mut buf = std::fs::read(cert)?;
        buf.push(b'\n');
        buf.extend_from_slice(&std::fs::read(key)?);
        buf
    };

    let identity = reqwest::Identity::from_pem(&pem)?;
    Client::builder()
        .use_rustls_tls()
        .identity(identity)
        .build()
        .map_err(Error::from)
}

/// Compute the AWS SigV4 headers (`Authorization`, `x-amz-date`, and
/// `x-amz-security-token` when a session token is present) for the given
/// request. `uri` must already include the final query string. Returns the
/// header (name, value) pairs to add to the outgoing request.
pub fn sign_aws(
    material: &Sigv4Material,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Vec<(String, String)>> {
    use aws_credential_types::Credentials;
    use aws_sigv4::http_request::{
        sign, SignableBody, SignableRequest, SigningParams, SigningSettings,
    };
    use aws_sigv4::sign::v4;
    use std::time::SystemTime;

    let credentials = Credentials::new(
        material.access_key_id.clone(),
        material.secret_access_key.clone(),
        material.session_token.clone(),
        None,
        "protoglot",
    );
    let identity = credentials.into();

    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&material.region)
        .name(&material.service)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .map_err(|e| Error::Auth(format!("sigv4 params: {e}")))?;
    let signing_params: SigningParams = params.into();

    let header_iter = headers.iter().map(|(k, v)| (k.as_str(), v.as_str()));
    let signable = SignableRequest::new(method, uri, header_iter, SignableBody::Bytes(body))
        .map_err(|e| Error::Auth(format!("sigv4 signable request: {e}")))?;

    let (instructions, _signature) = sign(signable, &signing_params)
        .map_err(|e| Error::Auth(format!("sigv4 signing: {e}")))?
        .into_parts();

    Ok(instructions
        .headers()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn bearer_header() {
        let applied = prepare(
            &Some(Auth::Bearer {
                token: "abc".into(),
            }),
            &Scope::new(),
            &Resolver::new(),
            &Client::new(),
        )
        .await
        .unwrap();
        match applied {
            AppliedAuth::Header { name, value } => {
                assert_eq!(name, "Authorization");
                assert_eq!(value, "Bearer abc");
            }
            _ => panic!("expected header"),
        }
    }

    #[tokio::test]
    async fn basic_header_is_base64() {
        let applied = prepare(
            &Some(Auth::Basic {
                username: "u".into(),
                password: "p".into(),
            }),
            &Scope::new(),
            &Resolver::new(),
            &Client::new(),
        )
        .await
        .unwrap();
        // base64("u:p") == "dTpw"
        match applied {
            AppliedAuth::Header { value, .. } => assert_eq!(value, "Basic dTpw"),
            _ => panic!("expected header"),
        }
    }

    #[tokio::test]
    async fn oauth2_client_credentials_fetches_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"access_token": "tok-123", "token_type": "Bearer"})),
            )
            .mount(&server)
            .await;

        let auth = Some(Auth::Oauth2ClientCredentials {
            token_url: format!("{}/token", server.uri()),
            client_id: "id".into(),
            client_secret: "secret".into(),
            scopes: vec!["read".into()],
            audience: None,
        });
        let applied = prepare(&auth, &Scope::new(), &Resolver::new(), &Client::new())
            .await
            .unwrap();
        match applied {
            AppliedAuth::Header { value, .. } => assert_eq!(value, "Bearer tok-123"),
            _ => panic!("expected header"),
        }
    }

    #[tokio::test]
    async fn oauth2_error_response_surfaces() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
            .mount(&server)
            .await;
        let auth = Some(Auth::Oauth2ClientCredentials {
            token_url: server.uri(),
            client_id: "id".into(),
            client_secret: "bad".into(),
            scopes: vec![],
            audience: None,
        });
        let err = prepare(&auth, &Scope::new(), &Resolver::new(), &Client::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
    }

    #[test]
    fn mtls_missing_files_errors() {
        let material = MtlsMaterial {
            cert: None,
            key: None,
            pem: Some("/definitely/not/here.pem".into()),
        };
        assert!(build_mtls_client(&material).is_err());
    }

    #[test]
    fn merge_header_overrides_existing() {
        let mut headers = vec![("Authorization".into(), "old".into()), ("X".into(), "y".into())];
        merge_header(
            &mut headers,
            &AppliedAuth::Header {
                name: "authorization".into(),
                value: "new".into(),
            },
        );
        let auth: Vec<_> = headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("authorization"))
            .collect();
        assert_eq!(auth.len(), 1);
        assert_eq!(auth[0].1, "new");
    }
}
