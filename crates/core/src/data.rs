//! Data-driven testing (§spec Phase 7). Load a CSV/JSON dataset into rows of
//! variables; the runner iterates a request over each row.

use crate::error::{Error, Result};
use protoglot_format::VarMap;
use serde_json::Value;
use std::path::Path;

/// Load rows from `path`. Format comes from `format` if given, else the file
/// extension. CSV uses the header row for column names; JSON must be an array
/// of objects. All values become strings (templating sees uniform data).
pub fn load_rows(path: &Path, format: Option<&str>) -> Result<Vec<VarMap>> {
    let fmt = format
        .map(|s| s.to_ascii_lowercase())
        .or_else(|| {
            path.extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
        })
        .unwrap_or_default();

    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Data(format!("reading {}: {e}", path.display())))?;

    match fmt.as_str() {
        "csv" => parse_csv(&content),
        "json" => parse_json(&content),
        other => Err(Error::Data(format!(
            "unsupported data format `{other}` (use csv or json)"
        ))),
    }
}

fn parse_csv(content: &str) -> Result<Vec<VarMap>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(content.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| Error::Data(format!("csv header: {e}")))?
        .clone();

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| Error::Data(format!("csv row: {e}")))?;
        let mut map = VarMap::new();
        for (header, value) in headers.iter().zip(record.iter()) {
            map.insert(header.to_string(), value.to_string());
        }
        rows.push(map);
    }
    Ok(rows)
}

fn parse_json(content: &str) -> Result<Vec<VarMap>> {
    let value: Value =
        serde_json::from_str(content).map_err(|e| Error::Data(format!("json: {e}")))?;
    let array = value
        .as_array()
        .ok_or_else(|| Error::Data("json data file must be an array of objects".into()))?;

    let mut rows = Vec::with_capacity(array.len());
    for item in array {
        let object = item
            .as_object()
            .ok_or_else(|| Error::Data("each json data row must be an object".into()))?;
        let mut map = VarMap::new();
        for (key, value) in object {
            map.insert(key.clone(), json_to_string(value));
        }
        rows.push(map);
    }
    Ok(rows)
}

fn json_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_rows() {
        let rows = parse_csv("id,name\n1,ada\n2,grace\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("id").unwrap(), "1");
        assert_eq!(rows[1].get("name").unwrap(), "grace");
    }

    #[test]
    fn json_rows_coerce_values() {
        let rows = parse_json(r#"[{"id": 1, "active": true}, {"id": 2, "active": false}]"#).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("id").unwrap(), "1");
        assert_eq!(rows[0].get("active").unwrap(), "true");
    }

    #[test]
    fn json_must_be_array() {
        assert!(parse_json(r#"{"id": 1}"#).is_err());
    }

    #[test]
    fn unsupported_format_errors() {
        // load_rows dispatches on format; a bogus extension errors.
        let dir = std::env::temp_dir().join(format!("pg-data-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("data.xml");
        std::fs::write(&p, "<x/>").unwrap();
        let err = load_rows(&p, None).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(err, Error::Data(_)));
    }
}
