//! The runner: resolve variables → execute protocol → apply assertions →
//! `ExecutionResult`. UI/CLI-agnostic; whoever calls it formats the output.

use crate::assertions;
use crate::capture;
use crate::environment::{Resolver, Scope};
use crate::error::Result;
use crate::protocols::{self, ExecOutcome};
use crate::report::{AssertionOutcome, ExecStatus, ExecutionResult, Protocol, ResponseSummary};
use protoglot_format::{LoadedRequest, Request};
use std::time::{Duration, Instant};

/// Default per-request timeout. Without one, a dead server hangs the whole run
/// forever — fatal for CI.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

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
        Self::with_timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
    }

    /// Build a runner whose HTTP client uses `timeout` per request. A zero
    /// duration disables the timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        let mut builder = reqwest::Client::builder();
        if !timeout.is_zero() {
            builder = builder.timeout(timeout);
        }
        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            resolver: Resolver::new(),
        }
    }

    /// Run one request. `scope` is `&mut` so future declarative captures (§10,
    /// Phase 2) can write values back for subsequent requests.
    pub async fn run_request(&self, request: &Request, scope: &mut Scope) -> ExecutionResult {
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
                    .map(|a| assertions::evaluate(a, &response, duration))
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

    /// Run a sequence of requests in order, sharing one mutable scope.
    pub async fn run_all(
        &self,
        items: &[LoadedRequest],
        scope: &mut Scope,
        opts: &RunOptions,
    ) -> Vec<ExecutionResult> {
        let mut results = Vec::with_capacity(items.len());
        for item in items {
            let result = self.run_request(&item.request, scope).await;
            let stop = opts.bail && !result.passed();
            results.push(result);
            if stop {
                break;
            }
        }
        results
    }

    /// Run requests concurrently, up to `concurrency` in flight, preserving
    /// result order. Each request gets a clone of `base_scope`, so **captures
    /// do not propagate** between requests here — use sequential [`run_all`]
    /// when requests depend on each other (e.g. auth chaining).
    pub async fn run_all_concurrent(
        &self,
        items: &[LoadedRequest],
        base_scope: &Scope,
        concurrency: usize,
    ) -> Vec<ExecutionResult> {
        use futures::stream::{self, StreamExt};
        stream::iter(items.iter())
            .map(|item| {
                let mut scope = base_scope.clone();
                async move { self.run_request(&item.request, &mut scope).await }
            })
            .buffered(concurrency.max(1))
            .collect()
            .await
    }
}
