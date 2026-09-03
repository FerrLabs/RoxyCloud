use anyhow::{Context, Result};
use roxycloud_client::sync::watch::{Status, watch};
use roxycloud_client::{Debounce, Engine, Remote, Report};

pub async fn once(engine: &mut Engine<Remote>) -> Result<()> {
    let report = engine.sync_once().await.context("syncing the folder")?;
    print(&report);
    if report.failures.is_empty() {
        return Ok(());
    }
    anyhow::bail!("{} paths did not sync", report.failures.len())
}

pub async fn keep_watching(engine: Engine<Remote>) -> Result<()> {
    let session = watch(engine, Debounce::default()).context("watching the folder")?;
    let mut status = session.subscribe();

    loop {
        tokio::select! {
            update = status.recv() => match update {
                Ok(update) => announce(&update),
                Err(_) => break,
            },
            result = tokio::signal::ctrl_c() => {
                result.context("waiting for an interrupt")?;
                break;
            }
        }
    }

    session.stop().await;
    Ok(())
}

fn announce(status: &Status) {
    match status {
        Status::Idle => println!("watching"),
        Status::Syncing => println!("syncing"),
        Status::Synced(report) => print(report),
        Status::Failed { reason } => println!("failed: {reason}"),
        Status::Paused => println!("paused"),
        Status::Stopped => println!("stopped"),
    }
}

fn print(report: &Report) {
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
        println!("skipped: {path}");
    }
    for failure in &report.failures {
        println!("failed: {} ({})", failure.path, failure.reason);
    }
}
