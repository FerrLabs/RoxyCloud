mod legacy;
mod sync;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use roxycloud_client::{Engine, Remote};
use roxycloud_core::node::{NodeKind, Trashed};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "stashden", version, about = "Command-line client for Stashden")]
struct Cli {
    #[arg(long, env = "STASHDEN_URL", help = SERVER_HELP)]
    server: Option<String>,

    #[arg(long, env = "STASHDEN_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Exchange an email and password for a session token
    Login {
        email: String,
        #[arg(long, env = "STASHDEN_PASSWORD", hide_env_values = true)]
        password: Option<String>,
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
    /// List what is in the trash
    Trash,
    /// Bring something back from the trash
    Restore { id: Uuid },
    /// Delete something from the trash for good
    Purge { id: Uuid },
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
    let server = legacy::settle(cli.server.clone(), "STASHDEN_URL", "ROXYCLOUD_URL")
        .unwrap_or_else(|| DEFAULT_SERVER.to_owned());
    let connect = || {
        let token = legacy::settle(cli.token.clone(), "STASHDEN_TOKEN", "ROXYCLOUD_TOKEN");
        remote(&server, token.as_deref())
    };

    match &cli.command {
        Command::Login { email, password } => {
            let password =
                legacy::settle(password.clone(), "STASHDEN_PASSWORD", "ROXYCLOUD_PASSWORD")
                    .context("a password is needed: pass --password or set STASHDEN_PASSWORD")?;
            let (_, session) = Remote::login(&server, email, &password)
                .await
                .context("logging in")?;
            println!("{}", session.token);
        }
        Command::Ls { path } => {
            for node in connect()?.list(path).await? {
                let marker = match node.kind {
                    NodeKind::Directory => "/",
                    NodeKind::File => "",
                };
                println!("{:>12}  {}{marker}", node.size, node.name);
            }
        }
        Command::Mv { from, to } => {
            connect()?.rename(from, to).await?;
        }
        Command::Rm { path } => {
            connect()?.delete(path).await?;
        }
        Command::Trash => {
            for Trashed { node, .. } in connect()?.trash().await? {
                let deleted = node
                    .deleted_at
                    .map(|at| at.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                println!("{}  {deleted}  {}", node.id, node.name);
            }
        }
        Command::Restore { id } => {
            connect()?.restore(*id).await?;
        }
        Command::Purge { id } => {
            connect()?.purge(*id).await?;
        }
        Command::Sync { folder, watch } => {
            let mut engine =
                Engine::open(folder.as_path(), connect()?).context("reading the sync state")?;
            if *watch {
                sync::keep_watching(engine).await?;
            } else {
                sync::once(&mut engine).await?;
            }
        }
    }
    Ok(())
}

const DEFAULT_SERVER: &str = "http://localhost:3001";
const SERVER_HELP: &str = "Server to talk to [default: http://localhost:3001]";

fn remote(server: &str, token: Option<&str>) -> Result<Remote> {
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        anyhow::bail!("no session token; run `stashden login` or set STASHDEN_TOKEN");
    };
    Remote::new(server, token.to_owned()).context("building the API client")
}
