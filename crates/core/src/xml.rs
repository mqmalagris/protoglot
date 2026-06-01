//! XPath evaluation over XML, used by SOAP Fault detection, the `xpath`
//! assertion, and `xpath` captures. Backed by `sxd-document` + `sxd-xpath`
//! (XPath 1.0, pure Rust). Namespace prefixes must be registered explicitly —
//! without that, queries against namespaced elements silently match nothing.

use protoglot_format::VarMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;
use sxd_document::parser;
use sxd_xpath::{Context, Factory, Value};

/// Serializes the panic-hook swap below so a concurrent xpath eval doesn't
/// observe the temporarily-silenced hook.
static HOOK_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone)]
pub struct XPathEval {
    /// True if the expression selected a non-empty node-set (or a truthy
    /// non-node-set value).
    pub exists: bool,
    /// The XPath string-value of the result.
    pub string: String,
}

pub fn eval_xpath(xml: &str, expr: &str, namespaces: &VarMap) -> Result<XPathEval, String> {
    let package = parser::parse(xml).map_err(|e| format!("xml parse error: {e:?}"))?;
    let document = package.as_document();

    let factory = Factory::new();
    let xpath = factory
        .build(expr)
        .map_err(|e| format!("invalid xpath: {e:?}"))?
        .ok_or_else(|| "empty xpath expression".to_string())?;

    let mut context = Context::new();
    for (prefix, uri) in namespaces {
        context.set_namespace(prefix, uri);
    }

    // sxd-xpath *panics* (rather than erroring) when an expression uses a
    // namespace prefix that wasn't registered. Catch it and return a clean
    // error, silencing the default panic hook for just this window.
    let evaluated = {
        let _guard = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let result =
            panic::catch_unwind(AssertUnwindSafe(|| xpath.evaluate(&context, document.root())));
        panic::set_hook(prev);
        result
    };
    let value = match evaluated {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(format!("xpath evaluation error: {e:?}")),
        Err(_) => {
            return Err(format!(
                "xpath `{expr}` references an unregistered namespace prefix"
            ))
        }
    };

    let exists = match &value {
        Value::Nodeset(ns) => ns.size() > 0,
        Value::Boolean(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => *n != 0.0 && !n.is_nan(),
    };
    Ok(XPathEval {
        exists,
        string: value.string(),
    })
}

/// True if the document contains a SOAP `Fault` element under any namespace
/// prefix (SOAP 1.1 `soap:`/`env:` or SOAP 1.2). Uses `local-name()` so it is
/// prefix-agnostic. Non-XML bodies are treated as "no fault".
pub fn has_soap_fault(xml: &str) -> bool {
    eval_xpath(xml, "//*[local-name()='Fault']", &VarMap::new())
        .map(|r| r.exists)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS_XML: &str = r#"<r:root xmlns:r="urn:rates"><r:rate id="42">3.5</r:rate></r:root>"#;
    const PLAIN_XML: &str = r#"<root><rate id="42">3.5</rate></root>"#;

    #[test]
    fn plain_xpath_exists_and_value() {
        let r = eval_xpath(PLAIN_XML, "//rate", &VarMap::new()).unwrap();
        assert!(r.exists);
        assert_eq!(r.string, "3.5");
    }

    #[test]
    fn attribute_value() {
        let r = eval_xpath(PLAIN_XML, "//rate/@id", &VarMap::new()).unwrap();
        assert_eq!(r.string, "42");
    }

    #[test]
    fn namespaced_query_needs_registration() {
        // Without registering the prefix, a namespaced query matches nothing.
        let without = eval_xpath(NS_XML, "//x:rate", &VarMap::new());
        // unknown prefix is an evaluation error OR an empty match, depending on
        // the engine — either way it must not "exist".
        assert!(without.map(|r| !r.exists).unwrap_or(true));

        let mut ns = VarMap::new();
        ns.insert("x".into(), "urn:rates".into());
        let with = eval_xpath(NS_XML, "//x:rate", &ns).unwrap();
        assert!(with.exists);
        assert_eq!(with.string, "3.5");
    }

    #[test]
    fn fault_detection() {
        assert!(has_soap_fault(
            r#"<env:Envelope xmlns:env="x"><env:Body><env:Fault/></env:Body></env:Envelope>"#
        ));
        assert!(!has_soap_fault(r#"<Envelope><Body><Ok/></Body></Envelope>"#));
        assert!(!has_soap_fault("not xml at all"));
    }
}
