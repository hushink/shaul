//! Use the blocking facade from a synchronous application.
//!
//! ```console
//! cargo run -p shaul --example library_blocking -- https://example.com/file.bin .
//! ```

use std::path::PathBuf;

use shaul::{Config, Link, TaskOptions, default_blocking_engine};

fn main() -> shaul::Result<()> {
    let raw = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/file.bin".to_string());
    let destination = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);

    let engine = default_blocking_engine(Config::default())?;
    let task = engine.download(Link::new(raw)?, destination, TaskOptions::default())?;
    println!("{:?}: {} file(s)", task.state, task.files.len());
    Ok(())
}
