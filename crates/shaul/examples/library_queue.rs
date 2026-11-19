//! Drive the persistent queue from library code: add, pause/resume and run.
//!
//! ```console
//! cargo run -p shaul --example library_queue -- https://example.com/a.bin https://example.com/b.bin
//! ```

use shaul::{CancellationToken, Config, Link, TaskOptions, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let links: Vec<String> = std::env::args().skip(1).collect();
    if links.is_empty() {
        eprintln!("usage: library_queue URL [URL ...]");
        return Ok(());
    }
    let destination = std::env::current_dir()?;

    let (engine, _store) = default_engine(Config::default())?;
    engine.requeue_stale().await?;

    let mut ids = Vec::new();
    for raw in links {
        let task = engine
            .add(Link::new(raw)?, destination.clone(), TaskOptions::default())
            .await?;
        println!("queued {} ({})", task.id, task.link);
        ids.push(task.id);
    }

    // Pause the second task to show that run_pending only picks Pending work.
    if let Some(second) = ids.get(1) {
        let paused = engine.pause(*second).await?;
        println!("paused {}: {:?}", paused.id, paused.state);
    }

    let done = engine.run_pending(1, CancellationToken::new()).await?;
    println!("completed {} task(s) in the first batch", done.len());

    if let Some(second) = ids.get(1) {
        let resumed = engine.resume(*second).await?;
        println!("resumed {}: {:?}", resumed.id, resumed.state);
        let done = engine.run_pending(1, CancellationToken::new()).await?;
        println!("completed {} task(s) in the second batch", done.len());
    }

    for task in engine.list().await? {
        println!("{} {:?} {}", task.id, task.state, task.link);
    }
    Ok(())
}
