use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn shaul() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shaul"));
    for key in [
        "SHAUL_CONFIG",
        "SHAUL_DIR",
        "SHAUL_CONNECTIONS",
        "SHAUL_PER_HOST",
        "SHAUL_PER_HOST_ACTIVE",
        "SHAUL_GLOBAL",
        "SHAUL_RATE_LIMIT",
        "SHAUL_FILE_JOBS",
        "SHAUL_SEGMENT_SIZE",
        "SHAUL_RESUME",
        "SHAUL_VERIFY",
        "SHAUL_EXTRACT",
        "SHAUL_EXTRACT_DIR",
        "SHAUL_DELETE_ARCHIVE",
        "SHAUL_STATE_DIR",
        "SHAUL_TIMEOUT_SECS",
        "SHAUL_CONNECT_TIMEOUT_SECS",
        "SHAUL_RETRIES",
        "SHAUL_USER_AGENT",
        "SHAUL_PROXY",
        "SHAUL_NO_PROXY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "HF_TOKEN",
        "HUGGING_FACE_HUB_TOKEN",
        "MODELSCOPE_API_TOKEN",
    ] {
        command.env_remove(key);
    }
    command
}

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("failed to run shaul");
    assert!(
        output.status.success(),
        "shaul failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn config_path(dir: &TempDir) -> PathBuf {
    dir.path().join("config.toml")
}

fn write_config(dir: &TempDir, extra: &str) -> PathBuf {
    let path = config_path(dir);
    let state = toml_path(dir.path().join("state"));
    let downloads = toml_path(dir.path().join("downloads"));
    let text = format!("state_dir = \"{state}\"\n[download]\ndir = \"{downloads}\"\n{extra}");
    fs::write(&path, text).unwrap();
    path
}

fn toml_path(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout was not JSON")
}

#[test]
fn reports_version_and_config_path() {
    let output = run(shaul().arg("--version"));
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output = run(shaul().args(["--config", path.to_str().unwrap(), "config", "path"]));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        path.to_string_lossy()
    );
}

#[test]
fn config_file_wins_over_environment() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "connections = 11\nper_host = 12\n");

    let output = run(shaul()
        .args([
            "--config",
            path.to_str().unwrap(),
            "config",
            "get",
            "download.connections",
        ])
        .env("SHAUL_CONNECTIONS", "22"));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "11");

    // A missing explicit config still allows environment values.
    let missing = dir.path().join("missing.toml");
    let output = run(shaul()
        .args([
            "--config",
            missing.to_str().unwrap(),
            "config",
            "get",
            "download.per_host",
        ])
        .env("SHAUL_PER_HOST", "33"));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "33");
}

#[test]
fn proxy_flags_override_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--proxy",
        "http://127.0.0.1:8080",
        "--no-proxy",
        "localhost,127.0.0.1",
        "config",
        "get",
        "network.proxy",
    ]));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "http://127.0.0.1:8080"
    );

    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--global-connections",
        "3",
        "--per-host-active",
        "2",
        "config",
        "get",
        "download.global",
    ]));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");
    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--global-connections",
        "3",
        "--per-host-active",
        "2",
        "config",
        "get",
        "download.per_host_active",
    ]));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2");
}

#[test]
fn config_show_redacts_proxy_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        &dir,
        "[network]\nproxy = \"http://user:secret@127.0.0.1:8080\"\n",
    );
    let output = run(shaul().args(["--config", path.to_str().unwrap(), "config", "show"]));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("secret"),
        "proxy password leaked: {stdout}"
    );
    assert!(
        stdout.contains("redacted"),
        "proxy was not redacted: {stdout}"
    );
}

#[test]
fn input_file_supplies_download_links() {
    let (url, _server) = spawn_file_server(b"from input file".to_vec());
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let input = dir.path().join("links.txt");
    fs::write(&input, format!("# generated\n\n{url}\n")).unwrap();
    let output_dir = dir.path().join("input-download");
    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        "--input-file",
        input.to_str().unwrap(),
        "--dir",
        output_dir.to_str().unwrap(),
    ]));
    assert_eq!(
        fs::read(output_dir.join("file.bin")).unwrap(),
        b"from input file"
    );
}

#[test]
fn add_list_status_and_cancel_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let config = path.to_str().unwrap();

    let override_dir = dir.path().join("override");
    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        "https://example.com/file.bin",
        "--dir",
        override_dir.to_str().unwrap(),
        "--priority",
        "7",
    ]));
    let added = json_stdout(&output);
    let tasks = added.as_array().expect("add output should be an array");
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task["state"], "pending");
    assert_eq!(task["backend"], "http");
    assert_eq!(task["priority"], 7);
    assert!(
        task["destination"]
            .as_str()
            .unwrap()
            .replace('\\', "/")
            .ends_with("override")
    );
    let id = task["id"].as_str().unwrap().to_string();

    let output = run(shaul().args(["--config", config, "--json", "list"]));
    let listed = json_stdout(&output);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == id)
    );

    let output = run(shaul().args(["--config", config, "--json", "status", &id]));
    assert_eq!(json_stdout(&output)["state"], "pending");

    let output = run(shaul().args(["--config", config, "pause", &id]));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Paused"));
    let output = run(shaul().args(["--config", config, "resume", &id]));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Pending"));

    let output = run(shaul().args(["--config", config, "cancel", &id]));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Cancelled"),
        "cancel output: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let output = run(shaul().args(["--config", config, "--json", "status", &id]));
    assert_eq!(json_stdout(&output)["state"], "cancelled");
}

#[test]
fn json_output_redacts_auth_and_query_strings() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--json",
        "add",
        "https://example.com/file.bin?token=super-secret",
        "--header",
        "Authorization: Bearer super-secret",
    ]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("super-secret"), "leaked secret: {text}");
    assert!(
        text.contains("<redacted>"),
        "missing redaction marker: {text}"
    );

    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--json",
        "add",
        "ftp://user:ftp-secret@example.com/file.bin",
    ]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("ftp-secret"), "leaked FTP password: {text}");
    assert!(text.contains("redacted"), "missing FTP redaction: {text}");
}

#[test]
fn info_reports_the_direct_http_route() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "info",
        "https://example.com/file.bin",
    ]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("backend: http"), "output: {text}");
    assert!(text.contains("parser:  direct"), "output: {text}");
}

#[test]
fn info_reports_the_ftp_route() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "info",
        "ftp://example.com/file.bin",
    ]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("backend: ftp"), "output: {text}");
    assert!(text.contains("parser:  ftp"), "output: {text}");
}

#[test]
fn download_runs_through_the_cli() {
    let data: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
    let (url, _server) = spawn_file_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("downloaded.bin");

    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--connections",
        "2",
        "--output",
        output_path.to_str().unwrap(),
    ]));

    assert_eq!(fs::read(&output_path).unwrap(), data);
}

#[test]
fn run_worker_processes_a_queued_task() {
    let data: Vec<u8> = (0..48 * 1024).map(|index| (index % 239) as u8).collect();
    let (url, _server) = spawn_file_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("queued.bin");
    let config = path.to_str().unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        &url,
        "--output",
        output_path.to_str().unwrap(),
    ]));
    let id = json_stdout(&output)[0]["id"].as_str().unwrap().to_string();

    run(shaul().args(["--config", config, "run", "--once"]));

    let output = run(shaul().args(["--config", config, "--json", "status", &id]));
    assert_eq!(json_stdout(&output)["state"], "completed");
    assert_eq!(fs::read(&output_path).unwrap(), data);

    let output = shaul()
        .args(["--config", config, "cancel", &id])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("already completed"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn killed_worker_resumes_from_checkpoint() {
    let data: Vec<u8> = (0..512 * 1024).map(|index| (index % 241) as u8).collect();
    let (url, requests, release_full) = spawn_slow_file_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("resumed.bin");
    let config = path.to_str().unwrap();

    run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        &url,
        "--output",
        output_path.to_str().unwrap(),
    ]));

    let mut child = shaul()
        .args(["--config", config, "run", "--once"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_until(
        || requests.load(Ordering::SeqCst) > 0,
        "worker did not reach the test server",
    );
    thread::sleep(Duration::from_millis(300));
    child.kill().unwrap();
    let _ = child.wait();

    // Let the next request receive the complete body. The stale Running task is
    // requeued by `run --once` on the next worker start.
    release_full.store(true, Ordering::SeqCst);
    run(shaul().args(["--config", config, "run", "--once"]));

    assert_eq!(fs::read(&output_path).unwrap(), data);
}

fn wait_until(mut predicate: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Minimal HTTP/1.1 server that ignores Range and always returns the full body.
fn spawn_file_server(data: Vec<u8>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let mut request = [0_u8; 16 * 1024];
            let _ = stream.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Length: {}\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Content-Disposition: attachment; filename=\"file.bin\"\r\n\
                 Connection: close\r\n\
                 \r\n",
                data.len()
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&data);
            let _ = stream.flush();
        }
    });
    (format!("http://{address}/file.bin"), handle)
}

/// Sends half of the body and stalls until `release_full` is set, then serves
/// complete responses. Used to simulate a worker killed mid-transfer.
fn spawn_slow_file_server(data: Vec<u8>) -> (String, Arc<AtomicUsize>, Arc<AtomicBool>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let release_full = Arc::new(AtomicBool::new(false));
    let seen = requests.clone();
    let release = release_full.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let data = data.clone();
            let seen = seen.clone();
            let release = release.clone();
            thread::spawn(move || {
                let mut request = [0_u8; 16 * 1024];
                let _ = stream.read(&mut request);
                seen.fetch_add(1, Ordering::SeqCst);
                let headers = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Length: {}\r\n\
                     Content-Type: application/octet-stream\r\n\
                     Content-Disposition: attachment; filename=\"file.bin\"\r\n\
                     Connection: close\r\n\
                     \r\n",
                    data.len()
                );
                let _ = stream.write_all(headers.as_bytes());
                if release.load(Ordering::SeqCst) {
                    let _ = stream.write_all(&data);
                    let _ = stream.flush();
                } else {
                    let _ = stream.write_all(&data[..data.len() / 2]);
                    let _ = stream.flush();
                    thread::sleep(Duration::from_secs(30));
                }
            });
        }
    });
    (format!("http://{address}/file.bin"), requests, release_full)
}
