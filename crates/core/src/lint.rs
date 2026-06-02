//! Secrets-hygiene lint (§spec DX, on-brand git-first). Flags credentials that
//! are hardcoded into a collection instead of referenced via `{{$secret:...}}`,
//! plus a few high-signal secret patterns (AWS keys, JWTs, private keys). Meant
//! to back `protoglot lint` and an optional pre-commit hook.

use protoglot_format::{Auth, Request, VarMap};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub location: String,
    pub message: String,
}

impl Finding {
    fn new(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            message: message.into(),
        }
    }
}

/// A value is considered safe if it interpolates a variable/secret (`{{...}}`)
/// rather than embedding a literal credential.
fn is_templated(value: &str) -> bool {
    value.contains("{{")
}

const USE_SECRET: &str = "hardcoded credential — use {{$secret:NAME}}";

fn aws_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(AKIA|ASIA)[0-9A-Z]{16}\b").unwrap())
}

fn jwt_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Three base64url segments; the header segment starts `eyJ`. Matches even
    // when prefixed, e.g. `Bearer eyJ...`.
    RE.get_or_init(|| {
        Regex::new(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap()
    })
}

/// High-signal secret patterns inside an otherwise-literal value.
fn looks_like_secret(value: &str) -> Option<&'static str> {
    if value.contains("PRIVATE KEY-----") {
        return Some("embedded private key");
    }
    if aws_key_re().is_match(value) {
        return Some("looks like an AWS access key id");
    }
    if jwt_re().is_match(value) {
        return Some("looks like a JWT");
    }
    None
}

/// Names that should carry secrets, so a literal value is suspicious.
fn secret_named(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "secret",
        "password",
        "passwd",
        "token",
        "apikey",
        "api_key",
        "access_key",
        "private",
        "authorization",
    ]
    .iter()
    .any(|needle| k.contains(needle))
}

pub fn lint_request(request: &Request) -> Vec<Finding> {
    let mut findings = Vec::new();

    let auth: Option<&Auth> = match request {
        Request::Rest(r) => r.auth.as_ref(),
        Request::Graphql(r) => r.auth.as_ref(),
        Request::Soap(r) => r.auth.as_ref(),
        _ => None,
    };
    if let Some(auth) = auth {
        lint_auth(auth, &mut findings);
    }

    let headers: Option<&VarMap> = match request {
        Request::Rest(r) => Some(&r.headers),
        Request::Graphql(r) => Some(&r.headers),
        Request::Soap(r) => Some(&r.headers),
        _ => None,
    };
    if let Some(headers) = headers {
        for (name, value) in headers {
            if is_templated(value) {
                continue;
            }
            if let Some(reason) = looks_like_secret(value) {
                findings.push(Finding::new(format!("header {name}"), reason));
            } else if secret_named(name) {
                findings.push(Finding::new(format!("header {name}"), USE_SECRET));
            }
        }
    }

    findings
}

fn lint_auth(auth: &Auth, findings: &mut Vec<Finding>) {
    let mut check = |value: &str, location: &str| {
        if !is_templated(value) {
            findings.push(Finding::new(location, USE_SECRET));
        }
    };
    match auth {
        Auth::Bearer { token } => check(token, "auth.token"),
        Auth::Basic { password, .. } => check(password, "auth.password"),
        Auth::Oauth2ClientCredentials { client_secret, .. } => {
            check(client_secret, "auth.client_secret")
        }
        Auth::AwsSigv4 {
            secret_access_key, ..
        } => check(secret_access_key, "auth.secret_access_key"),
        Auth::Mtls { .. } => {} // file paths, not embedded secrets
    }
}

/// Lint an environment file's variables.
pub fn lint_env(vars: &VarMap) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (key, value) in vars {
        if is_templated(value) {
            continue;
        }
        if let Some(reason) = looks_like_secret(value) {
            findings.push(Finding::new(key.clone(), reason));
        } else if secret_named(key) {
            findings.push(Finding::new(key.clone(), USE_SECRET));
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use protoglot_format::parse_request_str;

    fn req(toml: &str) -> Request {
        parse_request_str(toml).unwrap()
    }

    #[test]
    fn hardcoded_bearer_flagged_templated_clean() {
        let bad = req(r#"
            name = "x"
            url = "http://e"
            [auth]
            type = "bearer"
            token = "sk-live-abc123"
        "#);
        assert_eq!(lint_request(&bad).len(), 1);

        let good = req(r#"
            name = "x"
            url = "http://e"
            [auth]
            type = "bearer"
            token = "{{$secret:api_token}}"
        "#);
        assert!(lint_request(&good).is_empty());
    }

    #[test]
    fn jwt_in_header_flagged() {
        let r = req(r#"
            name = "x"
            url = "http://e"
            [headers]
            Authorization = "Bearer eyJhbGc.eyJzdWI.sig"
        "#);
        let f = lint_request(&r);
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("JWT"));
    }

    #[test]
    fn env_hardcoded_secret_and_aws_key() {
        let mut env = VarMap::new();
        env.insert("apiToken".into(), "literal-value".into());
        env.insert("baseUrl".into(), "https://api.example.com".into());
        env.insert("awsKey".into(), "AKIAIOSFODNN7EXAMPLE".into());
        env.insert("token".into(), "{{$secret:tok}}".into());

        let f = lint_env(&env);
        // apiToken (secret-named literal) + awsKey (pattern); baseUrl & templated token clean.
        assert_eq!(f.len(), 2);
        assert!(f.iter().any(|x| x.location == "apiToken"));
        assert!(f.iter().any(|x| x.message.contains("AWS")));
    }

    #[test]
    fn sigv4_secret_literal_flagged() {
        let r = req(r#"
            name = "x"
            url = "http://e"
            [auth]
            type = "aws_sigv4"
            access_key_id = "{{AWS_KEY}}"
            secret_access_key = "hardcoded-secret"
            region = "us-east-1"
            service = "execute-api"
        "#);
        let f = lint_request(&r);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].location, "auth.secret_access_key");
    }
}
