mod sync;

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
    /// Rename a remote node, or move it under another directory
    Mv { from: String, to: String },
    /// Move a remote file to the trash
    Rm { path: String },
    /// Reconcile a local folder with the server
    Sync {
        folder: PathBuf,
        /// Keep running, syncing the folder as it changes
        #[arg(long)]
        watch: bool,
    },
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
        Command::Mv { from, to } => {
            connect(&cli)?.rename(from, to).await?;
        }
        Command::Rm { path } => {
            connect(&cli)?.delete(path).await?;
        }
        Command::Sync { folder, watch } => {
            let mut engine =
                Engine::open(folder.as_path(), connect(&cli)?).context("reading the sync state")?;
            if *watch {
                sync::keep_watching(engine).await?;
            } else {
                sync::once(&mut engine).await?;
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
