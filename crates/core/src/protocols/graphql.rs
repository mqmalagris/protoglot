//! GraphQL — stub (Phase 2). Reuses the REST/HTTP layer once implemented.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::RawResponse;
use protoglot_format::GraphqlRequest;
use reqwest::Client;

pub async fn execute(
    _req: &GraphqlRequest,
    _scope: &Scope,
    _client: &Client,
    _resolver: &Resolver,
) -> Result<RawResponse> {
    Err(Error::NotImplemented("graphql"))
}
