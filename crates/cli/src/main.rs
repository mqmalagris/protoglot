//! `protoglot` — the CLI. A thin wrapper over `protoglot-core` (§2 core-first).

use anyhow::{anyhow, bail, Context};
use clap::{Args, Parser, Subcommand, ValueEnum};
use protoglot_core::codegen;
use protoglot_core::environment::Scope;
use protoglot_core::format::{self, VarMap};
use protoglot_core::report::{self, Reporter};
use protoglot_core::runner::{RunOptions, Runner};
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
        Command::Run(args) | Command::Test(args) => match run(args).await {
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
    };
    std::process::exit(code);
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
async fn run(args: &RunArgs) -> anyhow::Result<bool> {
    let mut scope = build_scope(&args.path, &args.env, &args.vars)?;

    let items = format::collect_requests(&args.path)
        .with_context(|| format!("loading requests from {}", args.path.display()))?;
    if items.is_empty() {
        bail!("no requests found at {}", args.path.display());
    }

    let runner = Runner::with_timeout(Duration::from_secs(args.timeout));
    let opts = RunOptions { bail: args.bail };
    let results = runner.run_all(&items, &mut scope, &opts).await;

    let rendered = report::render(&results, args.reporter.into());
    println!("{rendered}");

    let (_, failed, errored) = report::tally(&results);
    Ok(failed + errored > 0)
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
