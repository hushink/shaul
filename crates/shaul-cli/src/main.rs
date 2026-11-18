#![forbid(unsafe_code)]

mod args;
mod render;
mod support;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use shaul::{Config, DownloadEngine, Error, Link, Result, TaskState};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use args::{Cli, Commands, ConfigArgs, ConfigCmd, DownloadArgs, IdArgs, InfoArgs, RunArgs};
use support::{
    acquire_run_lock, display_link, human_bytes, redacted_task, resolve_task_id, short_id,
};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let Cli {
        config: config_arg,
        json,
        quiet,
        verbose: _,
        command,
    } = cli;
    let (mut config, config_path) = Config::load(config_arg.as_deref())?;

    match command {
        Commands::Download(args) => command_download(&mut config, args, json, quiet).await,
        Commands::Add(args) => command_add(&mut config, args, json).await,
        Commands::Run(args) => command_run(&mut config, args, json, quiet).await,
        Commands::List => command_list(&config, json).await,
        Commands::Status(args) => command_status(&config, args, json).await,
        Commands::Pause(args) => command_pause(&config, args).await,
        Commands::Resume(args) => command_resume(&config, args).await,
        Commands::Cancel(args) => command_cancel(&config, args).await,
        Commands::Info(args) => command_info(&config, args, json).await,
        Commands::Config(args) => command_config(&config, &config_path, args, json),
        Commands::Completions { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "shaul", &mut std::io::stdout());
            Ok(())
        }
    }
}

fn init_logging(verbose: u8) {
    let default = if verbose > 0 { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn build_engine(config: &Config) -> Result<DownloadEngine> {
    let (engine, _store) = shaul::default_engine(config.clone())?;
    Ok(engine)
}

fn apply_download_overrides(config: &mut Config, args: &DownloadArgs) {
    if let Some(dir) = &args.dir {
        config.download.dir = dir.clone();
    }
    if let Some(connections) = args.connections {
        config.download.connections = connections;
    }
}

async fn command_download(
    config: &mut Config,
    args: DownloadArgs,
    json: bool,
    quiet: bool,
) -> Result<()> {
    if args.links.len() > 1 && args.output.is_some() {
        return Err(Error::Config(
            "--output applies to one link; use --dir for multiple links".to_string(),
        ));
    }
    apply_download_overrides(config, &args);
    let options = args.to_task_options()?;
    let destination = args
        .dir
        .clone()
        .unwrap_or_else(|| config.download.dir.clone());
    let engine = build_engine(config)?;

    let (renderer, stop) = render::spawn(json, quiet, engine.subscribe());
    let cancel = CancellationToken::new();
    let ctrl_c = spawn_ctrl_c(cancel.clone());
    let mut failures = 0usize;

    for raw in &args.links {
        if cancel.is_cancelled() {
            break;
        }
        let link = Link::new(raw.clone())?;
        let task = match engine.add(link, destination.clone(), options.clone()).await {
            Ok(task) => task,
            Err(error) => {
                eprintln!("error: {error}");
                failures += 1;
                continue;
            }
        };
        if task.state == TaskState::Running {
            eprintln!(
                "task {} is already running in another worker",
                short_id(&task.id)
            );
            continue;
        }
        match engine
            .run_task_with_cancel(task.id, Some(cancel.clone()))
            .await
        {
            Ok(task) => {
                if task.state == TaskState::Failed {
                    failures += 1;
                }
                if task.state == TaskState::Paused && cancel.is_cancelled() {
                    eprintln!(
                        "paused {}; continue with `shaul resume {}` and `shaul run`",
                        short_id(&task.id),
                        task.id
                    );
                }
            }
            Err(error) => {
                eprintln!("error: {error}");
                failures += 1;
            }
        }
    }

    stop.cancel();
    let _ = renderer.await;
    ctrl_c.abort();
    if failures > 0 {
        Err(Error::Other(format!("{failures} download(s) failed")))
    } else {
        Ok(())
    }
}

async fn command_add(config: &mut Config, args: DownloadArgs, json: bool) -> Result<()> {
    if args.links.len() > 1 && args.output.is_some() {
        return Err(Error::Config(
            "--output applies to one link; use --dir for multiple links".to_string(),
        ));
    }
    apply_download_overrides(config, &args);
    let options = args.to_task_options()?;
    let destination = args
        .dir
        .clone()
        .unwrap_or_else(|| config.download.dir.clone());
    let engine = build_engine(config)?;

    let mut tasks = Vec::new();
    for raw in &args.links {
        let link = Link::new(raw.clone())?;
        tasks.push(
            engine
                .add(link, destination.clone(), options.clone())
                .await?,
        );
    }

    if json {
        let redacted: Vec<_> = tasks.iter().map(redacted_task).collect();
        println!("{}", serde_json::to_string_pretty(&redacted)?);
    } else {
        for task in &tasks {
            println!(
                "{}  {:?}  {}",
                short_id(&task.id),
                task.state,
                display_link(&task.link)
            );
        }
    }
    Ok(())
}

async fn command_run(config: &mut Config, args: RunArgs, json: bool, quiet: bool) -> Result<()> {
    let state_dir = config.state_dir()?;
    let _lock = acquire_run_lock(&state_dir)?;
    let engine = build_engine(config)?;
    engine.requeue_stale().await?;

    let (renderer, stop) = render::spawn(json, quiet, engine.subscribe());
    let cancel = CancellationToken::new();
    let ctrl_c = spawn_ctrl_c(cancel.clone());
    let mut idle = false;

    loop {
        if cancel.is_cancelled() {
            break;
        }
        let has_pending = engine
            .list()
            .await?
            .into_iter()
            .any(|task| task.state == TaskState::Pending);
        if has_pending {
            idle = false;
            engine.run_pending(args.jobs.max(1), cancel.clone()).await?;
        } else if args.once {
            break;
        } else {
            if !idle {
                eprintln!("queue is empty; waiting for tasks (Ctrl-C to stop)");
                idle = true;
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(750)) => {}
            }
        }
    }

    stop.cancel();
    let _ = renderer.await;
    ctrl_c.abort();
    Ok(())
}

async fn command_list(config: &Config, json: bool) -> Result<()> {
    let engine = build_engine(config)?;
    let tasks = engine.list().await?;
    if json {
        let redacted: Vec<_> = tasks.iter().map(redacted_task).collect();
        println!("{}", serde_json::to_string_pretty(&redacted)?);
        return Ok(());
    }
    println!("{:<10} {:<10} {:>20}  LINK", "ID", "STATE", "PROGRESS");
    for task in &tasks {
        let progress = match task.total_bytes() {
            Some(total) => format!(
                "{}/{}",
                human_bytes(task.downloaded_bytes()),
                human_bytes(total)
            ),
            None => human_bytes(task.downloaded_bytes()),
        };
        println!(
            "{:<10} {:<10} {:>20}  {}",
            short_id(&task.id),
            format!("{:?}", task.state),
            progress,
            display_link(&task.link)
        );
    }
    Ok(())
}

async fn command_status(config: &Config, args: IdArgs, json: bool) -> Result<()> {
    let engine = build_engine(config)?;
    let id = resolve_task_id(&engine, &args.id).await?;
    let task = engine.status(id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&redacted_task(&task))?);
        return Ok(());
    }

    println!("id:          {}", task.id);
    println!("state:       {:?}", task.state);
    println!("link:        {}", display_link(&task.link));
    println!("destination: {}", task.destination.display());
    println!("backend:     {}", task.backend.as_deref().unwrap_or("-"));
    println!(
        "progress:    {}/{}",
        human_bytes(task.downloaded_bytes()),
        task.total_bytes()
            .map(human_bytes)
            .unwrap_or_else(|| "?".to_string())
    );
    if let Some(error) = &task.error {
        println!("error:       {error}");
    }
    for file in &task.files {
        println!(
            "  {:<40} {}/{}  {:?}",
            file.path,
            human_bytes(file.downloaded),
            file.size
                .map(human_bytes)
                .unwrap_or_else(|| "?".to_string()),
            file.state
        );
    }
    Ok(())
}

async fn command_pause(config: &Config, args: IdArgs) -> Result<()> {
    let engine = build_engine(config)?;
    let id = resolve_task_id(&engine, &args.id).await?;
    let task = engine.pause(id).await?;
    println!("{}: {:?}", short_id(&task.id), task.state);
    Ok(())
}

async fn command_resume(config: &Config, args: IdArgs) -> Result<()> {
    let engine = build_engine(config)?;
    let id = resolve_task_id(&engine, &args.id).await?;
    let task = engine.resume(id).await?;
    println!(
        "{}: {:?} (the run worker will pick it up)",
        short_id(&task.id),
        task.state
    );
    Ok(())
}

async fn command_cancel(config: &Config, args: IdArgs) -> Result<()> {
    let engine = build_engine(config)?;
    let id = resolve_task_id(&engine, &args.id).await?;
    let task = engine.cancel(id).await?;
    println!("{}: {:?}", short_id(&task.id), task.state);
    Ok(())
}

async fn command_info(config: &Config, args: InfoArgs, json: bool) -> Result<()> {
    let engine = build_engine(config)?;
    let link = Link::new(args.link.clone())?;
    let routed = engine.resolve(&link, &args.to_parse_options()?).await?;

    if json {
        let value = serde_json::json!({
            "resource": routed.resource,
            "backend": routed.backend.name(),
            "parser": routed.parser,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("name:    {}", routed.resource.name);
    println!("kind:    {:?}", routed.resource.kind);
    println!("parser:  {}", routed.parser);
    println!("backend: {}", routed.backend.name());
    if let Some(revision) = &routed.resource.revision {
        println!("revision: {revision}");
    }
    if let Some(checksum) = &routed.resource.checksum {
        println!("checksum: {checksum}");
    }
    for file in &routed.resource.files {
        println!("  {}", file.path);
        println!("    url: {}", file.url);
        if let Some(size) = file.size {
            println!("    size: {}", human_bytes(size));
        }
        if let Some(checksum) = &file.checksum {
            println!("    checksum: {checksum}");
        }
    }
    Ok(())
}

fn command_config(config: &Config, path: &Path, args: ConfigArgs, json: bool) -> Result<()> {
    match args.command {
        ConfigCmd::Path => println!("{}", path.display()),
        ConfigCmd::Show => {
            let display = redacted_config(config);
            if json {
                println!("{}", serde_json::to_string_pretty(&display)?);
            } else {
                println!("{}", toml::to_string_pretty(&display)?);
            }
        }
        ConfigCmd::Get { key } => {
            if let Some(name) = key.strip_prefix("headers.")
                && is_sensitive_header(name)
            {
                println!("<redacted>");
                return Ok(());
            }
            println!("{}", config.get_value(&key)?);
        }
        ConfigCmd::Set { key, value } => {
            let mut file_config = Config::load_file_only(path)?;
            file_config.set_value(&key, &value)?;
            file_config.save(path)?;
            println!("{key} = {value}");
        }
        ConfigCmd::Init { force } => {
            if path.exists() && !force {
                return Err(Error::Config(format!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                )));
            }
            Config::default().save(path)?;
            println!("{}", path.display());
        }
        ConfigCmd::Edit => {
            if !path.exists() {
                Config::default().save(path)?;
            }
            open_editor(path)?;
        }
    }
    Ok(())
}

fn redacted_config(config: &Config) -> Config {
    let mut config = config.clone();
    for (name, value) in &mut config.headers {
        if is_sensitive_header(name) {
            *value = "<redacted>".to_string();
        }
    }
    config
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "proxy-authorization" | "x-api-key"
    )
}

fn open_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });
    let status = Command::new(&editor)
        .arg(path)
        .status()
        .map_err(|error| Error::Other(format!("failed to start {editor}: {error}")))?;
    if !status.success() {
        return Err(Error::Other(format!("{editor} exited with {status}")));
    }
    Ok(())
}

fn spawn_ctrl_c(cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\ninterrupt received; checkpointing...");
            cancel.cancel();
        }
    })
}
