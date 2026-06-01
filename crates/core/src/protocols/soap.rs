//! SOAP — stub (Phase 2). HTTP POST of an XML envelope; XPath assertions added then.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::RawResponse;
use protoglot_format::SoapRequest;
use reqwest::Client;

pub async fn execute(
    _req: &SoapRequest,
    _scope: &Scope,
    _client: &Client,
    _resolver: &Resolver,
) -> Result<RawResponse> {
    Err(Error::NotImplemented("soap"))
}
