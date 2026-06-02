//! protoglot core: the domain crate. Protocols, runner, environment/templating,
//! assertion engine, and reporting. CLI and desktop are thin consumers of this
//! (the **core-first** principle, §2).

pub mod assertions;
pub mod auth;
pub mod capture;
pub mod codegen;
pub mod data;
pub mod environment;
pub mod error;
pub mod lint;
pub mod protocols;
pub mod report;
pub mod runner;
pub mod secrets;
pub mod snapshot;
pub mod xml;

pub use error::{Error, Result};
pub use report::{ExecStatus, ExecutionResult, Protocol, Reporter};
pub use runner::{RunOptions, Runner};

// Re-export the format crate so consumers get one import surface.
pub use protoglot_format as format;
