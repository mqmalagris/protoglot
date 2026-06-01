//! protoglot collection format.
//!
//! Pure parse/serialize of the on-disk collection layout (one TOML file per
//! request, in a directory tree). Carries **no execution runtime** so it can be
//! reused by external tools without dragging in `reqwest`/`tokio`.

mod model;
mod parse;

pub use model::*;
pub use parse::*;
