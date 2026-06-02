// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Thin Tauri commands over `protoglot-core` (§2 core-first). No protocol logic
//! lives here — these wrappers resolve a path, call the same runner the CLI
//! uses, and hand the structured `ExecutionResult`s back to the UI.

use protoglot_core::environment::Scope;
use protoglot_core::format::{self, VarMap};
use protoglot_core::report::ExecutionResult;
use protoglot_core::runner::{RunOptions, Runner};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct RequestInfo {
    name: String,
    kind: String,
    path: String,
}

#[tauri::command]
fn list_requests(path: String) -> Result<Vec<RequestInfo>, String> {
    let items = format::collect_requests(&PathBuf::from(path)).map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .map(|item| RequestInfo {
            name: item.request.name().to_string(),
            kind: format!("{:?}", item.request.kind()).to_lowercase(),
            path: item.path.display().to_string(),
        })
        .collect())
}

#[tauri::command]
fn read_request(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn run_collection(path: String, env: Option<String>) -> Result<Vec<ExecutionResult>, String> {
    let path = PathBuf::from(path);
    let config = format::find_config(&path);
    let env_vars: VarMap = match env {
        Some(name) => format::find_environment(&path, &name)
            .ok_or_else(|| format!("environment `{name}` not found"))?,
        None => VarMap::new(),
    };
    let mut scope = Scope::layered(&config.variables, &env_vars, &VarMap::new());
    let items = format::collect_requests(&path).map_err(|e| e.to_string())?;
    let runner = Runner::new();
    Ok(runner.run_all(&items, &mut scope, &RunOptions::default()).await)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_requests,
            read_request,
            run_collection
        ])
        .run(tauri::generate_context!())
        .expect("error while running protoglot desktop");
}
