//! Execution result types + reporters. The runner returns structured
//! `ExecutionResult`s; this module turns them into `pretty`/`json`/`junit`/`tap`.

use owo_colors::OwoColorize;
use serde::Serialize;
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Rest,
    Graphql,
    Grpc,
    Websocket,
    Soap,
}

impl From<protoglot_format::Kind> for Protocol {
    fn from(k: protoglot_format::Kind) -> Self {
        use protoglot_format::Kind;
        match k {
            Kind::Rest => Protocol::Rest,
            Kind::Graphql => Protocol::Graphql,
            Kind::Grpc => Protocol::Grpc,
            Kind::Websocket => Protocol::Websocket,
            Kind::Soap => Protocol::Soap,
        }
    }
}

impl Protocol {
    fn as_str(self) -> &'static str {
        match self {
            Protocol::Rest => "rest",
            Protocol::Graphql => "graphql",
            Protocol::Grpc => "grpc",
            Protocol::Websocket => "websocket",
            Protocol::Soap => "soap",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecStatus {
    /// Executed and all assertions passed.
    Ok,
    /// Executed but at least one assertion failed.
    Failed,
    /// Could not execute (transport/protocol error).
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssertionOutcome {
    pub description: String,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl AssertionOutcome {
    pub fn pass(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            passed: true,
            message: None,
        }
    }
    pub fn fail(description: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            passed: false,
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseSummary {
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionResult {
    pub request_name: String,
    pub protocol: Protocol,
    pub status: ExecStatus,
    #[serde(rename = "duration_ms", serialize_with = "ser_dur_ms")]
    pub duration: Duration,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseSummary>,
    pub assertions: Vec<AssertionOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ExecutionResult {
    pub fn passed(&self) -> bool {
        self.status == ExecStatus::Ok
    }
}

fn ser_dur_ms<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_millis() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reporter {
    Pretty,
    Json,
    Junit,
    Tap,
}

/// Counts: (ok, failed, errored).
pub fn tally(results: &[ExecutionResult]) -> (usize, usize, usize) {
    let mut ok = 0;
    let mut failed = 0;
    let mut errored = 0;
    for r in results {
        match r.status {
            ExecStatus::Ok => ok += 1,
            ExecStatus::Failed => failed += 1,
            ExecStatus::Error => errored += 1,
        }
    }
    (ok, failed, errored)
}

pub fn render(results: &[ExecutionResult], reporter: Reporter) -> String {
    match reporter {
        Reporter::Pretty => render_pretty(results),
        Reporter::Json => render_json(results),
        Reporter::Junit => render_junit(results),
        Reporter::Tap => render_tap(results),
    }
}

fn render_pretty(results: &[ExecutionResult]) -> String {
    let mut out = String::new();
    for r in results {
        let head = match r.status {
            ExecStatus::Ok => format!("{}", "✓".green()),
            ExecStatus::Failed => format!("{}", "✗".red()),
            ExecStatus::Error => format!("{}", "!".yellow()),
        };
        let code = r
            .response
            .as_ref()
            .map(|s| format!(" {}", s.status))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{head} {} [{}]{code} ({}ms)",
            r.request_name.bold(),
            r.protocol.as_str().dimmed(),
            r.duration.as_millis()
        );
        for a in &r.assertions {
            let mark = if a.passed {
                format!("{}", "✓".green())
            } else {
                format!("{}", "✗".red())
            };
            let detail = a
                .message
                .as_ref()
                .map(|m| format!(" — {m}"))
                .unwrap_or_default();
            let _ = writeln!(out, "    {mark} {}{}", a.description, detail.dimmed());
        }
        if let Some(err) = &r.error {
            let _ = writeln!(out, "    {} {err}", "error:".red());
        }
    }
    let (ok, failed, errored) = tally(results);
    let _ = writeln!(
        out,
        "\n{ok} passed, {failed} failed, {errored} errored"
    );
    out
}

fn render_json(results: &[ExecutionResult]) -> String {
    serde_json::to_string_pretty(results).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

fn render_junit(results: &[ExecutionResult]) -> String {
    let (ok, failed, errored) = tally(results);
    let total = ok + failed + errored;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<testsuites tests=\"{total}\" failures=\"{failed}\" errors=\"{errored}\">"
    );
    let _ = writeln!(
        out,
        "  <testsuite name=\"protoglot\" tests=\"{total}\" failures=\"{failed}\" errors=\"{errored}\">"
    );
    for r in results {
        let time = r.duration.as_secs_f64();
        let _ = write!(
            out,
            "    <testcase name=\"{}\" classname=\"{}\" time=\"{time:.3}\"",
            xml_escape(&r.request_name),
            r.protocol.as_str()
        );
        match r.status {
            ExecStatus::Ok => {
                out.push_str("/>\n");
            }
            ExecStatus::Failed => {
                let msg = failed_assertions_msg(r);
                let _ = writeln!(out, ">\n      <failure message=\"{}\"/>", xml_escape(&msg));
                out.push_str("    </testcase>\n");
            }
            ExecStatus::Error => {
                let msg = r.error.clone().unwrap_or_else(|| "execution error".into());
                let _ = writeln!(out, ">\n      <error message=\"{}\"/>", xml_escape(&msg));
                out.push_str("    </testcase>\n");
            }
        }
    }
    out.push_str("  </testsuite>\n</testsuites>\n");
    out
}

fn render_tap(results: &[ExecutionResult]) -> String {
    let mut out = String::from("TAP version 13\n");
    let _ = writeln!(out, "1..{}", results.len());
    for (i, r) in results.iter().enumerate() {
        let n = i + 1;
        match r.status {
            ExecStatus::Ok => {
                let _ = writeln!(out, "ok {n} - {}", r.request_name);
            }
            ExecStatus::Failed => {
                let _ = writeln!(out, "not ok {n} - {}", r.request_name);
                let _ = writeln!(out, "# {}", failed_assertions_msg(r));
            }
            ExecStatus::Error => {
                let _ = writeln!(out, "not ok {n} - {}", r.request_name);
                let _ = writeln!(
                    out,
                    "# error: {}",
                    r.error.clone().unwrap_or_else(|| "execution error".into())
                );
            }
        }
    }
    out
}

fn failed_assertions_msg(r: &ExecutionResult) -> String {
    r.assertions
        .iter()
        .filter(|a| !a.passed)
        .map(|a| match &a.message {
            Some(m) => format!("{}: {m}", a.description),
            None => a.description.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<ExecutionResult> {
        vec![
            ExecutionResult {
                request_name: "Get User".into(),
                protocol: Protocol::Rest,
                status: ExecStatus::Ok,
                duration: Duration::from_millis(12),
                response: Some(ResponseSummary {
                    status: 200,
                    content_type: Some("application/json".into()),
                    size_bytes: 42,
                }),
                assertions: vec![AssertionOutcome::pass("status == 200")],
                error: None,
            },
            ExecutionResult {
                request_name: "Bad <One>".into(),
                protocol: Protocol::Rest,
                status: ExecStatus::Failed,
                duration: Duration::from_millis(5),
                response: Some(ResponseSummary {
                    status: 500,
                    content_type: None,
                    size_bytes: 0,
                }),
                assertions: vec![AssertionOutcome::fail("status == 200", "got 500")],
                error: None,
            },
        ]
    }

    #[test]
    fn junit_escapes_and_counts() {
        let xml = render_junit(&sample());
        assert!(xml.contains("tests=\"2\" failures=\"1\" errors=\"0\""));
        assert!(xml.contains("Bad &lt;One&gt;"));
        assert!(xml.contains("<failure"));
    }

    #[test]
    fn tap_plan_and_lines() {
        let tap = render_tap(&sample());
        assert!(tap.starts_with("TAP version 13\n1..2\n"));
        assert!(tap.contains("ok 1 - Get User"));
        assert!(tap.contains("not ok 2 - Bad <One>"));
    }

    #[test]
    fn json_is_valid() {
        let json = render_json(&sample());
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["duration_ms"], 12);
        assert_eq!(v[0]["status"], "ok");
    }
}
