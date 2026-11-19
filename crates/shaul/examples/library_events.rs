//! Consume the typed event stream from a library embedding.
//!
//! ```console
//! cargo run -p shaul --example library_events -- https://example.com/file.bin .
//! ```

use std::path::PathBuf;

use shaul::{Config, EventKind, Link, TaskOptions, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let raw = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/file.bin".to_string());
    let destination = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);

    let (engine, _store) = default_engine(Config::default())?;
    let mut events = engine.subscribe();

    let listener = tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event.kind {
                EventKind::TaskStarted { task_id, .. } => {
                    println!("started {task_id}");
                }
                EventKind::Progress {
                    file,
                    downloaded_bytes,
                    total_bytes,
                    ..
                } => {
                    println!("{file}: {downloaded_bytes}/{:?}", total_bytes);
                }
                EventKind::TaskCompleted { bytes, .. } => {
                    println!("completed, {bytes} bytes");
                    break;
                }
                EventKind::TaskFailed { error, .. } => {
                    eprintln!("failed: {error}");
                    break;
                }
                _ => {}
            }
        }
    });

    let task = engine
        .add(Link::new(raw)?, destination, TaskOptions::default())
        .await?;
    let task = engine.run_task(task.id).await?;
    let _ = listener.await;
    println!("final state: {:?}", task.state);
    Ok(())
}
