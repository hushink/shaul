use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use shaul::{DownloadEvent, EventKind};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::support::{human_bytes, redact_link_text, short_id};

pub fn spawn(
    json: bool,
    quiet: bool,
    receiver: broadcast::Receiver<DownloadEvent>,
) -> (tokio::task::JoinHandle<()>, CancellationToken) {
    let stop = CancellationToken::new();
    let stop_for_task = stop.clone();
    let handle = tokio::spawn(async move {
        if json {
            render_json(receiver, stop_for_task).await;
        } else if quiet || !std::io::stderr().is_terminal() {
            render_plain(receiver, quiet, stop_for_task).await;
        } else {
            render_progress(receiver, stop_for_task).await;
        }
    });
    (handle, stop)
}

async fn render_json(mut receiver: broadcast::Receiver<DownloadEvent>, stop: CancellationToken) {
    let mut stdout = std::io::stdout();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            received = receiver.recv() => match received {
                Ok(mut event) => {
                    match &mut event.kind {
                        EventKind::TaskAdded { link, .. }
                        | EventKind::TaskStarted { link, .. } => {
                            *link = redact_link_text(link);
                        }
                        _ => {}
                    }
                    if let Ok(line) = serde_json::to_string(&event) {
                        let _ = writeln!(stdout, "{line}");
                        let _ = stdout.flush();
                    }
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    eprintln!("warning: event stream lagged by {count} events");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

async fn render_plain(
    mut receiver: broadcast::Receiver<DownloadEvent>,
    quiet: bool,
    stop: CancellationToken,
) {
    loop {
        let received = tokio::select! {
            _ = stop.cancelled() => break,
            received = receiver.recv() => received,
        };
        match received {
            Ok(event) => match event.kind {
                EventKind::FileCompleted { path, .. } => println!("{}", path.display()),
                EventKind::TaskCompleted {
                    task_id,
                    files,
                    bytes,
                } => {
                    if !quiet {
                        println!(
                            "[{}] completed: {files} file(s), {}",
                            short_id(&task_id),
                            human_bytes(bytes)
                        );
                    }
                }
                EventKind::TaskFailed { task_id, error } => {
                    eprintln!("[{}] failed: {error}", short_id(&task_id));
                }
                EventKind::TaskPaused { task_id } => {
                    if !quiet {
                        println!("[{}] paused", short_id(&task_id));
                    }
                }
                EventKind::TaskCancelled { task_id } => {
                    if !quiet {
                        println!("[{}] cancelled", short_id(&task_id));
                    }
                }
                EventKind::Warning { message, .. } => eprintln!("warning: {message}"),
                _ => {}
            },
            Err(broadcast::error::RecvError::Lagged(count)) => {
                eprintln!("warning: event stream lagged by {count} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn render_progress(
    mut receiver: broadcast::Receiver<DownloadEvent>,
    stop: CancellationToken,
) {
    let multi = MultiProgress::new();
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();

    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            received = receiver.recv() => match received {
                Ok(event) => match event.kind {
                    EventKind::FileStarted {
                        task_id,
                        file,
                        total_bytes,
                    } => {
                        let key = format!("{task_id}:{file}");
                        let pb = bars
                            .entry(key)
                            .or_insert_with(|| multi.add(new_bar(&file, total_bytes)));
                        if let Some(total) = total_bytes {
                            pb.set_length(total);
                        }
                        pb.set_message(file);
                    }
                    EventKind::Progress {
                        task_id,
                        file,
                        downloaded_bytes,
                        total_bytes,
                    } => {
                        let key = format!("{task_id}:{file}");
                        let pb = bars
                            .entry(key)
                            .or_insert_with(|| multi.add(new_bar(&file, total_bytes)));
                        if let Some(total) = total_bytes {
                            pb.set_length(total);
                        }
                        pb.set_position(downloaded_bytes);
                    }
                    EventKind::FileCompleted {
                        task_id,
                        file,
                        path,
                        ..
                    } => {
                        let key = format!("{task_id}:{file}");
                        if let Some(pb) = bars.remove(&key) {
                            pb.finish_and_clear();
                            multi.remove(&pb);
                        }
                        let _ = multi.println(format!(
                            "[{}] saved {}",
                            short_id(&task_id),
                            path.display()
                        ));
                    }
                    EventKind::ExtractionStarted {
                        task_id,
                        file,
                        target,
                    } => {
                        let _ = multi.println(format!(
                            "[{}] extracting {file} -> {}",
                            short_id(&task_id),
                            target.display()
                        ));
                    }
                    EventKind::ExtractionCompleted {
                        task_id,
                        target,
                        files_extracted,
                        ..
                    } => {
                        let _ = multi.println(format!(
                            "[{}] extracted {files_extracted} file(s) to {}",
                            short_id(&task_id),
                            target.display()
                        ));
                    }
                    EventKind::VerificationCompleted {
                        task_id,
                        file,
                        ok: false,
                        actual,
                    } => {
                        let _ = multi.println(format!(
                            "[{}] checksum mismatch for {file}: {actual}",
                            short_id(&task_id)
                        ));
                    }
                    EventKind::TaskCompleted {
                        task_id,
                        files,
                        bytes,
                    } => {
                        let _ = multi.println(format!(
                            "[{}] completed: {files} file(s), {}",
                            short_id(&task_id),
                            human_bytes(bytes)
                        ));
                    }
                    EventKind::TaskFailed { task_id, error } => {
                        let _ = multi.println(format!(
                            "[{}] failed: {error}",
                            short_id(&task_id)
                        ));
                    }
                    EventKind::TaskPaused { task_id } => {
                        let _ = multi.println(format!("[{}] paused", short_id(&task_id)));
                    }
                    EventKind::TaskCancelled { task_id } => {
                        let _ = multi.println(format!("[{}] cancelled", short_id(&task_id)));
                    }
                    EventKind::Warning { message, .. } => eprintln!("warning: {message}"),
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    eprintln!("warning: event stream lagged by {count} events");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    for (_, pb) in bars {
        pb.finish_and_clear();
    }
}

fn new_bar(file: &str, total: Option<u64>) -> ProgressBar {
    let pb = match total {
        Some(total) => ProgressBar::new(total),
        None => ProgressBar::new_spinner(),
    };
    let template = if total.is_some() {
        "{msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {binary_bytes_per_sec} {eta}"
    } else {
        "{msg} {bytes} {binary_bytes_per_sec}"
    };
    let style = ProgressStyle::with_template(template)
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-");
    pb.set_style(style);
    pb.set_message(file.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}
