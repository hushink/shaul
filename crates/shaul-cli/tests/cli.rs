use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
        "SHAUL_PER_HOST_RATE_LIMIT",
        "SHAUL_FILE_JOBS",
        "SHAUL_CONTENT_CACHE",
        "SHAUL_CONTENT_CACHE_DIR",
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
    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--per-host-limit",
        "1MiB",
        "config",
        "get",
        "download.per_host_rate_limit",
    ]));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        (1024 * 1024).to_string()
    );
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
fn config_set_redacts_sensitive_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let config = path.to_str().unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "config",
        "set",
        "headers.Authorization",
        "Bearer super-secret",
    ]));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("super-secret"),
        "header secret leaked: {stdout}"
    );
    assert!(stdout.contains("<redacted>"), "{stdout}");
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("super-secret"),
        "the value should still be stored in the file"
    );

    let output = run(shaul().args([
        "--config",
        config,
        "config",
        "set",
        "network.proxy",
        "http://user:proxy-secret@127.0.0.1:8080",
    ]));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("proxy-secret"),
        "proxy password leaked: {stdout}"
    );
    assert!(stdout.contains("redacted"), "{stdout}");
}

#[test]
fn config_set_preserves_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_path(&dir);
    let state = toml_path(dir.path().join("state"));
    let downloads = toml_path(dir.path().join("downloads"));
    fs::write(
        &path,
        format!(
            "# top comment\nstate_dir = \"{state}\"\n\n[download]\n# connection comment\ndir = \"{downloads}\"\nconnections = 4 # trailing\n"
        ),
    )
    .unwrap();

    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "config",
        "set",
        "download.connections",
        "12",
    ]));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("download.connections = 12"), "{stdout}");

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# top comment"), "{text}");
    assert!(text.contains("# connection comment"), "{text}");
    assert!(text.contains("connections = 12 # trailing"), "{text}");
}

#[test]
fn unknown_config_keys_fail_without_rewriting_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "connection = 8\n");
    let config = path.to_str().unwrap();

    let output = shaul()
        .args([
            "--config",
            config,
            "config",
            "set",
            "download.connections",
            "4",
        ])
        .output()
        .expect("failed to run shaul");
    assert!(
        !output.status.success(),
        "a typo in the config file must not be ignored"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown field") || stderr.contains("connection"),
        "unexpected error: {stderr}"
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("connection = 8"),
        "the original config must be left untouched"
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
        "--content-cache",
    ]));
    let added = json_stdout(&output);
    let tasks = added.as_array().expect("add output should be an array");
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task["state"], "pending");
    assert_eq!(task["backend"], "http");
    assert_eq!(task["priority"], 7);
    assert_eq!(task["options"]["content_cache"], true);
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
fn batch_pause_resume_cancel_and_list_filters() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let config = path.to_str().unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        "https://example.com/a.bin",
        "https://example.com/b.bin",
        "https://example.com/c.bin",
    ]));
    let tasks = json_stdout(&output);
    let ids: Vec<String> = tasks
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 3);

    let output = run(shaul().args(["--config", config, "--json", "list", "--status", "pending"]));
    assert_eq!(json_stdout(&output).as_array().unwrap().len(), 3);

    run(shaul().args(["--config", config, "pause", &ids[0], &ids[1]]));
    let output = run(shaul().args(["--config", config, "--json", "list", "--status", "paused"]));
    assert_eq!(json_stdout(&output).as_array().unwrap().len(), 2);

    run(shaul().args(["--config", config, "pause", "--all", "--status", "pending"]));
    let output = run(shaul().args(["--config", config, "--json", "list", "--status", "paused"]));
    assert_eq!(json_stdout(&output).as_array().unwrap().len(), 3);

    run(shaul().args(["--config", config, "resume", "--all", "--status", "paused"]));
    let output = run(shaul().args(["--config", config, "--json", "list", "--status", "pending"]));
    assert_eq!(json_stdout(&output).as_array().unwrap().len(), 3);

    run(shaul().args(["--config", config, "cancel", "--all"]));
    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "list",
        "--status",
        "cancelled",
        "--status",
        "pending",
    ]));
    assert_eq!(json_stdout(&output).as_array().unwrap().len(), 3);

    let output = run(shaul().args(["--config", config, "cancel", "--all"]));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no matching tasks"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let output = shaul()
        .args(["--config", config, "list", "--status", "bogus"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown task state"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = shaul()
        .args(["--config", config, "pause", "--all", &ids[0]])
        .output()
        .unwrap();
    assert!(!output.status.success());
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
fn info_json_redacts_credentials_and_query_strings() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let config = path.to_str().unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "info",
        "https://example.com/file.bin?token=super-secret",
        "--header",
        "Authorization: Bearer header-secret",
    ]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("super-secret"),
        "query secret leaked: {text}"
    );
    assert!(
        !text.contains("header-secret"),
        "header secret leaked: {text}"
    );
    assert!(text.contains("<redacted>"), "missing redaction: {text}");

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "info",
        "ftp://user:ftp-secret@example.com/file.bin",
    ]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("ftp-secret"), "FTP secret leaked: {text}");
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
fn doctor_reports_config_and_directories() {
    let dir = tempfile::tempdir().unwrap();
    let cache = toml_path(dir.path().join("content-cache"));
    let path = write_config(
        &dir,
        &format!("content_cache = true\ncontent_cache_dir = \"{cache}\"\n"),
    );
    let output = run(shaul().args(["--config", path.to_str().unwrap(), "--json", "doctor"]));
    let checks = json_stdout(&output);
    let checks = checks.as_array().expect("doctor output should be an array");
    for name in ["config", "state_dir", "download_dir", "content_cache"] {
        let check = checks
            .iter()
            .find(|check| check["name"] == name)
            .unwrap_or_else(|| panic!("missing doctor check: {name}"));
        assert_ne!(check["status"], "error", "doctor check failed: {check}");
    }
}

#[test]
fn stats_and_clean_manage_finished_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let cache = toml_path(dir.path().join("content-cache"));
    let path = write_config(&dir, &format!("content_cache_dir = \"{cache}\"\n"));
    let config = path.to_str().unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        "https://example.com/file.bin",
    ]));
    let id = json_stdout(&output)[0]["id"].as_str().unwrap().to_string();
    run(shaul().args(["--config", config, "cancel", &id]));

    let output = run(shaul().args(["--config", config, "--json", "stats"]));
    let stats = json_stdout(&output);
    assert_eq!(stats["tasks"]["total"], 1);
    assert_eq!(stats["tasks"]["by_state"]["cancelled"], 1);

    let output = run(shaul().args(["--config", config, "--json", "clean", "--cancelled"]));
    let cleaned = json_stdout(&output);
    assert_eq!(cleaned["tasks"], 1);

    let output = run(shaul().args(["--config", config, "--json", "list"]));
    assert!(json_stdout(&output).as_array().unwrap().is_empty());
}

#[test]
fn content_cache_is_populated_and_can_be_cleaned() {
    let data = b"cache test".to_vec();
    let checksum = "d420c9e26942b8062e73f9aa281c912cf0f8eada582fede3de1cd10bd4f72396";
    let (url, _server) = spawn_file_server(data.clone());
    let dir = tempfile::tempdir().unwrap();
    let cache = toml_path(dir.path().join("content-cache"));
    let path = write_config(
        &dir,
        &format!("content_cache = true\ncontent_cache_dir = \"{cache}\"\n"),
    );
    let config = path.to_str().unwrap();
    let first = dir.path().join("first.bin");
    let second = dir.path().join("second.bin");

    run(shaul().args([
        "--config",
        config,
        "download",
        &url,
        "--output",
        first.to_str().unwrap(),
        "--checksum",
        &format!("sha256:{checksum}"),
    ]));
    assert_eq!(fs::read(&first).unwrap(), data);

    let output = run(shaul().args(["--config", config, "--json", "stats"]));
    assert_eq!(json_stdout(&output)["cache"]["entries"], 1);

    run(shaul().args([
        "--config",
        config,
        "download",
        &url,
        "--output",
        second.to_str().unwrap(),
        "--checksum",
        &format!("sha256:{checksum}"),
        "--force",
    ]));
    assert_eq!(fs::read(&second).unwrap(), data);
    let output = run(shaul().args(["--config", config, "--json", "stats"]));
    assert_eq!(json_stdout(&output)["cache"]["entries"], 1);

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "clean",
        "--cache",
        "--cache-max-size",
        "0",
    ]));
    let cleaned = json_stdout(&output);
    assert_eq!(cleaned["cache_entries"], 1);
    assert_eq!(cleaned["cache_entries_remaining"], 0);
    let output = run(shaul().args(["--config", config, "--json", "stats"]));
    assert_eq!(json_stdout(&output)["cache"]["entries"], 0);
}

#[test]
fn content_cache_hit_avoids_the_response_body() {
    let data = b"cache hit body".to_vec();
    let checksum = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&data))
    };
    let (url, serve_body) = spawn_cache_test_server(data.clone());
    let dir = tempfile::tempdir().unwrap();
    let cache = toml_path(dir.path().join("content-cache"));
    let path = write_config(
        &dir,
        &format!("content_cache = true\ncontent_cache_dir = \"{cache}\"\n"),
    );
    let config = path.to_str().unwrap();
    let first = dir.path().join("first.bin");
    let second = dir.path().join("second.bin");

    run(shaul().args([
        "--config",
        config,
        "download",
        &url,
        "--output",
        first.to_str().unwrap(),
        "--connections",
        "1",
        "--checksum",
        &format!("sha256:{checksum}"),
    ]));
    serve_body.store(false, Ordering::SeqCst);

    run(shaul().args([
        "--config",
        config,
        "download",
        &url,
        "--output",
        second.to_str().unwrap(),
        "--connections",
        "1",
        "--checksum",
        &format!("sha256:{checksum}"),
        "--force",
    ]));
    assert_eq!(fs::read(&second).unwrap(), data);
}

#[test]
fn clean_removes_orphan_parts() {
    let dir = tempfile::tempdir().unwrap();
    let cache = toml_path(dir.path().join("content-cache"));
    let path = write_config(&dir, &format!("content_cache_dir = \"{cache}\"\n"));
    let config = path.to_str().unwrap();
    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        "https://example.com/file.bin",
    ]));
    let id = json_stdout(&output)[0]["id"].as_str().unwrap().to_string();
    run(shaul().args(["--config", config, "cancel", &id]));

    let part = dir.path().join("downloads").join("file.bin.shaul.part");
    fs::create_dir_all(part.parent().unwrap()).unwrap();
    fs::write(&part, b"partial").unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "clean",
        "--orphans",
        "--dry-run",
    ]));
    assert_eq!(json_stdout(&output)["orphan_parts"], 1);
    assert!(part.exists());

    let output = run(shaul().args(["--config", config, "--json", "clean", "--orphans"]));
    assert_eq!(json_stdout(&output)["orphan_parts"], 1);
    assert!(!part.exists());
}

#[test]
fn download_prints_progress_without_a_terminal() {
    let data: Vec<u8> = (0..2 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    let (url, handle) = spawn_streaming_file_server(data.clone(), 12, Duration::from_millis(150));
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("progress.bin");

    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--output",
        output_path.to_str().unwrap(),
        "--connections",
        "1",
    ]));
    drop(handle);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("/s"),
        "expected a progress line, got: {stderr}"
    );
    assert_eq!(fs::read(&output_path).unwrap(), data);
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
fn hls_download_runs_through_the_cli() {
    let (url, _server) = spawn_hls_server(None);

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("downloads").join("master.ts");

    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--connections",
        "2",
    ]));

    assert_eq!(fs::read(&output_path).unwrap(), b"AAAABBBB".to_vec());
}

#[test]
fn clean_removes_hls_temp_directories() {
    let (url, _server) = spawn_hls_server(Some("/seg-1.ts"));

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "[network]\nretries = 0\n");
    let config = path.to_str().unwrap();

    let output = run(shaul().args(["--config", config, "--json", "add", &url]));
    let id = json_stdout(&output)[0]["id"].as_str().unwrap().to_string();

    let _ = shaul()
        .args(["--config", config, "run", "--once"])
        .output()
        .unwrap();

    let output = run(shaul().args(["--config", config, "--json", "status", &id]));
    assert_eq!(json_stdout(&output)["state"], "failed");

    run(shaul().args(["--config", config, "cancel", &id]));

    let temp = dir
        .path()
        .join("downloads")
        .join("master.ts.shaul.hls.parts");
    assert!(
        temp.is_dir(),
        "HLS temp directory should exist: {}",
        temp.display()
    );
    fs::write(temp.join("segment-00000.bin"), b"partial").unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "clean",
        "--orphans",
        "--dry-run",
    ]));
    assert_eq!(json_stdout(&output)["orphan_dirs"], 1);
    assert!(temp.exists());

    let output = run(shaul().args(["--config", config, "--json", "clean", "--orphans"]));
    assert_eq!(json_stdout(&output)["orphan_dirs"], 1);
    assert!(!temp.exists());
}

#[test]
fn dash_download_runs_through_the_cli() {
    let (url, _server) = spawn_dash_server(None);

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("downloads").join("master.mp4");

    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--connections",
        "2",
    ]));

    assert_eq!(fs::read(&output_path).unwrap(), b"INITAAAABBBB".to_vec());
}

#[test]
fn clean_removes_dash_temp_directories() {
    let (url, _server) = spawn_dash_server(Some("/seg-2.m4s"));

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "[network]\nretries = 0\n");
    let config = path.to_str().unwrap();

    let output = run(shaul().args(["--config", config, "--json", "add", &url]));
    let id = json_stdout(&output)[0]["id"].as_str().unwrap().to_string();

    let _ = shaul()
        .args(["--config", config, "run", "--once"])
        .output()
        .unwrap();

    let output = run(shaul().args(["--config", config, "--json", "status", &id]));
    assert_eq!(json_stdout(&output)["state"], "failed");

    run(shaul().args(["--config", config, "cancel", &id]));

    let temp = dir
        .path()
        .join("downloads")
        .join("master.mp4.shaul.dash.parts");
    assert!(
        temp.is_dir(),
        "DASH temp directory should exist: {}",
        temp.display()
    );
    fs::write(temp.join("segment-00000.bin"), b"partial").unwrap();

    let output = run(shaul().args([
        "--config",
        config,
        "--json",
        "clean",
        "--orphans",
        "--dry-run",
    ]));
    assert_eq!(json_stdout(&output)["orphan_dirs"], 1);
    assert!(temp.exists());

    let output = run(shaul().args(["--config", config, "--json", "clean", "--orphans"]));
    assert_eq!(json_stdout(&output)["orphan_dirs"], 1);
    assert!(!temp.exists());
}

#[test]
fn post_download_sends_method_and_body() {
    let data: Vec<u8> = (0..32 * 1024).map(|index| (index % 251) as u8).collect();
    let (url, requests) = spawn_method_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("posted.bin");

    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--method",
        "POST",
        "--data",
        "{\"id\":42}",
        "--header",
        "Content-Type: application/json",
        "--output",
        output_path.to_str().unwrap(),
    ]));

    assert_eq!(fs::read(&output_path).unwrap(), data);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "POST");
    assert_eq!(requests[0].1, b"{\"id\":42}");
}

#[test]
fn data_implies_post() {
    let data: Vec<u8> = (0..16 * 1024).map(|index| (index % 239) as u8).collect();
    let (url, requests) = spawn_method_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("posted.bin");

    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--data",
        "payload",
        "--output",
        output_path.to_str().unwrap(),
    ]));

    assert_eq!(fs::read(&output_path).unwrap(), data);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "POST");
    assert_eq!(requests[0].1, b"payload");
}

#[test]
fn data_file_sends_post_body() {
    let data: Vec<u8> = (0..16 * 1024).map(|index| (index % 227) as u8).collect();
    let (url, requests) = spawn_method_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let body_path = dir.path().join("body.json");
    fs::write(&body_path, "{\"from\":\"file\"}").unwrap();
    let output_path = dir.path().join("posted.bin");

    run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "download",
        &url,
        "--method",
        "PATCH",
        "--data-file",
        body_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]));

    assert_eq!(fs::read(&output_path).unwrap(), data);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "PATCH");
    assert_eq!(requests[0].1, b"{\"from\":\"file\"}");
}

#[test]
fn queued_post_task_keeps_method_and_body() {
    let data: Vec<u8> = (0..16 * 1024).map(|index| (index % 223) as u8).collect();
    let (url, requests) = spawn_method_server(data.clone());

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");
    let output_path = dir.path().join("queued-post.bin");
    let config = path.to_str().unwrap();

    run(shaul().args([
        "--config",
        config,
        "--json",
        "add",
        &url,
        "--method",
        "POST",
        "--data",
        "queued-body",
        "--output",
        output_path.to_str().unwrap(),
    ]));
    run(shaul().args(["--config", config, "run", "--once"]));

    assert_eq!(fs::read(&output_path).unwrap(), data);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "POST");
    assert_eq!(requests[0].1, b"queued-body");
}

#[test]
fn json_output_redacts_request_body() {
    let data: Vec<u8> = (0..8 * 1024).map(|index| (index % 233) as u8).collect();
    let (url, _requests) = spawn_method_server(data);

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");

    let output = run(shaul().args([
        "--config",
        path.to_str().unwrap(),
        "--json",
        "add",
        &url,
        "--method",
        "POST",
        "--data",
        "body-secret",
    ]));

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("body-secret"), "body leaked: {text}");
    assert!(text.contains("<redacted>"), "{text}");
}

#[test]
fn http_method_is_rejected_for_non_http_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(&dir, "");

    let output = shaul()
        .args([
            "--config",
            path.to_str().unwrap(),
            "add",
            "--method",
            "POST",
            "ftp://example.com/file.bin",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("direct HTTP"), "stderr: {stderr}");
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

type RecordedRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Records the request method and body, then serves `data`.
fn spawn_method_server(data: Vec<u8>) -> (String, RecordedRequests) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let mut buffer = vec![0u8; 32 * 1024];
            let mut read = 0;
            while let Ok(count) = stream.read(&mut buffer[read..]) {
                if count == 0 {
                    break;
                }
                read += count;
                if buffer[..read]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break;
                }
                if read == buffer.len() {
                    break;
                }
            }
            let Some(header_end) = buffer[..read]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
            else {
                continue;
            };
            let request = String::from_utf8_lossy(&buffer[..header_end]).to_string();
            let mut lines = request.lines();
            let request_line = lines.next().unwrap_or_default();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let mut headers = HashMap::new();
            for line in lines {
                if let Some((name, value)) = line.split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let content_length = headers
                .get("content-length")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = buffer[header_end..read].to_vec();
            while body.len() < content_length {
                let mut chunk = vec![0u8; content_length - body.len()];
                let Ok(count) = stream.read(&mut chunk) else {
                    break;
                };
                if count == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..count]);
            }
            body.truncate(content_length);
            seen.lock().unwrap().push((method, body));

            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Length: {}\r\n\
                 Content-Disposition: attachment; filename=\"posted.bin\"\r\n\
                 Connection: close\r\n\
                 \r\n",
                data.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&data);
            let _ = stream.flush();
        }
    });
    (format!("http://{address}/api/download"), requests)
}

/// Serves a tiny HLS master/media playlist and two segments. When
/// `fail_segment` is set, the first request for that path gets a 500.
fn spawn_hls_server(fail_segment: Option<&str>) -> (String, thread::JoinHandle<()>) {
    const MASTER: &[u8] = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nvideo.m3u8\n";
    const MEDIA: &[u8] =
        b"#EXTM3U\n#EXTINF:4.0,\nseg-0.ts\n#EXTINF:4.0,\nseg-1.ts\n#EXT-X-ENDLIST\n";
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let fail_segment = fail_segment.map(str::to_string);
    let failed = Arc::new(AtomicBool::new(false));
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let fail_segment = fail_segment.clone();
            let failed = failed.clone();
            thread::spawn(move || {
                let mut request = [0_u8; 16 * 1024];
                let _ = stream.read(&mut request);
                let text = String::from_utf8_lossy(&request);
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/");
                if fail_segment.as_deref() == Some(path) && !failed.swap(true, Ordering::SeqCst) {
                    let _ = stream.write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 6\r\nConnection: close\r\n\r\nfailed",
                    );
                    return;
                }
                let (body, content_type) = match path {
                    "/master.m3u8" => (MASTER, "application/vnd.apple.mpegurl"),
                    "/video.m3u8" => (MEDIA, "application/vnd.apple.mpegurl"),
                    "/seg-0.ts" => (&b"AAAA"[..], "video/mp2t"),
                    "/seg-1.ts" => (&b"BBBB"[..], "video/mp2t"),
                    _ => (&b""[..], "application/octet-stream"),
                };
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            });
        }
    });
    (format!("http://{address}/master.m3u8"), handle)
}

/// Serves a tiny static DASH manifest with an init segment and two media
/// segments. When `fail_segment` is set, the first request for that path gets
/// a 500.
fn spawn_dash_server(fail_segment: Option<&str>) -> (String, thread::JoinHandle<()>) {
    const MANIFEST: &[u8] = br#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4">
<Representation id="v" bandwidth="1">
<SegmentTemplate timescale="1000" duration="4000" initialization="init.mp4" media="seg-$Number$.m4s"/>
</Representation>
</AdaptationSet>
</Period></MPD>"#;
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let fail_segment = fail_segment.map(str::to_string);
    let failed = Arc::new(AtomicBool::new(false));
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let fail_segment = fail_segment.clone();
            let failed = failed.clone();
            thread::spawn(move || {
                let mut request = [0_u8; 16 * 1024];
                let _ = stream.read(&mut request);
                let text = String::from_utf8_lossy(&request);
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/");
                if fail_segment.as_deref() == Some(path) && !failed.swap(true, Ordering::SeqCst) {
                    let _ = stream.write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 6\r\nConnection: close\r\n\r\nfailed",
                    );
                    return;
                }
                let (body, content_type) = match path {
                    "/master.mpd" => (MANIFEST, "application/dash+xml"),
                    "/init.mp4" => (&b"INIT"[..], "video/mp4"),
                    "/seg-1.m4s" => (&b"AAAA"[..], "video/mp4"),
                    "/seg-2.m4s" => (&b"BBBB"[..], "video/mp4"),
                    _ => (&b""[..], "application/octet-stream"),
                };
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            });
        }
    });
    (format!("http://{address}/master.mpd"), handle)
}

/// Serves the body in chunks with a delay between writes so a non-terminal
/// renderer has time to print an intermediate progress line.
fn spawn_streaming_file_server(
    data: Vec<u8>,
    chunks: usize,
    delay: Duration,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let data = data.clone();
            let chunk_size = data.len().div_ceil(chunks.max(1));
            thread::spawn(move || {
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
                for chunk in data.chunks(chunk_size) {
                    if stream.write_all(chunk).is_err() {
                        break;
                    }
                    let _ = stream.flush();
                    thread::sleep(delay);
                }
            });
        }
    });
    (format!("http://{address}/file.bin"), handle)
}

/// Serves a file while `serve_body` is true. Once disabled, probes still get
/// headers but no body, so only a cache hit can complete successfully.
fn spawn_cache_test_server(data: Vec<u8>) -> (String, Arc<AtomicBool>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let serve_body = Arc::new(AtomicBool::new(true));
    let serve = serve_body.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let data = data.clone();
            let serve = serve.clone();
            thread::spawn(move || {
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
                if serve.load(Ordering::SeqCst) {
                    let _ = stream.write_all(&data);
                }
                let _ = stream.flush();
            });
        }
    });
    (format!("http://{address}/file.bin"), serve_body)
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
