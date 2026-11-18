use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use sha2::{Digest, Sha256};
use shaul::{Auth, Config, Link, RequestOptions, TaskOptions, TaskState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    HuggingFace,
    ModelScope,
}

struct MockHub {
    addr: SocketAddr,
    file_requests: Arc<AtomicUsize>,
}

impl MockHub {
    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn file_requests(&self) -> usize {
        self.file_requests.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
struct MockFile {
    path: String,
    data: Vec<u8>,
    lfs: bool,
}

async fn spawn_hf(files: Vec<MockFile>, token: &str, commit: &str) -> MockHub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let files = Arc::new(files);
    let token = token.to_string();
    let commit = commit.to_string();
    let file_requests = Arc::new(AtomicUsize::new(0));
    let counter = file_requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let files = files.clone();
            let token = token.clone();
            let commit = commit.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                handle_hf(socket, files, token, commit, counter).await;
            });
        }
    });
    MockHub {
        addr,
        file_requests,
    }
}

async fn spawn_ms(files: Vec<MockFile>, token: &str) -> MockHub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let files = Arc::new(files);
    let token = token.to_string();
    let file_requests = Arc::new(AtomicUsize::new(0));
    let counter = file_requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let files = files.clone();
            let token = token.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                handle_ms(socket, files, token, counter).await;
            });
        }
    });
    MockHub {
        addr,
        file_requests,
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<(String, HashMap<String, String>)> {
    let mut buffer = vec![0u8; 32 * 1024];
    let mut read = 0;
    loop {
        let count = socket.read(&mut buffer[read..]).await.ok()?;
        if count == 0 {
            return None;
        }
        read += count;
        if buffer[..read]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
        if read == buffer.len() {
            return None;
        }
    }
    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
    let mut lines = text.lines();
    let request_line = lines.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Some((request_line, headers))
}

async fn handle_hf(
    mut socket: TcpStream,
    files: Arc<Vec<MockFile>>,
    token: String,
    commit: String,
    counter: Arc<AtomicUsize>,
) {
    let Some((request_line, headers)) = read_request(&mut socket).await else {
        return;
    };
    let expected_auth = format!("Bearer {token}");
    if headers.get("authorization").map(String::as_str) != Some(expected_auth.as_str()) {
        respond(
            &mut socket,
            "401 Unauthorized",
            vec![("X-Error-Code".to_string(), "RepoNotFound".to_string())],
            b"unauthorized",
        )
        .await;
        return;
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split('?').next().unwrap_or(target);

    if path == "/api/models/org/repo/tree/main" {
        let entries: Vec<_> = files
            .iter()
            .map(|file| {
                let sha = sha256_hex(&file.data);
                let oid: String = sha.chars().take(40).collect();
                if file.lfs {
                    json!({
                        "type": "file",
                        "path": file.path,
                        "size": file.data.len(),
                        "oid": oid,
                        "lfs": { "oid": sha, "size": file.data.len() }
                    })
                } else {
                    json!({
                        "type": "file",
                        "path": file.path,
                        "size": file.data.len(),
                        "oid": oid
                    })
                }
            })
            .collect();
        let body = serde_json::to_vec(&entries).unwrap();
        respond(
            &mut socket,
            "200 OK",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            &body,
        )
        .await;
        return;
    }

    let prefix = "/org/repo/resolve/main/";
    if let Some(relative) = path.strip_prefix(prefix) {
        let Some(file) = files.iter().find(|file| file.path == relative) else {
            respond(&mut socket, "404 Not Found", vec![], b"missing").await;
            return;
        };
        counter.fetch_add(1, Ordering::SeqCst);
        respond_file(&mut socket, &headers, &file.data, Some(&commit), None).await;
        return;
    }

    respond(&mut socket, "404 Not Found", vec![], b"missing").await;
}

async fn handle_ms(
    mut socket: TcpStream,
    files: Arc<Vec<MockFile>>,
    token: String,
    counter: Arc<AtomicUsize>,
) {
    let Some((request_line, headers)) = read_request(&mut socket).await else {
        return;
    };
    let expected_auth = format!("Bearer {token}");
    if headers.get("authorization").map(String::as_str) != Some(expected_auth.as_str()) {
        respond(
            &mut socket,
            "401 Unauthorized",
            vec![("X-Error-Code".to_string(), "RepoNotFound".to_string())],
            b"unauthorized",
        )
        .await;
        return;
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path == "/api/v1/models/org/repo/repo/files" {
        let entries: Vec<_> = files
            .iter()
            .map(|file| {
                json!({
                    "Path": file.path,
                    "Size": file.data.len(),
                    "Sha256": sha256_hex(&file.data),
                    "Type": "blob"
                })
            })
            .collect();
        let body = serde_json::to_vec(&json!({
            "Code": 200,
            "Data": { "Files": entries },
            "Message": "success",
            "Success": true
        }))
        .unwrap();
        respond(
            &mut socket,
            "200 OK",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            &body,
        )
        .await;
        return;
    }
    if path == "/api/v1/models/org/repo/repo" {
        let file_path = url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "FilePath")
            .map(|(_, value)| value.to_string())
            .unwrap_or_default();
        let Some(file) = files.iter().find(|file| file.path == file_path) else {
            respond(&mut socket, "404 Not Found", vec![], b"missing").await;
            return;
        };
        counter.fetch_add(1, Ordering::SeqCst);
        respond_file(&mut socket, &headers, &file.data, None, None).await;
        return;
    }
    respond(&mut socket, "404 Not Found", vec![], b"missing").await;
}

async fn respond_file(
    socket: &mut TcpStream,
    headers: &HashMap<String, String>,
    data: &[u8],
    commit: Option<&str>,
    _extra: Option<&str>,
) {
    let total = data.len() as u64;
    let sha = sha256_hex(data);
    let mut common = vec![
        ("ETag".to_string(), format!("\"{sha}\"")),
        ("X-Linked-Etag".to_string(), format!("\"{sha}\"")),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
        (
            "Content-Disposition".to_string(),
            "attachment; filename=\"file\"".to_string(),
        ),
    ];
    if let Some(commit) = commit {
        common.push(("X-Repo-Commit".to_string(), commit.to_string()));
    }
    if let Some(range) = headers.get("range") {
        if let Some((start, end)) = parse_range(range, total) {
            let body = &data[start as usize..=end as usize];
            common.push((
                "Content-Range".to_string(),
                format!("bytes {start}-{end}/{total}"),
            ));
            common.push(("Content-Length".to_string(), body.len().to_string()));
            respond(socket, "206 Partial Content", common, body).await;
            return;
        }
        respond(socket, "416 Range Not Satisfiable", common, b"").await;
        return;
    }
    common.push(("Content-Length".to_string(), total.to_string()));
    respond(socket, "200 OK", common, data).await;
}

async fn respond(
    socket: &mut TcpStream,
    status: &str,
    headers: Vec<(String, String)>,
    body: &[u8],
) {
    let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let value = value.trim().strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let start = start.trim().parse::<u64>().ok()?;
    let end = if end.trim().is_empty() {
        total.saturating_sub(1)
    } else {
        end.trim().parse::<u64>().ok()?
    };
    if start >= total {
        return None;
    }
    Some((start, end.min(total - 1)))
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn test_config(dir: &Path, provider: Provider, endpoint: &str) -> Config {
    let mut config = Config::default();
    config.download.connections = 4;
    config.download.segment_size = 512 * 1024;
    config.download.dir = dir.join("downloads");
    config.state_dir = Some(dir.join("state"));
    config.network.retries = 1;
    config.network.timeout_secs = 10;
    config.network.connect_timeout_secs = 5;
    config.hub.use_cache = true;
    match provider {
        Provider::HuggingFace => {
            config.hub.hf_endpoint = Some(endpoint.to_string());
            config.hub.hf_cache = Some(dir.join("hf"));
        }
        Provider::ModelScope => {
            config.hub.modelscope_endpoint = Some(endpoint.to_string());
            config.hub.modelscope_cache = Some(dir.join("ms"));
        }
    }
    std::fs::create_dir_all(&config.download.dir).unwrap();
    config
}

fn options(token: &str) -> TaskOptions {
    TaskOptions {
        cache: Some(true),
        request: RequestOptions {
            auth: Some(Auth::bearer(token)),
            ..RequestOptions::default()
        },
        ..TaskOptions::default()
    }
}

fn mock_files() -> Vec<MockFile> {
    vec![
        MockFile {
            path: "config.json".to_string(),
            data: br#"{"model":"test"}"#.to_vec(),
            lfs: false,
        },
        MockFile {
            path: "weights/model.bin".to_string(),
            data: (0..900 * 1024).map(|index| (index % 251) as u8).collect(),
            lfs: true,
        },
    ]
}

#[tokio::test]
async fn hf_repo_downloads_and_populates_compatible_cache() {
    let files = mock_files();
    let server = spawn_hf(files.clone(), "test-token", "commit123").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path(), Provider::HuggingFace, &server.endpoint());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let task = engine
        .add(
            Link::new(format!("{}/org/repo", server.endpoint())).unwrap(),
            config.download.dir.clone(),
            options("test-token"),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    for file in &files {
        let output = config.download.dir.join("repo").join(&file.path);
        assert_eq!(tokio::fs::read(&output).await.unwrap(), file.data);
    }

    let repo_cache = dir.path().join("hf/models--org--repo");
    let model_sha = sha256_hex(&files[1].data);
    let blob = repo_cache.join("blobs").join(&model_sha);
    assert_eq!(tokio::fs::read(&blob).await.unwrap(), files[1].data);
    assert!(
        repo_cache
            .join("blobs")
            .join(
                sha256_hex(&files[0].data)
                    .chars()
                    .take(40)
                    .collect::<String>()
            )
            .exists()
    );
    assert!(
        repo_cache
            .join("snapshots/commit123/weights/model.bin")
            .exists()
    );
    assert!(repo_cache.join("snapshots/commit123/config.json").exists());
    let reference = tokio::fs::read_to_string(repo_cache.join("refs/main"))
        .await
        .unwrap();
    assert_eq!(reference, "commit123");

    let first_requests = server.file_requests();
    tokio::fs::remove_dir_all(config.download.dir.join("repo"))
        .await
        .unwrap();
    let second = engine
        .add(
            Link::new(format!("{}/org/repo", server.endpoint())).unwrap(),
            config.download.dir.clone(),
            options("test-token"),
        )
        .await
        .unwrap();
    let second = engine.run_task(second.id).await.unwrap();
    assert_eq!(second.state, TaskState::Completed);
    assert_eq!(
        server.file_requests(),
        first_requests,
        "a cache hit must not re-request file bodies"
    );
    for file in &files {
        let output = config.download.dir.join("repo").join(&file.path);
        assert_eq!(tokio::fs::read(&output).await.unwrap(), file.data);
    }
}

#[tokio::test]
async fn hf_include_filter_limits_files() {
    let files = mock_files();
    let server = spawn_hf(files, "test-token", "commit123").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path(), Provider::HuggingFace, &server.endpoint());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let mut options = options("test-token");
    options.include = vec!["*.json".to_string()];
    let task = engine
        .add(
            Link::new(format!("{}/org/repo", server.endpoint())).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert!(config.download.dir.join("repo/config.json").exists());
    assert!(!config.download.dir.join("repo/weights/model.bin").exists());
}

#[tokio::test]
async fn hf_repo_without_cache_skips_cache() {
    let files = mock_files();
    let server = spawn_hf(files.clone(), "test-token", "commit123").await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path(), Provider::HuggingFace, &server.endpoint());
    config.hub.use_cache = false;
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let mut options = options("test-token");
    options.cache = Some(false);
    let task = engine
        .add(
            Link::new(format!("{}/org/repo", server.endpoint())).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    for file in &files {
        let output = config.download.dir.join("repo").join(&file.path);
        assert_eq!(tokio::fs::read(&output).await.unwrap(), file.data);
    }
    assert!(
        !dir.path().join("hf").exists(),
        "cache must not be created when disabled"
    );
}

#[tokio::test]
async fn modelscope_repo_downloads_and_caches() {
    let files = mock_files();
    let server = spawn_ms(files.clone(), "ms-token").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path(), Provider::ModelScope, &server.endpoint());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let task = engine
        .add(
            Link::new(format!("{}/models/org/repo", server.endpoint())).unwrap(),
            config.download.dir.clone(),
            options("ms-token"),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    for file in &files {
        let output = config.download.dir.join("repo").join(&file.path);
        assert_eq!(tokio::fs::read(&output).await.unwrap(), file.data);
    }
    let repo_cache = dir.path().join("ms/models--org--repo");
    let blob = repo_cache.join("blobs").join(sha256_hex(&files[1].data));
    assert_eq!(tokio::fs::read(&blob).await.unwrap(), files[1].data);
    assert!(
        repo_cache
            .join("snapshots/master/weights/model.bin")
            .exists()
    );
}

#[tokio::test]
async fn hf_single_file_link_uses_hub_backend() {
    let files = mock_files();
    let server = spawn_hf(files.clone(), "test-token", "commit123").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path(), Provider::HuggingFace, &server.endpoint());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let task = engine
        .add(
            Link::new(format!(
                "{}/org/repo/resolve/main/weights/model.bin",
                server.endpoint()
            ))
            .unwrap(),
            config.download.dir.clone(),
            options("test-token"),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("repo/weights/model.bin"))
            .await
            .unwrap(),
        files[1].data
    );
}

#[tokio::test]
async fn hub_auth_failure_is_reported() {
    let files = mock_files();
    let server = spawn_hf(files, "test-token", "commit123").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path(), Provider::HuggingFace, &server.endpoint());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let error = engine
        .add(
            Link::new(format!("{}/org/repo", server.endpoint())).unwrap(),
            config.download.dir.clone(),
            options("wrong-token"),
        )
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("401") || message.contains("RepoNotFound"),
        "unexpected error: {message}"
    );
}
