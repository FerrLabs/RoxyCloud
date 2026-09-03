use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use roxycloud_client::{Engine, Remote};
use roxycloud_core::node::NodeKind;

#[derive(Parser)]
#[command(name = "roxy", version, about = "Command-line client for RoxyCloud")]
struct Cli {
    #[arg(long, env = "ROXYCLOUD_URL", default_value = "http://localhost:3001")]
    server: String,

    #[arg(
        long,
        env = "ROXYCLOUD_TOKEN",
        hide_env_values = true,
        default_value = ""
    )]
    token: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Exchange an email and password for a session token
    Login {
        email: String,
        #[arg(long, env = "ROXYCLOUD_PASSWORD", hide_env_values = true)]
        password: String,
    },
    /// List a remote directory
    Ls {
        #[arg(default_value = "/")]
        path: String,
    },
    /// Move a remote file to the trash
    Rm { path: String },
    /// Reconcile a local folder with the server, once
    Sync { folder: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Command::Login { email, password } => {
            let (_, session) = Remote::login(&cli.server, email, password)
                .await
                .context("logging in")?;
            println!("{}", session.token);
        }
        Command::Ls { path } => {
            for node in connect(&cli)?.list(path).await? {
                let marker = match node.kind {
                    NodeKind::Directory => "/",
                    NodeKind::File => "",
                };
                println!("{:>12}  {}{marker}", node.size, node.name);
            }
        }
        Command::Rm { path } => {
            connect(&cli)?.delete(path).await?;
        }
        Command::Sync { folder } => {
            let mut engine =
                Engine::open(folder.as_path(), connect(&cli)?).context("reading the sync state")?;
            let report = engine.sync_once().await.context("syncing the folder")?;

            println!(
                "{} up, {} down, {} deleted here, {} deleted on the server",
                report.uploaded, report.downloaded, report.deleted_locally, report.deleted_remotely
            );
            for path in &report.conflicts {
                println!("conflict: {path}, both copies kept");
            }
            for path in &report.blocked {
                println!("blocked: {path} is a file on one side and a directory on the other");
            }
            for path in &report.skipped {
                println!("skipped: {}", path.display());
            }
            for failure in &report.failures {
                println!("failed: {} ({})", failure.path, failure.reason);
            }
            if !report.failures.is_empty() {
                anyhow::bail!("{} paths did not sync", report.failures.len());
            }
        }
    }
    Ok(())
}

fn connect(cli: &Cli) -> Result<Remote> {
    if cli.token.is_empty() {
        anyhow::bail!("no session token; run `roxy login` or set ROXYCLOUD_TOKEN");
    }
    Remote::new(&cli.server, cli.token.clone()).context("building the API client")
}
