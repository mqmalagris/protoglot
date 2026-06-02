//! Variable scope + `{{...}}` templating.
//!
//! Decision §13.1: a custom (regex) resolver, not `minijinja`. Secret lookup is
//! async (keychain) and the real needs are interpolation + dynamic generators +
//! a precedence chain — not template logic.

use crate::error::Result;
use crate::secrets;
use protoglot_format::VarMap;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The merged variable scope for one run. Mutable so later phases can write
/// captured values back for subsequent requests.
#[derive(Debug, Default, Clone)]
pub struct Scope {
    vars: HashMap<String, String>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a scope with the precedence chain **cli > environment > collection**
    /// (§13.1). Later layers override earlier ones.
    pub fn layered(collection: &VarMap, environment: &VarMap, cli: &VarMap) -> Self {
        let mut vars = HashMap::new();
        for (k, v) in collection {
            vars.insert(k.clone(), v.clone());
        }
        for (k, v) in environment {
            vars.insert(k.clone(), v.clone());
        }
        for (k, v) in cli {
            vars.insert(k.clone(), v.clone());
        }
        Self { vars }
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }
}

fn template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").expect("valid template regex"))
}

/// Resolves `{{...}}` templates against a [`Scope`]. Stateless; cheap to clone.
#[derive(Debug, Default, Clone, Copy)]
pub struct Resolver;

impl Resolver {
    pub fn new() -> Self {
        Self
    }

    /// Resolve every `{{token}}` in `template`. Async because secret resolution
    /// hits the keychain.
    pub async fn resolve(&self, template: &str, scope: &Scope) -> Result<String> {
        let re = template_re();
        let mut out = String::with_capacity(template.len());
        let mut last = 0;
        for caps in re.captures_iter(template) {
            let whole = caps.get(0).unwrap();
            out.push_str(&template[last..whole.start()]);
            let token = caps.get(1).unwrap().as_str().trim();
            out.push_str(&self.resolve_token(token, scope).await?);
            last = whole.end();
        }
        out.push_str(&template[last..]);
        Ok(out)
    }

    /// Synchronous resolution for previews/codegen. Resolves plain vars and
    /// dynamic values; a `{{$secret:NAME}}` becomes a shell-style
    /// `$PROTOGLOT_SECRET_NAME` placeholder so the secret value is never baked
    /// into generated output. Unknown vars are left as the literal `{{var}}`.
    pub fn resolve_sync(&self, template: &str, scope: &Scope) -> String {
        template_re()
            .replace_all(template, |caps: &regex::Captures| {
                let token = caps.get(1).unwrap().as_str().trim();
                if let Some(name) = token.strip_prefix("$secret:") {
                    let key = name.trim().to_uppercase().replace('-', "_");
                    format!("$PROTOGLOT_SECRET_{key}")
                } else if token == "$uuid" {
                    uuid::Uuid::new_v4().to_string()
                } else if token == "$timestamp" {
                    chrono::Utc::now().timestamp().to_string()
                } else {
                    match scope.get(token) {
                        Some(v) => v.to_string(),
                        None => format!("{{{{{token}}}}}"),
                    }
                }
            })
            .into_owned()
    }

    async fn resolve_token(&self, token: &str, scope: &Scope) -> Result<String> {
        if let Some(name) = token.strip_prefix("$secret:") {
            return secrets::resolve_secret(name.trim()).await;
        }
        match token {
            "$uuid" => Ok(uuid::Uuid::new_v4().to_string()),
            "$timestamp" => Ok(chrono::Utc::now().timestamp().to_string()),
            name => match scope.get(name) {
                Some(v) => Ok(v.to_string()),
                None => {
                    // Leave the literal in place rather than fail the whole run;
                    // surfaces in the response/log as an obvious sentinel.
                    tracing::warn!(variable = name, "unresolved template variable");
                    Ok(format!("{{{{{name}}}}}"))
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_with(pairs: &[(&str, &str)]) -> Scope {
        let mut s = Scope::new();
        for (k, v) in pairs {
            s.set(*k, *v);
        }
        s
    }

    #[tokio::test]
    async fn resolves_simple_vars() {
        let s = scope_with(&[("baseUrl", "http://api"), ("userId", "42")]);
        let out = Resolver::new()
            .resolve("{{baseUrl}}/users/{{userId}}", &s)
            .await
            .unwrap();
        assert_eq!(out, "http://api/users/42");
    }

    #[tokio::test]
    async fn whitespace_inside_braces_is_trimmed() {
        let s = scope_with(&[("x", "y")]);
        assert_eq!(Resolver::new().resolve("{{  x  }}", &s).await.unwrap(), "y");
    }

    #[tokio::test]
    async fn unknown_var_left_as_literal() {
        let s = Scope::new();
        assert_eq!(
            Resolver::new().resolve("a/{{missing}}/b", &s).await.unwrap(),
            "a/{{missing}}/b"
        );
    }

    #[tokio::test]
    async fn dynamic_uuid_and_timestamp() {
        let s = Scope::new();
        let u = Resolver::new().resolve("{{$uuid}}", &s).await.unwrap();
        assert_eq!(u.len(), 36, "uuid v4 hyphenated form");
        let t = Resolver::new().resolve("{{$timestamp}}", &s).await.unwrap();
        assert!(t.parse::<i64>().unwrap() > 0);
    }

    #[tokio::test]
    async fn precedence_cli_over_env_over_collection() {
        let collection = [("k".to_string(), "c".to_string())].into_iter().collect();
        let env = [("k".to_string(), "e".to_string())].into_iter().collect();
        let cli = [("k".to_string(), "cli".to_string())].into_iter().collect();
        let s = Scope::layered(&collection, &env, &cli);
        assert_eq!(s.get("k"), Some("cli"));
    }

    #[tokio::test]
    async fn secret_resolves_from_env_var() {
        // Use a fixed name; safe in single-threaded test context.
        std::env::set_var("PROTOGLOT_SECRET_DEMO_TOKEN", "s3cr3t");
        let s = Scope::new();
        let out = Resolver::new()
            .resolve("Bearer {{$secret:demo_token}}", &s)
            .await
            .unwrap();
        assert_eq!(out, "Bearer s3cr3t");
        std::env::remove_var("PROTOGLOT_SECRET_DEMO_TOKEN");
    }
}
