//! Secret resolution.
//!
//! Phase 1 stub: secrets resolve from the environment variable
//! `PROTOGLOT_SECRET_<NAME>` (name upper-cased, `-` -> `_`). Phase 3 swaps this
//! for the OS keychain via the `keyring` crate. Resolution is intentionally
//! `async` so the real keychain backend slots in without changing call sites.

use crate::error::{Error, Result};

pub async fn resolve_secret(name: &str) -> Result<String> {
    let env_key = format!("PROTOGLOT_SECRET_{}", name.to_uppercase().replace('-', "_"));
    std::env::var(&env_key).map_err(|_| {
        Error::Template(format!(
            "secret `{name}` not available (set {env_key}; keychain backend lands in Phase 3)"
        ))
    })
}
