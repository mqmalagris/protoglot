//! `protoglot` — the CLI. A thin wrapper over `protoglot-core` (§2 core-first).

use anyhow::{anyhow, bail, Context};
use clap::{Args, Parser, Subcommand, ValueEnum};
use protoglot_core::environment::Scope;
use protoglot_core::format::{self, VarMap};
use protoglot_core::report::{self, Reporter};
use protoglot_core::runner::{RunOptions, Runner};
use std::path::PathBuf;

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
    let args = match &cli.command {
        Command::Run(a) | Command::Test(a) => a,
    };

    let code = match run(args).await {
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
    };
    std::process::exit(code);
}

/// Returns `true` if any request failed or errored.
async fn run(args: &RunArgs) -> anyhow::Result<bool> {
    let config = format::find_config(&args.path);

    let env_vars: VarMap = match &args.env {
        Some(name) => format::find_environment(&args.path, name)
            .ok_or_else(|| anyhow!("environment `{name}` not found (looked for environments/{name}.toml)"))?,
        None => VarMap::new(),
    };

    let cli_vars = parse_vars(&args.vars)?;
    let mut scope = Scope::layered(&config.variables, &env_vars, &cli_vars);

    let items = format::collect_requests(&args.path)
        .with_context(|| format!("loading requests from {}", args.path.display()))?;
    if items.is_empty() {
        bail!("no requests found at {}", args.path.display());
    }

    let runner = Runner::new();
    let opts = RunOptions { bail: args.bail };
    let results = runner.run_all(&items, &mut scope, &opts).await;

    let rendered = report::render(&results, args.reporter.into());
    println!("{rendered}");

    let (_, failed, errored) = report::tally(&results);
    Ok(failed + errored > 0)
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
