//! JS scripting (§spec Phase 10/§13.3) — pre-request and post-response hooks
//! via `boa_engine` (pure-Rust JS, cross-compiles cleanly). Covers the last gap
//! with Postman: HMAC/signing in a pre-script, arbitrary checks/transforms in a
//! post-script.
//!
//! Rather than wire boa native functions + GC-traced host state, we hand the
//! scope to JS as a literal (`__vars`), run the script, and read changes back
//! via `JSON.stringify`. Scripts see a small `pg` API:
//!   pre:  `pg.get(k)`, `pg.set(k, v)`, `pg.vars`
//!   post: the above + `pg.response.{status,body,json}` and `pg.assert(name, cond)`

use crate::environment::Scope;
use crate::error::{Error, Result};
use crate::report::AssertionOutcome;
use boa_engine::{Context, Source};
use serde_json::Value;

pub struct ScriptResponse<'a> {
    pub status: u16,
    pub body: &'a str,
}

/// Run a pre-request script. Variable changes (via `pg.set`) merge back into
/// `scope` for templating the request.
pub fn run_pre(script: &str, scope: &mut Scope) -> Result<()> {
    let mut ctx = Context::default();
    eval(&mut ctx, &pre_prelude(scope))?;
    eval(&mut ctx, script)?;
    let vars = eval(&mut ctx, "JSON.stringify(__vars)")?;
    merge_vars(scope, &vars);
    Ok(())
}

/// Run a post-response script. Returns any `pg.assert(...)` outcomes; variable
/// changes merge back into `scope` for later requests.
pub fn run_post(
    script: &str,
    response: &ScriptResponse,
    scope: &mut Scope,
) -> Result<Vec<AssertionOutcome>> {
    let mut ctx = Context::default();
    eval(&mut ctx, &post_prelude(scope, response))?;
    eval(&mut ctx, script)?;
    let vars = eval(&mut ctx, "JSON.stringify(__vars)")?;
    merge_vars(scope, &vars);
    let asserts = eval(&mut ctx, "JSON.stringify(__asserts)")?;
    Ok(parse_asserts(&asserts))
}

const PG_VARS_API: &str = "var pg = { get: function(k){return __vars[k];}, \
     set: function(k,v){__vars[k] = String(v);}, vars: __vars };";

fn pre_prelude(scope: &Scope) -> String {
    let vars = serde_json::to_string(&scope.snapshot()).unwrap_or_else(|_| "{}".into());
    format!("var __vars = {vars};\n{PG_VARS_API}\n")
}

fn post_prelude(scope: &Scope, response: &ScriptResponse) -> String {
    let vars = serde_json::to_string(&scope.snapshot()).unwrap_or_else(|_| "{}".into());
    // A JSON-encoded string is also a valid JS string literal; a parsed JSON
    // body is a valid JS literal too (else `null`).
    let body = serde_json::to_string(response.body).unwrap_or_else(|_| "\"\"".into());
    let json = serde_json::from_str::<Value>(response.body)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "null".into());
    format!(
        "var __vars = {vars};\nvar __asserts = [];\n{PG_VARS_API}\n\
         pg.response = {{ status: {status}, body: {body}, json: {json} }};\n\
         pg.assert = function(name, cond){{ __asserts.push({{ name: String(name), ok: !!cond }}); }};\n",
        status = response.status
    )
}

fn eval(ctx: &mut Context, code: &str) -> Result<String> {
    let value = ctx
        .eval(Source::from_bytes(code))
        .map_err(|e| Error::Script(e.to_string()))?;
    let s = value
        .to_string(ctx)
        .map_err(|e| Error::Script(e.to_string()))?
        .to_std_string_escaped();
    Ok(s)
}

fn merge_vars(scope: &mut Scope, json: &str) {
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(json) {
        for (k, v) in map {
            scope.set(k, value_to_string(&v));
        }
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn parse_asserts(json: &str) -> Vec<AssertionOutcome> {
    let mut out = Vec::new();
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) {
        for item in items {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("script assertion")
                .to_string();
            let ok = item.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let desc = format!("script: {name}");
            if ok {
                out.push(AssertionOutcome::pass(desc));
            } else {
                out.push(AssertionOutcome::fail(desc, "assertion failed"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_script_sets_variable() {
        let mut scope = Scope::new();
        scope.set("base", "ab");
        run_pre("pg.set('derived', pg.get('base') + 'cd');", &mut scope).unwrap();
        assert_eq!(scope.get("derived"), Some("abcd"));
    }

    #[test]
    fn pre_script_can_compute() {
        let mut scope = Scope::new();
        run_pre("pg.set('sum', 2 + 3);", &mut scope).unwrap();
        assert_eq!(scope.get("sum"), Some("5"));
    }

    #[test]
    fn post_script_reads_response_and_asserts() {
        let mut scope = Scope::new();
        let resp = ScriptResponse {
            status: 200,
            body: r#"{"id": 7, "name": "ada"}"#,
        };
        let script = r#"
            pg.assert("status ok", pg.response.status === 200);
            pg.assert("has id", pg.response.json.id === 7);
            pg.set("capturedName", pg.response.json.name);
        "#;
        let outcomes = run_post(script, &resp, &mut scope).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.passed));
        assert_eq!(scope.get("capturedName"), Some("ada"));
    }

    #[test]
    fn failed_assert_is_reported() {
        let mut scope = Scope::new();
        let resp = ScriptResponse {
            status: 500,
            body: "oops",
        };
        let outcomes = run_post(
            "pg.assert('ok', pg.response.status === 200);",
            &resp,
            &mut scope,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].passed);
    }

    #[test]
    fn syntax_error_surfaces() {
        let mut scope = Scope::new();
        let err = run_pre("this is not valid js !!!", &mut scope).unwrap_err();
        assert!(matches!(err, Error::Script(_)));
    }
}
