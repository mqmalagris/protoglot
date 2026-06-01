//! The runner: resolve variables → execute protocol → apply assertions →
//! `ExecutionResult`. UI/CLI-agnostic; whoever calls it formats the output.

use crate::assertions;
use crate::environment::{Resolver, Scope};
use crate::error::Result;
use crate::protocols::{self, RawResponse};
use crate::report::{AssertionOutcome, ExecStatus, ExecutionResult, Protocol, ResponseSummary};
use protoglot_format::{LoadedRequest, Request};
use std::time::Instant;

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
        Self {
            client: reqwest::Client::new(),
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
            Ok(resp) => {
                let assertions: Vec<AssertionOutcome> = request
                    .assertions()
                    .iter()
                    .map(|a| assertions::evaluate(a, &resp, duration))
                    .collect();
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
                        status: resp.status,
                        content_type: resp.content_type.clone(),
                        size_bytes: resp.body.len(),
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

    async fn execute(&self, request: &Request, scope: &Scope) -> Result<RawResponse> {
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
}
