//! Declarative capture (§10): after a response, pull a value out (jsonpath or
//! xpath) and write it into the run scope so later requests can reference it.
//! Covers the auth-chaining use case (login → grab `$.token` → reuse) without a
//! JS engine.

use crate::environment::Scope;
use crate::protocols::RawResponse;
use crate::xml;
use protoglot_format::{Capture, VarMap};
use serde_json::Value;
use serde_json_path::JsonPath;

/// Apply every capture against `response`, writing results into `scope`. A
/// capture that produces no value is logged and skipped (it does not fail the
/// request).
pub fn apply(captures: &[Capture], response: &RawResponse, scope: &mut Scope) {
    for capture in captures {
        let value = extract(capture, response);
        match value {
            Some(v) => scope.set(capture.var.clone(), v),
            None => tracing::warn!(var = %capture.var, "capture produced no value"),
        }
    }
}

fn extract(capture: &Capture, response: &RawResponse) -> Option<String> {
    if let Some(path) = &capture.jsonpath {
        return extract_jsonpath(response, path);
    }
    if let Some(path) = &capture.xpath {
        return extract_xpath(response, path);
    }
    tracing::warn!(var = %capture.var, "capture has neither jsonpath nor xpath");
    None
}

fn extract_jsonpath(response: &RawResponse, path: &str) -> Option<String> {
    let json = response.json()?;
    let jp = JsonPath::parse(path).ok()?;
    let node = jp.query(&json).all().into_iter().next()?;
    Some(match node {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn extract_xpath(response: &RawResponse, expr: &str) -> Option<String> {
    xml::eval_xpath(&response.text(), expr, &VarMap::new())
        .ok()
        .filter(|r| r.exists)
        .map(|r| r.string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(body: &str) -> RawResponse {
        RawResponse {
            status: 200,
            headers: vec![],
            body: body.as_bytes().to_vec(),
            content_type: None,
        }
    }

    #[test]
    fn captures_jsonpath_into_scope() {
        let captures = vec![Capture {
            var: "authToken".into(),
            jsonpath: Some("$.token".into()),
            xpath: None,
        }];
        let mut scope = Scope::new();
        apply(&captures, &resp(r#"{"token": "abc123"}"#), &mut scope);
        assert_eq!(scope.get("authToken"), Some("abc123"));
    }

    #[test]
    fn captures_xpath_attribute_into_scope() {
        let captures = vec![Capture {
            var: "rateId".into(),
            jsonpath: None,
            xpath: Some("//GetRateResult/@id".into()),
        }];
        let mut scope = Scope::new();
        apply(
            &captures,
            &resp(r#"<root><GetRateResult id="99">3.5</GetRateResult></root>"#),
            &mut scope,
        );
        assert_eq!(scope.get("rateId"), Some("99"));
    }

    #[test]
    fn missing_value_is_skipped_not_set() {
        let captures = vec![Capture {
            var: "nope".into(),
            jsonpath: Some("$.absent".into()),
            xpath: None,
        }];
        let mut scope = Scope::new();
        apply(&captures, &resp(r#"{"token": "x"}"#), &mut scope);
        assert_eq!(scope.get("nope"), None);
    }
}
