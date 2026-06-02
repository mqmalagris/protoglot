//! `protoglot` — the CLI. A thin wrapper over `protoglot-core` (§2 core-first).

use anyhow::{anyhow, bail, Context};
use clap::{Args, Parser, Subcommand, ValueEnum};
use protoglot_core::codegen;
use protoglot_core::environment::Scope;
use protoglot_core::lint;
use protoglot_core::format::{self, VarMap};
use protoglot_core::report::{self, Reporter};
use protoglot_core::runner::{ClientConfig, HttpVersion, RunOptions, Runner};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "protoglot",
    version,
    about = "Local-first, git-friendly multiprotocol API client"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Execute a request, folder, or whole collection.
    Run(RunArgs),
    /// Run for CI: same execution, exits non-zero if anything fails.
    Test(RunArgs),
    /// Scaffold a new collection (runnable out of the box).
    New(NewArgs),
    /// Export a request as a curl / fetch / reqwest snippet.
    Codegen(CodegenArgs),
    /// Scan a collection for hardcoded credentials (secrets hygiene).
    Lint(LintArgs),
}

#[derive(Args)]
struct LintArgs {
    /// Path to a request file, folder, or collection root.
    path: PathBuf,
}

#[derive(Args)]
struct CodegenArgs {
    /// Path to a single request file.
    path: PathBuf,
    /// Snippet target.
    #[arg(long = "as", value_enum, default_value_t = TargetArg::Curl)]
    target: TargetArg,
    /// Select an environment for variable resolution.
    #[arg(long)]
    env: Option<String>,
    /// Inline variable override. Repeatable.
    #[arg(long = "var", value_name = "KEY=VALUE")]
    vars: Vec<String>,
}

#[derive(Copy, Clone, ValueEnum)]
enum TargetArg {
    Curl,
    Fetch,
    Reqwest,
}

impl From<TargetArg> for codegen::Target {
    fn from(t: TargetArg) -> Self {
        match t {
            TargetArg::Curl => codegen::Target::Curl,
            TargetArg::Fetch => codegen::Target::Fetch,
            TargetArg::Reqwest => codegen::Target::Reqwest,
        }
    }
}

#[derive(Args)]
struct NewArgs {
    /// Directory to create for the new collection.
    name: PathBuf,
    /// Overwrite scaffold files if the directory already exists.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct RunArgs {
    /// Path to a request file, a folder, or the collection root.
    path: PathBuf,
    /// Select an environment (environments/<name>.toml).
    #[arg(long)]
    env: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = ReporterArg::Pretty)]
    reporter: ReporterArg,
    /// Stop at the first failing request.
    #[arg(long)]
    bail: bool,
    /// Per-request timeout in seconds (0 disables it).
    #[arg(long, default_value_t = 30)]
    timeout: u64,
    /// Run up to N requests concurrently (captures don't propagate when > 1).
    #[arg(long, default_value_t = 1)]
    concurrency: usize,
    /// Force an HTTP version (auto negotiates h2/h1.1 via ALPN).
    #[arg(long = "http-version", value_enum, default_value_t = HttpVersionArg::Auto)]
    http_version: HttpVersionArg,
    /// Re-run automatically when a file in the collection changes.
    #[arg(long)]
    watch: bool,
    /// Overwrite snapshots with the current response instead of diffing.
    #[arg(long = "update-snapshots")]
    update_snapshots: bool,
    /// Inline variable override (highest precedence). Repeatable.
    #[arg(long = "var", value_name = "KEY=VALUE")]
    vars: Vec<String>,
}

#[derive(Copy, Clone, ValueEnum)]
enum ReporterArg {
    Pretty,
    Json,
    Junit,
    Tap,
}

#[derive(Copy, Clone, ValueEnum)]
enum HttpVersionArg {
    Auto,
    #[value(name = "1")]
    One,
    #[value(name = "2")]
    Two,
}

impl From<HttpVersionArg> for HttpVersion {
    fn from(v: HttpVersionArg) -> Self {
        match v {
            HttpVersionArg::Auto => HttpVersion::Auto,
            HttpVersionArg::One => HttpVersion::Http1,
            HttpVersionArg::Two => HttpVersion::Http2,
        }
    }
}

impl From<ReporterArg> for Reporter {
    fn from(r: ReporterArg) -> Self {
        match r {
            ReporterArg::Pretty => Reporter::Pretty,
            ReporterArg::Json => Reporter::Json,
            ReporterArg::Junit => Reporter::Junit,
            ReporterArg::Tap => Reporter::Tap,
        }
    }
}

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    let code = match &cli.command {
        Command::Run(args) | Command::Test(args) if args.watch => match watch_loop(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
        Command::Run(args) | Command::Test(args) => match execute_once(args).await {
            Ok(any_failed) => {
                if any_failed {
                    1
                } else {
                    0
                }
            }
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
        Command::New(args) => match scaffold(args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
        Command::Codegen(args) => match codegen_cmd(args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
        Command::Lint(args) => match lint_cmd(args) {
            Ok(found) => {
                if found {
                    1
                } else {
                    0
                }
            }
            Err(e) => {
                eprintln!("error: {e:#}");
                2
            }
        },
    };
    std::process::exit(code);
}

/// Returns `true` if any hygiene issue was found.
fn lint_cmd(args: &LintArgs) -> anyhow::Result<bool> {
    let mut total = 0usize;

    let items = format::collect_requests(&args.path)
        .with_context(|| format!("loading requests from {}", args.path.display()))?;
    for item in &items {
        total += report_findings(&item.path, &lint::lint_request(&item.request));
    }

    let mut env_files = Vec::new();
    find_env_files(&args.path, &mut env_files);
    env_files.sort();
    for path in &env_files {
        let vars = format::load_environment(path)
            .with_context(|| format!("loading environment {}", path.display()))?;
        total += report_findings(path, &lint::lint_env(&vars));
    }

    if total == 0 {
        println!("no secrets-hygiene issues found");
    } else {
        println!("\n{total} issue(s) found");
    }
    Ok(total > 0)
}

fn report_findings(path: &Path, findings: &[lint::Finding]) -> usize {
    if findings.is_empty() {
        return 0;
    }
    println!("{}", path.display());
    for f in findings {
        println!("  {}: {}", f.location, f.message);
    }
    findings.len()
}

fn find_env_files(path: &Path, out: &mut Vec<PathBuf>) {
    let root = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };
    collect_env_files(&root, out);
}

fn collect_env_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_env_files(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("toml")
            && p.parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                == Some("environments")
        {
            out.push(p);
        }
    }
}

fn codegen_cmd(args: &CodegenArgs) -> anyhow::Result<()> {
    if !args.path.is_file() {
        bail!("codegen expects a single request file, got {}", args.path.display());
    }
    let scope = build_scope(&args.path, &args.env, &args.vars)?;
    let request = format::load_request(&args.path)?;
    let snippet = codegen::generate(&request, args.target.into(), &scope)?;
    print!("{snippet}");
    Ok(())
}

/// Returns `true` if any request failed or errored.
async fn execute_once(args: &RunArgs) -> anyhow::Result<bool> {
    let mut scope = build_scope(&args.path, &args.env, &args.vars)?;

    let items = format::collect_requests(&args.path)
        .with_context(|| format!("loading requests from {}", args.path.display()))?;
    if items.is_empty() {
        bail!("no requests found at {}", args.path.display());
    }

    let runner = Runner::with_config(ClientConfig {
        timeout: (args.timeout != 0).then(|| Duration::from_secs(args.timeout)),
        http_version: args.http_version.into(),
    });

    let results = if args.concurrency > 1 {
        if args.bail {
            eprintln!("warning: --bail is ignored with --concurrency > 1");
        }
        if items.iter().any(|i| !i.request.captures().is_empty()) {
            eprintln!("warning: captures do not propagate across requests in parallel mode");
        }
        runner
            .run_all_concurrent(&items, &scope, args.concurrency, args.update_snapshots)
            .await
    } else {
        let opts = RunOptions {
            bail: args.bail,
            update_snapshots: args.update_snapshots,
        };
        runner.run_all(&items, &mut scope, &opts).await
    };

    let rendered = report::render(&results, args.reporter.into());
    println!("{rendered}");

    let (_, failed, errored) = report::tally(&results);
    Ok(failed + errored > 0)
}

/// Run once, then re-run whenever a file in the collection changes.
async fn watch_loop(args: &RunArgs) -> anyhow::Result<()> {
    use notify::{RecursiveMode, Watcher};

    let _ = execute_once(args).await; // first run; keep watching even on failure

    let watch_path = if args.path.is_dir() {
        args.path.clone()
    } else {
        args.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    watcher.watch(&watch_path, RecursiveMode::Recursive)?;
    eprintln!("watching {} (ctrl-c to stop)", watch_path.display());

    while let Some(res) = rx.recv().await {
        if !res.map(|e| touches_toml(&e)).unwrap_or(false) {
            continue;
        }
        // Coalesce a burst (editor saves emit several events) by waiting for a
        // quiet window with no further .toml changes before re-running once.
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let mut more = false;
            while let Ok(res) = rx.try_recv() {
                if res.map(|e| touches_toml(&e)).unwrap_or(false) {
                    more = true;
                }
            }
            if !more {
                break;
            }
        }
        eprintln!("\n── change detected, re-running ──");
        let _ = execute_once(args).await;
    }
    Ok(())
}

/// Only `.toml` changes matter — ignores editor temp files and any output the
/// run itself may write into the collection directory.
fn touches_toml(event: &notify::Event) -> bool {
    event
        .paths
        .iter()
        .any(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
}

fn build_scope(path: &Path, env: &Option<String>, vars: &[String]) -> anyhow::Result<Scope> {
    let config = format::find_config(path);
    let env_vars: VarMap = match env {
        Some(name) => format::find_environment(path, name).ok_or_else(|| {
            anyhow!("environment `{name}` not found (looked for environments/{name}.toml)")
        })?,
        None => VarMap::new(),
    };
    let cli_vars = parse_vars(vars)?;
    Ok(Scope::layered(&config.variables, &env_vars, &cli_vars))
}

fn parse_vars(raw: &[String]) -> anyhow::Result<VarMap> {
    let mut map = VarMap::new();
    for item in raw {
        let (k, v) = item
            .split_once('=')
            .ok_or_else(|| anyhow!("--var expects key=value, got `{item}`"))?;
        map.insert(k.to_string(), v.to_string());
    }
    Ok(map)
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .init();
}

const ENV_LOCAL: &str = "baseUrl = \"https://jsonplaceholder.typicode.com\"\n";

const REQUEST_EXAMPLE: &str = r#"name = "Get example"
method = "GET"
url = "{{baseUrl}}/todos/1"

[[assertions]]
type = "status"
equals = 200

[[assertions]]
type = "jsonpath"
path = "$.title"
exists = true
"#;

fn root_toml(name: &str) -> String {
    format!("name = \"{name}\"\n\n[variables]\nbaseUrl = \"https://jsonplaceholder.typicode.com\"\n")
}

/// The (relative path, content) pairs a new collection is made of.
fn scaffold_files(name: &str) -> Vec<(PathBuf, String)> {
    vec![
        (PathBuf::from("protoglot.toml"), root_toml(name)),
        (
            PathBuf::from("environments").join("local.toml"),
            ENV_LOCAL.to_string(),
        ),
        (
            PathBuf::from("requests").join("get-example.toml"),
            REQUEST_EXAMPLE.to_string(),
        ),
    ]
}

fn scaffold(args: &NewArgs) -> anyhow::Result<()> {
    let root = &args.name;
    if root.exists() {
        let non_empty = fs::read_dir(root)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        if non_empty && !args.force {
            bail!(
                "{} already exists and is not empty (use --force to overwrite)",
                root.display()
            );
        }
    }

    let collection_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("collection");

    for (rel, content) in scaffold_files(collection_name) {
        let target = root.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if target.exists() && !args.force {
            continue;
        }
        fs::write(&target, content)
            .with_context(|| format!("writing {}", target.display()))?;
        println!("  created {}", display_rel(root, &target));
    }

    println!("\nScaffolded collection at {}", root.display());
    println!("Try it:\n  protoglot test {}", root.display());
    Ok(())
}

fn display_rel(root: &Path, target: &Path) -> String {
    target
        .strip_prefix(root.parent().unwrap_or(Path::new("")))
        .unwrap_or(target)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_produces_parseable_files() {
        let files = scaffold_files("demo");
        assert_eq!(files.len(), 3);

        for (path, content) in &files {
            let name = path.file_name().unwrap().to_str().unwrap();
            match name {
                "protoglot.toml" => {
                    let cfg = protoglot_core::format::parse_config_str(content).unwrap();
                    assert_eq!(cfg.name.as_deref(), Some("demo"));
                }
                "local.toml" => {
                    let env = protoglot_core::format::parse_env_str(content).unwrap();
                    assert!(env.contains_key("baseUrl"));
                }
                "get-example.toml" => {
                    let req = protoglot_core::format::parse_request_str(content).unwrap();
                    assert_eq!(req.name(), "Get example");
                }
                other => panic!("unexpected scaffold file: {other}"),
            }
        }
    }
}
