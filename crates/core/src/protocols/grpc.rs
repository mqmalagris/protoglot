//! gRPC — stub (Phase 6, the boss final). Dynamic invocation via reflection,
//! then `.proto`/`protox`, with a custom `prost-reflect` codec.

use crate::environment::{Resolver, Scope};
use crate::error::{Error, Result};
use crate::protocols::ExecOutcome;
use protoglot_format::GrpcRequest;

pub async fn execute(
    _req: &GrpcRequest,
    _scope: &Scope,
    _resolver: &Resolver,
) -> Result<ExecOutcome> {
    Err(Error::NotImplemented("grpc"))
}
