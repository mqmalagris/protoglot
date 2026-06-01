//! Declarative assertion engine. Evaluates [`protoglot_format::Assertion`] over
//! a [`RawResponse`]. Phase 1 types: status, jsonpath, header, response_time,
//! body_contains. (xpath arrives with SOAP in Phase 2.)

use crate::protocols::RawResponse;
use crate::report::AssertionOutcome;
use crate::xml;
use protoglot_format::{Assertion, VarMap};
use regex::Regex;
use serde_json::Value;
use serde_json_path::JsonPath;
use std::time::Duration;

pub fn evaluate(assertion: &Assertion, resp: &RawResponse, duration: Duration) -> AssertionOutcome {
    match assertion {
        Assertion::Status { equals, in_range } => eval_status(resp, *equals, *in_range),
        Assertion::Jsonpath {
            path,
            exists,
            equals,
            matches,
        } => eval_jsonpath(resp, path, *exists, equals.as_ref(), matches.as_deref()),
        Assertion::Header {
            name,
            exists,
            equals,
        } => eval_header(resp, name, *exists, equals.as_deref()),
        Assertion::ResponseTime { max_ms } => eval_response_time(duration, *max_ms),
        Assertion::BodyContains { value } => eval_body_contains(resp, value),
        Assertion::Xpath {
            path,
            exists,
            equals,
            namespaces,
        } => eval_xpath(resp, path, *exists, equals.as_deref(), namespaces),
    }
}

fn judge(desc: impl Into<String>, ok: bool, fail_msg: impl Into<String>) -> AssertionOutcome {
    if ok {
        AssertionOutcome::pass(desc)
    } else {
        AssertionOutcome::fail(desc, fail_msg)
    }
}

fn eval_status(resp: &RawResponse, equals: Option<u16>, in_range: Option<[u16; 2]>) -> AssertionOutcome {
    if let Some(want) = equals {
        return judge(
            format!("status == {want}"),
            resp.status == want,
            format!("got {}", resp.status),
        );
    }
    if let Some([lo, hi]) = in_range {
        return judge(
            format!("status in [{lo}, {hi}]"),
            resp.status >= lo && resp.status <= hi,
            format!("got {}", resp.status),
        );
    }
    AssertionOutcome::fail("status", "needs `equals` or `in_range`")
}

fn eval_jsonpath(
    resp: &RawResponse,
    path: &str,
    exists: Option<bool>,
    equals: Option<&Value>,
    matches: Option<&str>,
) -> AssertionOutcome {
    let desc = format!("jsonpath {path}");
    let json = match resp.json() {
        Some(j) => j,
        None => return AssertionOutcome::fail(desc, "response body is not valid JSON"),
    };
    let jp = match JsonPath::parse(path) {
        Ok(p) => p,
        Err(e) => return AssertionOutcome::fail(desc, format!("invalid jsonpath: {e}")),
    };
    let nodes = jp.query(&json).all();
    let first = nodes.first().copied();

    // explicit absence check
    if exists == Some(false) {
        return judge(desc, nodes.is_empty(), "expected no match");
    }
    // presence is required when exists==true, or implicitly when no other predicate is set
    if exists == Some(true) || (exists.is_none() && equals.is_none() && matches.is_none()) {
        if nodes.is_empty() {
            return AssertionOutcome::fail(desc, "expected a match, found none");
        }
    }
    if let Some(expected) = equals {
        return match first {
            Some(v) => judge(desc, v == expected, format!("expected {expected}, got {v}")),
            None => AssertionOutcome::fail(desc, "no node to compare for `equals`"),
        };
    }
    if let Some(pattern) = matches {
        let re = match Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => return AssertionOutcome::fail(desc, format!("invalid regex: {e}")),
        };
        return match first {
            Some(v) => {
                let s = json_match_str(v);
                judge(desc, re.is_match(&s), format!("`{s}` did not match /{pattern}/"))
            }
            None => AssertionOutcome::fail(desc, "no node to match"),
        };
    }
    AssertionOutcome::pass(desc)
}

fn json_match_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn eval_header(
    resp: &RawResponse,
    name: &str,
    exists: Option<bool>,
    equals: Option<&str>,
) -> AssertionOutcome {
    let desc = format!("header {name}");
    let got = resp.header(name);
    if exists == Some(false) {
        return judge(desc, got.is_none(), "expected header to be absent");
    }
    if exists == Some(true) || equals.is_none() {
        if got.is_none() {
            return AssertionOutcome::fail(desc, "expected header to be present");
        }
    }
    if let Some(expected) = equals {
        return match got {
            Some(v) => judge(desc, v == expected, format!("expected `{expected}`, got `{v}`")),
            None => AssertionOutcome::fail(desc, "header absent"),
        };
    }
    AssertionOutcome::pass(desc)
}

fn eval_response_time(duration: Duration, max_ms: u64) -> AssertionOutcome {
    let got = duration.as_millis() as u64;
    judge(
        format!("response_time <= {max_ms}ms"),
        got <= max_ms,
        format!("took {got}ms"),
    )
}

fn eval_body_contains(resp: &RawResponse, value: &str) -> AssertionOutcome {
    judge(
        format!("body contains `{value}`"),
        resp.text().contains(value),
        "substring not found in body",
    )
}

fn eval_xpath(
    resp: &RawResponse,
    path: &str,
    exists: Option<bool>,
    equals: Option<&str>,
    namespaces: &VarMap,
) -> AssertionOutcome {
    let desc = format!("xpath {path}");
    let result = match xml::eval_xpath(&resp.text(), path, namespaces) {
        Ok(r) => r,
        Err(e) => return AssertionOutcome::fail(desc, e),
    };

    if exists == Some(false) {
        return judge(desc, !result.exists, "expected no match");
    }
    if exists == Some(true) || (exists.is_none() && equals.is_none()) {
        if !result.exists {
            return AssertionOutcome::fail(desc, "expected a match, found none");
        }
    }
    if let Some(expected) = equals {
        return judge(
            desc,
            result.string == expected,
            format!("expected `{expected}`, got `{}`", result.string),
        );
    }
    AssertionOutcome::pass(desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, body: &str, ct: &str) -> RawResponse {
        RawResponse {
            status,
            headers: vec![("content-type".into(), ct.into())],
            body: body.as_bytes().to_vec(),
            content_type: Some(ct.into()),
        }
    }

    #[test]
    fn status_equals() {
        let r = resp(200, "", "text/plain");
        assert!(eval_status(&r, Some(200), None).passed);
        assert!(!eval_status(&r, Some(404), None).passed);
    }

    #[test]
    fn status_in_range() {
        let r = resp(204, "", "text/plain");
        assert!(eval_status(&r, None, Some([200, 299])).passed);
        assert!(!eval_status(&r, None, Some([300, 399])).passed);
    }

    #[test]
    fn jsonpath_exists_and_equals() {
        let r = resp(200, r#"{"id": 7, "name": "ada"}"#, "application/json");
        assert!(eval_jsonpath(&r, "$.id", Some(true), None, None).passed);
        assert!(eval_jsonpath(&r, "$.id", None, Some(&Value::from(7)), None).passed);
        assert!(!eval_jsonpath(&r, "$.id", None, Some(&Value::from(8)), None).passed);
        assert!(!eval_jsonpath(&r, "$.missing", Some(true), None, None).passed);
    }

    #[test]
    fn jsonpath_matches_regex() {
        let r = resp(200, r#"{"name": "ada"}"#, "application/json");
        assert!(eval_jsonpath(&r, "$.name", None, None, Some("^a.a$")).passed);
        assert!(!eval_jsonpath(&r, "$.name", None, None, Some("^z")).passed);
    }

    #[test]
    fn header_checks() {
        let r = resp(200, "", "application/json");
        assert!(eval_header(&r, "Content-Type", None, Some("application/json")).passed);
        assert!(eval_header(&r, "content-type", Some(true), None).passed);
        assert!(eval_header(&r, "x-missing", Some(false), None).passed);
        assert!(!eval_header(&r, "x-missing", Some(true), None).passed);
    }

    #[test]
    fn body_contains_and_time() {
        let r = resp(200, "hello world", "text/plain");
        assert!(eval_body_contains(&r, "world").passed);
        assert!(!eval_body_contains(&r, "nope").passed);
        assert!(eval_response_time(Duration::from_millis(50), 100).passed);
        assert!(!eval_response_time(Duration::from_millis(150), 100).passed);
    }

    #[test]
    fn xpath_plain_and_equals() {
        let r = resp(200, "<root><rate>3.5</rate></root>", "text/xml");
        assert!(eval_xpath(&r, "//rate", Some(true), None, &VarMap::new()).passed);
        assert!(eval_xpath(&r, "//rate", None, Some("3.5"), &VarMap::new()).passed);
        assert!(!eval_xpath(&r, "//rate", None, Some("9.9"), &VarMap::new()).passed);
        assert!(!eval_xpath(&r, "//missing", Some(true), None, &VarMap::new()).passed);
    }

    #[test]
    fn xpath_with_namespaces() {
        let r = resp(
            200,
            r#"<r:root xmlns:r="urn:x"><r:rate>3.5</r:rate></r:root>"#,
            "text/xml",
        );
        // unregistered prefix -> no match
        assert!(!eval_xpath(&r, "//x:rate", Some(true), None, &VarMap::new()).passed);
        // registered prefix -> match
        let mut ns = VarMap::new();
        ns.insert("x".into(), "urn:x".into());
        assert!(eval_xpath(&r, "//x:rate", Some(true), None, &ns).passed);
    }
}
