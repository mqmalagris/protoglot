//! Generate a collection from an OpenAPI 3 / Swagger 2 spec.
//!
//! Deliberately schema-light: the spec is walked as a plain `serde_json::Value`
//! (YAML accepted too), so no typed OpenAPI crate and no `$ref` resolution.
//! v1 emits method + url + a status assertion per operation — a runnable smoke
//! suite. Request bodies, auth blocks, and response-schema assertions are TODO
//! markers for the user (ponytail: `$ref` resolution is the rabbit hole).

use crate::error::{Error, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

const HTTP_METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

/// Parse an OpenAPI/Swagger spec (JSON or YAML) and return the collection
/// files to write, as (relative path, content) pairs — same shape the CLI
/// scaffold uses.
pub fn openapi(text: &str) -> Result<Vec<(PathBuf, String)>> {
    let spec: Value = serde_json::from_str(text)
        .or_else(|_| serde_yaml::from_str(text))
        .map_err(|e| Error::Import(format!("spec is neither valid JSON nor YAML: {e}")))?;

    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Import("spec has no `paths` object".into()))?;

    let title = spec
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("api");
    let base_url = base_url(&spec);

    let mut files = vec![
        (
            PathBuf::from("protoglot.toml"),
            format!("name = {}\n\n[variables]\nbaseUrl = {}\n", q(title), q(&base_url)),
        ),
        (
            PathBuf::from("environments").join("local.toml"),
            format!("baseUrl = {}\n", q(&base_url)),
        ),
    ];

    let mut used_slugs = HashSet::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else { continue };
        for method in HTTP_METHODS {
            let Some(op) = item.get(*method) else { continue };
            let file = request_file(path, method, op, &mut used_slugs);
            files.push(file);
        }
    }

    if files.len() == 2 {
        return Err(Error::Import("spec has no operations under `paths`".into()));
    }
    Ok(files)
}

fn request_file(
    path: &str,
    method: &str,
    op: &Value,
    used_slugs: &mut HashSet<String>,
) -> (PathBuf, String) {
    let op_id = op.get("operationId").and_then(Value::as_str);
    let name = op
        .get("summary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or_else(|| op_id.map(str::to_string))
        .unwrap_or_else(|| format!("{} {path}", method.to_uppercase()));

    let slug = unique_slug(
        &op_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("{method}-{path}")),
        used_slugs,
    );

    let mut out = String::new();
    out.push_str(&format!("name = {}\n", q(&name)));
    out.push_str(&format!("method = \"{}\"\n", method.to_uppercase()));
    out.push_str(&format!(
        "url = {}\n",
        q(&format!(
            "{{{{baseUrl}}}}{}",
            path.replace('{', "{{").replace('}', "}}")
        ))
    ));

    let params: Vec<&str> = path
        .split('{')
        .skip(1)
        .filter_map(|s| s.split('}').next())
        .collect();
    if !params.is_empty() {
        out.push_str(&format!(
            "# TODO: set path parameter(s) via --var or [variables]: {}\n",
            params.join(", ")
        ));
    }

    let has_body = matches!(method, "post" | "put" | "patch")
        && (op.get("requestBody").is_some() || swagger2_body_param(op));
    if has_body {
        out.push_str("# TODO: fill in the request body\nbody = \"{}\"\n\n[headers]\nContent-Type = \"application/json\"\n");
    }

    if let Some(status) = success_status(op) {
        out.push_str(&format!(
            "\n[[assertions]]\ntype = \"status\"\nequals = {status}\n"
        ));
    }

    let dir = op
        .pointer("/tags/0")
        .and_then(Value::as_str)
        .map(slugify)
        .filter(|s| !s.is_empty());
    let rel = match dir {
        Some(d) => PathBuf::from("requests").join(d).join(format!("{slug}.toml")),
        None => PathBuf::from("requests").join(format!("{slug}.toml")),
    };
    (rel, out)
}

/// First documented 2xx response status, lowest wins (200 before 201).
fn success_status(op: &Value) -> Option<u16> {
    op.get("responses")?
        .as_object()?
        .keys()
        .filter_map(|k| k.parse::<u16>().ok())
        .filter(|s| (200..300).contains(s))
        .min()
}

/// Swagger 2 puts the body in `parameters` with `in: body`.
fn swagger2_body_param(op: &Value) -> bool {
    op.get("parameters")
        .and_then(Value::as_array)
        .is_some_and(|ps| {
            ps.iter()
                .any(|p| p.get("in").and_then(Value::as_str) == Some("body"))
        })
}

/// OpenAPI 3 `servers[0].url`, or Swagger 2 `schemes[0]://host + basePath`.
fn base_url(spec: &Value) -> String {
    if let Some(url) = spec.pointer("/servers/0/url").and_then(Value::as_str) {
        return url.trim_end_matches('/').to_string();
    }
    if let Some(host) = spec.get("host").and_then(Value::as_str) {
        let scheme = spec
            .pointer("/schemes/0")
            .and_then(Value::as_str)
            .unwrap_or("https");
        let base_path = spec.get("basePath").and_then(Value::as_str).unwrap_or("");
        return format!("{scheme}://{host}{}", base_path.trim_end_matches('/'));
    }
    "http://localhost".to_string()
}

/// TOML basic strings accept JSON string escaping, so lean on serde_json.
fn q(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true; // suppress leading dash
    let mut prev_lower = false; // camelCase boundary: createPet → create-pet
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && prev_lower {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            last_dash = false;
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else if !last_dash {
            out.push('-');
            last_dash = true;
            prev_lower = false;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn unique_slug(raw: &str, used: &mut HashSet<String>) -> String {
    let base = {
        let s = slugify(raw);
        if s.is_empty() {
            "request".to_string()
        } else {
            s
        }
    };
    let mut candidate = base.clone();
    let mut n = 1;
    while !used.insert(candidate.clone()) {
        n += 1;
        candidate = format!("{base}-{n}");
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENAPI3: &str = r#"{
        "openapi": "3.0.0",
        "info": { "title": "Pet Store" },
        "servers": [{ "url": "https://api.example.com/v1/" }],
        "paths": {
            "/pets/{petId}": {
                "get": {
                    "operationId": "getPet",
                    "summary": "Get a pet",
                    "tags": ["pets"],
                    "responses": { "200": {}, "404": {} }
                }
            },
            "/pets": {
                "post": {
                    "operationId": "createPet",
                    "tags": ["pets"],
                    "requestBody": {},
                    "responses": { "201": {} }
                }
            }
        }
    }"#;

    #[test]
    fn generates_parseable_collection_from_openapi3() {
        let files = openapi(OPENAPI3).unwrap();
        assert_eq!(files.len(), 4); // config + env + 2 requests

        for (path, content) in &files {
            match path.file_name().unwrap().to_str().unwrap() {
                "protoglot.toml" => {
                    let cfg = protoglot_format::parse_config_str(content).unwrap();
                    assert_eq!(cfg.name.as_deref(), Some("Pet Store"));
                    assert_eq!(
                        cfg.variables.get("baseUrl").map(String::as_str),
                        Some("https://api.example.com/v1")
                    );
                }
                "local.toml" => {
                    protoglot_format::parse_env_str(content).unwrap();
                }
                "get-pet.toml" => {
                    assert_eq!(path.parent().unwrap(), PathBuf::from("requests").join("pets"));
                    let req = protoglot_format::parse_request_str(content).unwrap();
                    assert_eq!(req.name(), "Get a pet");
                    assert!(content.contains("url = \"{{baseUrl}}/pets/{{petId}}\""));
                    assert!(content.contains("equals = 200"));
                    assert!(content.contains("# TODO: set path parameter(s)"));
                }
                "create-pet.toml" => {
                    let req = protoglot_format::parse_request_str(content).unwrap();
                    assert_eq!(req.name(), "createPet");
                    assert!(content.contains("method = \"POST\""));
                    assert!(content.contains("body = \"{}\""));
                    assert!(content.contains("equals = 201"));
                }
                other => panic!("unexpected file: {other}"),
            }
        }
    }

    #[test]
    fn accepts_yaml_and_swagger2() {
        let yaml = r#"
swagger: "2.0"
info: { title: Legacy }
host: legacy.example.com
basePath: /api
schemes: [https]
paths:
  /things:
    get:
      responses:
        "200": {}
    post:
      parameters:
        - in: body
          name: body
      responses:
        "201": {}
"#;
        let files = openapi(yaml).unwrap();
        let (_, config) = &files[0];
        assert!(config.contains("baseUrl = \"https://legacy.example.com/api\""));
        // no operationId → slug from method + path, unique per method
        let names: Vec<_> = files
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"get-things.toml".to_string()));
        assert!(names.contains(&"post-things.toml".to_string()));
        for (_, content) in &files[2..] {
            protoglot_format::parse_request_str(content).unwrap();
        }
    }

    #[test]
    fn rejects_specs_without_operations() {
        assert!(openapi("{}").is_err());
        assert!(openapi(r#"{"paths": {}}"#).is_err());
        assert!(openapi("not: [valid").is_err());
    }
}
