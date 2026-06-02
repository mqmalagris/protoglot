//! The runner: resolve variables → execute protocol → apply assertions →
//! `ExecutionResult`. UI/CLI-agnostic; whoever calls it formats the output.

use crate::assertions;
use crate::capture;
use crate::environment::{Resolver, Scope};
use crate::error::Result;
use crate::protocols::{self, ExecOutcome};
use crate::report::{AssertionOutcome, ExecStatus, ExecutionResult, Protocol, ResponseSummary};
use protoglot_format::{LoadedRequest, Request};
use std::path::Path;
use std::time::{Duration, Instant};

/// Default per-request timeout. Without one, a dead server hangs the whole run
/// forever — fatal for CI.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Which HTTP version to use. `Auto` negotiates via ALPN (h2 then h1.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HttpVersion {
    #[default]
    Auto,
    Http1,
    /// Force HTTP/2 (prior knowledge — skips ALPN negotiation).
    Http2,
}

/// HTTP client configuration for a run.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `None` disables the per-request timeout.
    pub timeout: Option<Duration>,
    pub http_version: HttpVersion,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
            http_version: HttpVersion::Auto,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Stop at the first non-passing request.
    pub bail: bool,
}

pub struct Runner {
    client: reqwest::Client,
    resolver: Resolver,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner {
    pub fn new() -> Self {
        Self::with_config(ClientConfig::default())
    }

    /// Build a runner whose HTTP client uses `timeout` per request. A zero
    /// duration disables the timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self::with_config(ClientConfig {
            timeout: (!timeout.is_zero()).then_some(timeout),
            ..ClientConfig::default()
        })
    }

    /// Build a runner from a full client config (timeout + HTTP version).
    pub fn with_config(config: ClientConfig) -> Self {
        let mut builder = reqwest::Client::builder();
        if let Some(timeout) = config.timeout {
            builder = builder.timeout(timeout);
        }
        builder = match config.http_version {
            HttpVersion::Auto => builder,
            HttpVersion::Http1 => builder.http1_only(),
            HttpVersion::Http2 => builder.http2_prior_knowledge(),
        };
        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            resolver: Resolver::new(),
        }
    }

    /// Run one request. `scope` is `&mut` so declarative captures (§10) can
    /// write values back for subsequent requests. `base_dir` is the request
    /// file's directory, used to resolve relative paths (e.g. schema files).
    pub async fn run_request(
        &self,
        request: &Request,
        scope: &mut Scope,
        base_dir: &Path,
    ) -> ExecutionResult {
        let protocol = Protocol::from(request.kind());
        let name = request.name().to_string();

        let started = Instant::now();
        let outcome = self.execute(request, scope).await;
        let duration = started.elapsed();

        match outcome {
            Ok(ExecOutcome {
                response,
                protocol_failure,
            }) => {
                let mut assertions: Vec<AssertionOutcome> = request
                    .assertions()
                    .iter()
                    .map(|a| assertions::evaluate(a, &response, duration, base_dir))
                    .collect();

                // Captures run after the response, writing into the shared scope
                // for subsequent requests in the same run (§10).
                capture::apply(request.captures(), &response, scope);

                // GraphQL `errors` / SOAP `Fault` flip the result to Failed.
                if let Some(msg) = protocol_failure {
                    assertions.push(AssertionOutcome::fail("protocol", msg));
                }

                let all_ok = assertions.iter().all(|a| a.passed);
                ExecutionResult {
                    request_name: name,
                    protocol,
                    status: if all_ok {
                        ExecStatus::Ok
                    } else {
                        ExecStatus::Failed
                    },
                    duration,
                    response: Some(ResponseSummary {
                        status: response.status,
                        content_type: response.content_type.clone(),
                        size_bytes: response.body.len(),
                    }),
                    assertions,
                    error: None,
                }
            }
            Err(e) => ExecutionResult {
                request_name: name,
                protocol,
                status: ExecStatus::Error,
                duration,
                response: None,
                assertions: Vec::new(),
                error: Some(e.to_string()),
            },
        }
    }

    async fn execute(&self, request: &Request, scope: &Scope) -> Result<ExecOutcome> {
        match request {
            Request::Rest(r) => {
                protocols::rest::execute(r, scope, &self.client, &self.resolver).await
            }
            Request::Graphql(r) => {
                protocols::graphql::execute(r, scope, &self.client, &self.resolver).await
            }
            Request::Soap(r) => {
                protocols::soap::execute(r, scope, &self.client, &self.resolver).await
            }
            Request::Grpc(r) => protocols::grpc::execute(r, scope, &self.resolver).await,
            Request::Websocket(r) => {
                protocols::websocket::execute(r, scope, &self.resolver).await
            }
        }
    }

    /// Run one collection item, expanding it into N executions when it declares
    /// a data source (§Phase 7); otherwise a single execution. Each data row
    /// runs against an ephemeral scope (base scope + row variables), so row
    /// values and any captures stay isolated to that iteration.
    pub async fn run_item(
        &self,
        item: &LoadedRequest,
        scope: &mut Scope,
    ) -> Vec<ExecutionResult> {
        let base_dir = item.path.parent().unwrap_or_else(|| Path::new("."));

        let Some(source) = item.request.data() else {
            return vec![self.run_request(&item.request, scope, base_dir).await];
        };

        let data_path = base_dir.join(&source.file);
        let rows = match crate::data::load_rows(&data_path, source.format.as_deref()) {
            Ok(rows) => rows,
            Err(e) => return vec![self.error_result(&item.request, e.to_string())],
        };
        if rows.is_empty() {
            return vec![self.error_result(
                &item.request,
                format!("data file {} has no rows", data_path.display()),
            )];
        }

        let mut results = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let mut row_scope = scope.clone();
            for (k, v) in row {
                row_scope.set(k.clone(), v.clone());
            }
            let mut result = self.run_request(&item.request, &mut row_scope, base_dir).await;
            result.request_name = format!("{} [row {}]", result.request_name, i + 1);
            results.push(result);
        }
        results
    }

    fn error_result(&self, request: &Request, message: String) -> ExecutionResult {
        ExecutionResult {
            request_name: request.name().to_string(),
            protocol: Protocol::from(request.kind()),
            status: ExecStatus::Error,
            duration: Duration::from_secs(0),
            response: None,
            assertions: Vec::new(),
            error: Some(message),
        }
    }

    /// Run a sequence of items in order, sharing one mutable scope.
    pub async fn run_all(
        &self,
        items: &[LoadedRequest],
        scope: &mut Scope,
        opts: &RunOptions,
    ) -> Vec<ExecutionResult> {
        let mut results = Vec::with_capacity(items.len());
        for item in items {
            let item_results = self.run_item(item, scope).await;
            let any_failed = item_results.iter().any(|r| !r.passed());
            results.extend(item_results);
            if opts.bail && any_failed {
                break;
            }
        }
        results
    }

    /// Run items concurrently, up to `concurrency` in flight, preserving order.
    /// Each item gets a clone of `base_scope`, so **captures do not propagate**
    /// between requests here — use sequential [`run_all`] when requests depend
    /// on each other (e.g. auth chaining). Data-driven rows within one item
    /// still run sequentially.
    pub async fn run_all_concurrent(
        &self,
        items: &[LoadedRequest],
        base_scope: &Scope,
        concurrency: usize,
    ) -> Vec<ExecutionResult> {
        use futures::stream::{self, StreamExt};
        let nested: Vec<Vec<ExecutionResult>> = stream::iter(items.iter())
            .map(|item| {
                let mut scope = base_scope.clone();
                async move { self.run_item(item, &mut scope).await }
            })
            .buffered(concurrency.max(1))
            .collect()
            .await;
        nested.into_iter().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protoglot_format::{Assertion, DataSource, RestRequest};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn data_driven_expands_one_run_per_row() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let dir = std::env::temp_dir().join(format!("pg-data-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ids.csv"), "id\n1\n2\n3\n").unwrap();

        let request = Request::Rest(RestRequest {
            name: "Item".into(),
            method: "GET".into(),
            url: format!("{}/item/{{{{id}}}}", server.uri()),
            headers: Default::default(),
            query: Default::default(),
            body: None,
            assertions: vec![Assertion::Status {
                equals: Some(200),
                in_range: None,
            }],
            capture: vec![],
            auth: None,
            data: Some(DataSource {
                file: "ids.csv".into(),
                format: None,
            }),
        });
        let item = LoadedRequest {
            path: dir.join("req.toml"),
            request,
        };

        let runner = Runner::new();
        let mut scope = Scope::new();
        let results = runner
            .run_all(std::slice::from_ref(&item), &mut scope, &RunOptions::default())
            .await;
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(results.len(), 3, "one execution per CSV row");
        assert!(results.iter().all(|r| r.passed()));
        assert!(results[0].request_name.contains("[row 1]"));
        assert!(results[2].request_name.contains("[row 3]"));
    }

    #[tokio::test]
    async fn missing_data_file_yields_error_result() {
        let request = Request::Rest(RestRequest {
            name: "Item".into(),
            method: "GET".into(),
            url: "http://example.invalid/{{id}}".into(),
            headers: Default::default(),
            query: Default::default(),
            body: None,
            assertions: vec![],
            capture: vec![],
            auth: None,
            data: Some(DataSource {
                file: "nope.csv".into(),
                format: None,
            }),
        });
        let item = LoadedRequest {
            path: std::env::temp_dir().join("req.toml"),
            request,
        };
        let runner = Runner::new();
        let mut scope = Scope::new();
        let results = runner.run_item(&item, &mut scope).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, ExecStatus::Error));
    }
}
