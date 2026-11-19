//! Minimal library embedding: resolve, add and run one task.
//!
//! ```console
//! cargo run -p shaul --example library_basic -- https://example.com/file.bin .
//! ```

use std::path::PathBuf;

use shaul::{Config, Link, TaskOptions, default_engine};

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
    let options = TaskOptions {
        connections: Some(4),
        ..TaskOptions::default()
    };
    let task = engine.add(Link::new(raw)?, destination, options).await?;
    println!("task {} -> backend {:?}", task.id, task.backend);

    let task = engine.run_task(task.id).await?;
    println!("final state: {:?}", task.state);
    for file in task.files {
        println!("{} ({} bytes)", file.path, file.size.unwrap_or(0));
    }
    Ok(())
}
