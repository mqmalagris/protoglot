//! Parsing the on-disk layout into the [`model`](crate::model) types.

use crate::model::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unknown request kind: {0}")]
    UnknownKind(String),
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A request together with the file it came from.
#[derive(Debug, Clone)]
pub struct LoadedRequest {
    pub path: PathBuf,
    pub request: Request,
}

/// Parse a single request from TOML text. `kind` defaults to `rest` when absent.
pub fn parse_request_str(s: &str) -> Result<Request, ParseError> {
    let value: toml::Value = toml::from_str(s)?;
    let kind = value.get("kind").and_then(|v| v.as_str()).map(str::to_string);
    let kind = match kind.as_deref() {
        None | Some("rest") => Kind::Rest,
        Some("graphql") => Kind::Graphql,
        Some("grpc") => Kind::Grpc,
        Some("websocket") => Kind::Websocket,
        Some("soap") => Kind::Soap,
        Some(other) => return Err(ParseError::UnknownKind(other.to_string())),
    };
    // The `kind` key is left in the value; variant structs ignore unknown fields.
    Ok(match kind {
        Kind::Rest => Request::Rest(value.try_into()?),
        Kind::Graphql => Request::Graphql(value.try_into()?),
        Kind::Grpc => Request::Grpc(value.try_into()?),
        Kind::Websocket => Request::Websocket(value.try_into()?),
        Kind::Soap => Request::Soap(value.try_into()?),
    })
}

pub fn parse_config_str(s: &str) -> Result<CollectionConfig, ParseError> {
    Ok(toml::from_str(s)?)
}

/// Parse an environment file. Values are coerced to strings (a bare number or
/// bool in TOML becomes its textual form) so templating sees uniform data.
pub fn parse_env_str(s: &str) -> Result<VarMap, ParseError> {
    let raw: BTreeMap<String, toml::Value> = toml::from_str(s)?;
    Ok(raw.into_iter().map(|(k, v)| (k, value_to_string(v))).collect())
}

fn value_to_string(v: toml::Value) -> String {
    match v {
        toml::Value::String(s) => s,
        other => other.to_string(),
    }
}

fn read(path: &Path) -> Result<String, ParseError> {
    fs::read_to_string(path).map_err(|source| ParseError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load_request(path: &Path) -> Result<Request, ParseError> {
    parse_request_str(&read(path)?)
}

pub fn load_config(path: &Path) -> Result<CollectionConfig, ParseError> {
    parse_config_str(&read(path)?)
}

pub fn load_environment(path: &Path) -> Result<VarMap, ParseError> {
    parse_env_str(&read(path)?)
}

/// Collect requests under `path` in stable (sorted) order. `path` may be a
/// single request file, a folder, or a whole collection root. The
/// `environments/` dir and the root `protoglot.toml` are skipped.
pub fn collect_requests(path: &Path) -> Result<Vec<LoadedRequest>, ParseError> {
    let mut out = Vec::new();
    if path.is_file() {
        out.push(LoadedRequest {
            path: path.to_path_buf(),
            request: load_request(path)?,
        });
    } else if path.is_dir() {
        collect_dir(path, &mut out)?;
    }
    Ok(out)
}

fn collect_dir(dir: &Path, out: &mut Vec<LoadedRequest>) -> Result<(), ParseError> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|source| ParseError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let p = entry.path();
        let name = entry.file_name();
        if p.is_dir() {
            if name.to_str() == Some("environments") {
                continue;
            }
            collect_dir(&p, out)?;
        } else if p.extension().and_then(|x| x.to_str()) == Some("toml") {
            if name.to_str() == Some("protoglot.toml") {
                continue;
            }
            out.push(LoadedRequest {
                path: p.clone(),
                request: load_request(&p)?,
            });
        }
    }
    Ok(())
}

fn search_root(start: &Path) -> PathBuf {
    if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent().unwrap_or(Path::new(".")).to_path_buf()
    }
}

/// Walk up from `start` looking for the nearest `protoglot.toml`. Returns a
/// default config if none is found.
pub fn find_config(start: &Path) -> CollectionConfig {
    let mut cur = Some(search_root(start));
    while let Some(dir) = cur {
        let candidate = dir.join("protoglot.toml");
        if candidate.is_file() {
            if let Ok(cfg) = load_config(&candidate) {
                return cfg;
            }
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    CollectionConfig::default()
}

/// Walk up from `start` looking for `environments/<name>.toml`.
pub fn find_environment(start: &Path, name: &str) -> Option<VarMap> {
    let file = format!("{name}.toml");
    let mut cur = Some(search_root(start));
    while let Some(dir) = cur {
        let candidate = dir.join("environments").join(&file);
        if candidate.is_file() {
            return load_environment(&candidate).ok();
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_kind_defaults_when_omitted() {
        let toml = r#"
            name = "Get User"
            method = "GET"
            url = "{{baseUrl}}/users/{{userId}}"
            [headers]
            Authorization = "Bearer {{token}}"
        "#;
        let req = parse_request_str(toml).unwrap();
        assert_eq!(req.kind(), Kind::Rest);
        assert_eq!(req.name(), "Get User");
        match req {
            Request::Rest(r) => {
                assert_eq!(r.method, "GET");
                assert_eq!(r.headers.get("Authorization").unwrap(), "Bearer {{token}}");
            }
            _ => panic!("expected rest"),
        }
    }

    #[test]
    fn rest_method_defaults_to_get() {
        let req = parse_request_str("name = \"x\"\nurl = \"http://e\"").unwrap();
        match req {
            Request::Rest(r) => assert_eq!(r.method, "GET"),
            _ => panic!("expected rest"),
        }
    }

    #[test]
    fn parses_assertions() {
        let toml = r#"
            name = "Get"
            url = "http://e"
            [[assertions]]
            type = "status"
            equals = 200
            [[assertions]]
            type = "jsonpath"
            path = "$.id"
            exists = true
        "#;
        let req = parse_request_str(toml).unwrap();
        let a = req.assertions();
        assert_eq!(a.len(), 2);
        assert!(matches!(a[0], Assertion::Status { equals: Some(200), .. }));
        assert!(matches!(a[1], Assertion::Jsonpath { exists: Some(true), .. }));
    }

    #[test]
    fn parses_graphql() {
        let toml = r#"
            kind = "graphql"
            name = "Fetch"
            url = "http://e/graphql"
            query = "query { me { id } }"
            [variables]
            id = "{{userId}}"
        "#;
        let req = parse_request_str(toml).unwrap();
        assert_eq!(req.kind(), Kind::Graphql);
        match req {
            Request::Graphql(g) => assert_eq!(g.variables.get("id").unwrap(), "{{userId}}"),
            _ => panic!("expected graphql"),
        }
    }

    #[test]
    fn unknown_kind_errors() {
        let err = parse_request_str("kind = \"carrier-pigeon\"\nname=\"x\"").unwrap_err();
        assert!(matches!(err, ParseError::UnknownKind(_)));
    }

    #[test]
    fn env_coerces_values_to_strings() {
        let env = parse_env_str("baseUrl = \"http://x\"\nport = 8080").unwrap();
        assert_eq!(env.get("baseUrl").unwrap(), "http://x");
        assert_eq!(env.get("port").unwrap(), "8080");
    }
}
