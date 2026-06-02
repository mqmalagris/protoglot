//! Core error type. Libraries use `thiserror`; the binary wraps with `anyhow`.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("protocol not implemented yet: {0}")]
    NotImplemented(&'static str),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid method or url: {0}")]
    Request(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("template resolution failed: {0}")]
    Template(String),

    #[error("assertion engine error: {0}")]
    Assertion(String),

    #[error("data file error: {0}")]
    Data(String),

    #[error("script error: {0}")]
    Script(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
