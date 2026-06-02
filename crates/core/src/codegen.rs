//! Export a request as a runnable snippet (curl / fetch / reqwest). DX win: a
//! shareable, debuggable command straight from a collection. REST only; auth
//! that can't be rendered statically (oauth2/sigv4/mtls) is noted as a comment.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use protoglot_format::{Auth, Request, RestRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Curl,
    Fetch,
    Reqwest,
}

pub fn generate(request: &Request, target: Target, scope: &Scope) -> Result<String> {
    let rest = match request {
        Request::Rest(r) => r,
        _ => {
            return Err(Error::Request(
                "codegen currently supports REST requests".into(),
            ))
        }
    };
    let resolver = Resolver::new();
    let parts = Parts::resolve(rest, &resolver, scope);
    Ok(match target {
        Target::Curl => parts.to_curl(),
        Target::Fetch => parts.to_fetch(),
        Target::Reqwest => parts.to_reqwest(),
    })
}

/// A request with everything resolved to strings, ready to render.
struct Parts {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    /// A note for auth that can't be expressed statically.
    auth_note: Option<String>,
}

impl Parts {
    fn resolve(req: &RestRequest, resolver: &Resolver, scope: &Scope) -> Self {
        let method = resolver.resolve_sync(&req.method, scope).to_uppercase();
        let mut url = resolver.resolve_sync(&req.url, scope);
        if !req.query.is_empty() {
            let qs: Vec<String> = req
                .query
                .iter()
                .map(|(k, v)| format!("{k}={}", resolver.resolve_sync(v, scope)))
                .collect();
            let sep = if url.contains('?') { '&' } else { '?' };
            url = format!("{url}{sep}{}", qs.join("&"));
        }

        let mut headers: Vec<(String, String)> = req
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), resolver.resolve_sync(v, scope)))
            .collect();

        let mut auth_note = None;
        match &req.auth {
            None => {}
            Some(Auth::Bearer { token }) => headers.push((
                "Authorization".into(),
                format!("Bearer {}", resolver.resolve_sync(token, scope)),
            )),
            Some(Auth::Basic { username, password }) => {
                let u = resolver.resolve_sync(username, scope);
                let p = resolver.resolve_sync(password, scope);
                let encoded = STANDARD.encode(format!("{u}:{p}"));
                headers.push(("Authorization".into(), format!("Basic {encoded}")));
            }
            Some(Auth::Oauth2ClientCredentials { .. }) => {
                auth_note = Some("oauth2_client_credentials".into())
            }
            Some(Auth::AwsSigv4 { .. }) => auth_note = Some("aws_sigv4".into()),
            Some(Auth::Mtls { .. }) => auth_note = Some("mtls".into()),
        }

        let body = req.body.as_ref().map(|b| resolver.resolve_sync(b, scope));
        Parts {
            method,
            url,
            headers,
            body,
            auth_note,
        }
    }

    fn auth_comment(&self, prefix: &str) -> String {
        match &self.auth_note {
            Some(kind) => format!("{prefix} auth `{kind}` is applied at runtime (not shown here)\n"),
            None => String::new(),
        }
    }

    fn to_curl(&self) -> String {
        let mut out = self.auth_comment("#");
        out.push_str(&format!("curl -X {} '{}'", self.method, self.url));
        for (k, v) in &self.headers {
            out.push_str(&format!(" \\\n  -H '{k}: {v}'"));
        }
        if let Some(body) = &self.body {
            out.push_str(&format!(" \\\n  --data-raw '{body}'"));
        }
        out.push('\n');
        out
    }

    fn to_fetch(&self) -> String {
        let mut out = self.auth_comment("//");
        let headers = self
            .headers
            .iter()
            .map(|(k, v)| format!("    {}: {},", js_str(k), js_str(v)))
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!("await fetch({}, {{\n  method: {},\n", js_str(&self.url), js_str(&self.method)));
        if !self.headers.is_empty() {
            out.push_str(&format!("  headers: {{\n{headers}\n  }},\n"));
        }
        if let Some(body) = &self.body {
            out.push_str(&format!("  body: {},\n", js_str(body)));
        }
        out.push_str("});\n");
        out
    }

    fn to_reqwest(&self) -> String {
        let mut out = self.auth_comment("//");
        let verb = self.method.to_lowercase();
        out.push_str("let client = reqwest::Client::new();\n");
        out.push_str(&format!(
            "let res = client.{verb}({})\n",
            rust_str(&self.url)
        ));
        for (k, v) in &self.headers {
            out.push_str(&format!("    .header({}, {})\n", rust_str(k), rust_str(v)));
        }
        if let Some(body) = &self.body {
            out.push_str(&format!("    .body({})\n", rust_str(body)));
        }
        out.push_str("    .send()\n    .await?;\n");
        out
    }
}

fn js_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn rust_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use protoglot_format::parse_request_str;

    fn req(toml: &str) -> Request {
        parse_request_str(toml).unwrap()
    }

    fn scope() -> Scope {
        let mut s = Scope::new();
        s.set("baseUrl", "https://api.example.com");
        s
    }

    #[test]
    fn curl_with_bearer_and_query() {
        let r = req(r#"
            name = "Get"
            method = "get"
            url = "{{baseUrl}}/users"
            [query]
            page = "2"
            [auth]
            type = "bearer"
            token = "tok"
        "#);
        let out = generate(&r, Target::Curl, &scope()).unwrap();
        assert!(out.contains("curl -X GET 'https://api.example.com/users?page=2'"));
        assert!(out.contains("-H 'Authorization: Bearer tok'"));
    }

    #[test]
    fn fetch_shape() {
        let r = req(r#"
            name = "Post"
            method = "POST"
            url = "{{baseUrl}}/x"
            body = "hello"
        "#);
        let out = generate(&r, Target::Fetch, &scope()).unwrap();
        assert!(out.contains("await fetch('https://api.example.com/x'"));
        assert!(out.contains("method: 'POST'"));
        assert!(out.contains("body: 'hello'"));
    }

    #[test]
    fn reqwest_shape_and_secret_placeholder() {
        let r = req(r#"
            name = "Get"
            url = "{{baseUrl}}/x"
            [auth]
            type = "bearer"
            token = "{{$secret:api_token}}"
        "#);
        let out = generate(&r, Target::Reqwest, &scope()).unwrap();
        assert!(out.contains("client.get(\"https://api.example.com/x\")"));
        // secret rendered as env placeholder, never the value
        assert!(out.contains("Bearer $PROTOGLOT_SECRET_API_TOKEN"));
    }

    #[test]
    fn sigv4_noted_not_baked() {
        let r = req(r#"
            name = "Get"
            url = "{{baseUrl}}/x"
            [auth]
            type = "aws_sigv4"
            access_key_id = "AKID"
            secret_access_key = "s"
            region = "us-east-1"
            service = "execute-api"
        "#);
        let out = generate(&r, Target::Curl, &scope()).unwrap();
        assert!(out.contains("aws_sigv4"));
        assert!(!out.contains("AKID"));
    }

    #[test]
    fn non_rest_errors() {
        let r = req(r#"
            kind = "graphql"
            name = "q"
            url = "{{baseUrl}}/graphql"
            query = "{ x }"
        "#);
        assert!(generate(&r, Target::Curl, &scope()).is_err());
    }
}
