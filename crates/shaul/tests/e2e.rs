use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use shaul::{
    ByteRange, CancellationToken, Checksum, ChecksumAlgorithm, Config, DownloadBackend,
    DownloadOutcome, DownloadRequest, EventSink, FileResume, Header, HttpBackend, Link, Resource,
    ResourceFile, ResourceKind, ResumeCheckpoint, TaskOptions, TaskResume, TaskState, Telemetry,
    Validator, metrics, part_path,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RangeMode {
    Supported,
    Ignored,
    IgnoredAfterProbe,
    /// Some real servers (ModelScope) answer a ranged request with 200 plus
    /// a Content-Range header instead of 206.
    Partial200,
}

struct TestServer {
    addr: SocketAddr,
    filename: String,
    max_active: Arc<AtomicUsize>,
}

impl TestServer {
    async fn spawn(
        data: Vec<u8>,
        mode: RangeMode,
        required_header: Option<String>,
        filename: &str,
    ) -> Self {
        Self::spawn_with_delay(data, mode, required_header, filename, Duration::ZERO).await
    }

    async fn spawn_with_delay(
        data: Vec<u8>,
        mode: RangeMode,
        required_header: Option<String>,
        filename: &str,
        delay: Duration,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let data = Arc::new(data);
        let server_filename = filename.to_string();
        let filename = server_filename.clone();
        let seen = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let server_active = active.clone();
        let server_max_active = max_active.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let data = data.clone();
                let filename = filename.clone();
                let required_header = required_header.clone();
                let seen = seen.clone();
                let active = server_active.clone();
                let max_active = server_max_active.clone();
                tokio::spawn(async move {
                    handle(
                        socket,
                        data,
                        mode,
                        required_header,
                        filename,
                        seen,
                        active,
                        max_active,
                        delay,
                    )
                    .await;
                });
            }
        });
        Self {
            addr,
            filename: server_filename,
            max_active,
        }
    }

    fn file_url(&self) -> String {
        format!("http://{}/file", self.addr)
    }

    fn redirect_url(&self) -> String {
        format!("http://{}/redirect", self.addr)
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }
}

struct ActiveGuard {
    active: Arc<AtomicUsize>,
}

impl ActiveGuard {
    fn new(active: Arc<AtomicUsize>, max_active: &AtomicUsize) -> Self {
        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
        max_active.fetch_max(current, Ordering::SeqCst);
        Self { active }
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ProxyServer {
    addr: SocketAddr,
    requests: Arc<AtomicUsize>,
}

impl ProxyServer {
    async fn spawn(data: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let data = Arc::new(data);
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let data = data.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    handle_proxy(socket, data, seen).await;
                });
            }
        });
        Self { addr, requests }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

async fn handle_proxy(mut socket: TcpStream, data: Arc<Vec<u8>>, seen: Arc<AtomicUsize>) {
    let mut buffer = vec![0u8; 32 * 1024];
    let mut read = 0;
    loop {
        let Ok(count) = socket.read(&mut buffer[read..]).await else {
            return;
        };
        if count == 0 {
            return;
        }
        read += count;
        if buffer[..read]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
        if read == buffer.len() {
            return;
        }
    }
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default();
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    assert!(
        target.starts_with("http://"),
        "proxy received a non-absolute target: {target}"
    );
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    seen.fetch_add(1, Ordering::SeqCst);

    let total = data.len() as u64;
    let etag = format!("\"{}\"", sha256_hex(&data));
    let common = vec![
        ("ETag".to_string(), etag),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
        (
            "Content-Disposition".to_string(),
            "attachment; filename=\"proxied.bin\"".to_string(),
        ),
    ];
    if let Some(range) = headers.get("range")
        && let Some((start, end)) = parse_range(range, total)
    {
        let body = &data[start as usize..=end as usize];
        let mut response_headers = common;
        response_headers.push((
            "Content-Range".to_string(),
            format!("bytes {start}-{end}/{total}"),
        ));
        response_headers.push(("Content-Length".to_string(), body.len().to_string()));
        respond(&mut socket, "206 Partial Content", response_headers, body).await;
        return;
    }

    let mut response_headers = common;
    response_headers.push(("Content-Length".to_string(), total.to_string()));
    respond(&mut socket, "200 OK", response_headers, &data).await;
}

#[allow(clippy::too_many_arguments)]
async fn handle(
    mut socket: TcpStream,
    data: Arc<Vec<u8>>,
    mode: RangeMode,
    required_header: Option<String>,
    filename: String,
    seen: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    delay: Duration,
) {
    let _active = ActiveGuard::new(active, &max_active);
    let mut buffer = vec![0u8; 32 * 1024];
    let mut read = 0;
    loop {
        let Ok(count) = socket.read(&mut buffer[read..]).await else {
            return;
        };
        if count == 0 {
            return;
        }
        read += count;
        if buffer[..read]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
        if read == buffer.len() {
            return;
        }
    }
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    if let Some(required) = required_header
        && headers
            .get(&required.to_ascii_lowercase())
            .map(String::as_str)
            != Some("yes")
    {
        respond(&mut socket, "403 Forbidden", vec![], b"missing header").await;
        return;
    }

    if path == "/redirect" {
        let response = "HTTP/1.1 302 Found\r\nConnection: close\r\nLocation: /file\r\nContent-Length: 0\r\n\r\n";
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
        return;
    }

    if path != "/file" {
        respond(&mut socket, "404 Not Found", vec![], b"not found").await;
        return;
    }

    let index = seen.fetch_add(1, Ordering::SeqCst);
    let etag = format!("\"{}\"", sha256_hex(&data));
    let total = data.len() as u64;
    let range = headers.get("range").cloned();
    let effective_mode = match mode {
        RangeMode::IgnoredAfterProbe if index > 0 => RangeMode::Ignored,
        other => other,
    };

    let common = vec![
        ("ETag".to_string(), etag),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
        (
            "Content-Disposition".to_string(),
            format!("attachment; filename=\"{filename}\""),
        ),
    ];

    if effective_mode == RangeMode::Ignored || total == 0 {
        let mut response_headers = common;
        response_headers.push(("Content-Length".to_string(), total.to_string()));
        respond(&mut socket, "200 OK", response_headers, &data).await;
        return;
    }

    if let Some(range) = range {
        if let Some((start, end)) = parse_range(&range, total) {
            let body = &data[start as usize..=end as usize];
            let mut response_headers = common;
            response_headers.push((
                "Content-Range".to_string(),
                format!("bytes {start}-{end}/{total}"),
            ));
            response_headers.push(("Content-Length".to_string(), body.len().to_string()));
            let status = if effective_mode == RangeMode::Partial200 {
                "200 OK"
            } else {
                "206 Partial Content"
            };
            respond(&mut socket, status, response_headers, body).await;
            return;
        }
        let mut response_headers = common;
        response_headers.push(("Content-Range".to_string(), format!("bytes */{total}")));
        respond(
            &mut socket,
            "416 Range Not Satisfiable",
            response_headers,
            b"",
        )
        .await;
        return;
    }

    let mut response_headers = common;
    response_headers.push(("Content-Length".to_string(), total.to_string()));
    respond(&mut socket, "200 OK", response_headers, &data).await;
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

fn checksum(data: &[u8]) -> Checksum {
    Checksum::new(ChecksumAlgorithm::Sha256, sha256_hex(data)).unwrap()
}

fn test_config(dir: &std::path::Path) -> Config {
    let mut config = Config::default();
    config.download.connections = 4;
    config.download.segment_size = 512 * 1024;
    config.download.dir = dir.join("downloads");
    config.state_dir = Some(dir.join("state"));
    config.network.retries = 2;
    config.network.timeout_secs = 10;
    config.network.connect_timeout_secs = 5;
    std::fs::create_dir_all(&config.download.dir).unwrap();
    config
}

fn test_data(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

#[tokio::test]
async fn downloads_with_segments_and_verifies() {
    let data = test_data(3 * 1024 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Supported, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();

    let options = TaskOptions {
        checksum: Some(checksum(&data)),
        connections: Some(4),
        ..TaskOptions::default()
    };
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    let output = config.download.dir.join(&server.filename);
    assert_eq!(tokio::fs::read(&output).await.unwrap(), data);
    assert!(!tokio::fs::try_exists(part_path(&output)).await.unwrap());
}

#[tokio::test]
async fn emits_task_telemetry() {
    let data = test_data(512 * 1024);
    let server = TestServer::spawn(data, RangeMode::Supported, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let telemetry = Arc::new(RecordingTelemetry::default());
    let engine = engine.with_telemetry(telemetry.clone());

    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    let counters = telemetry.counters.lock().unwrap();
    assert!(
        counters
            .iter()
            .any(|(name, value, _)| { name == metrics::TASKS_ADDED && *value == 1 })
    );
    assert!(
        counters
            .iter()
            .any(|(name, value, _)| { name == metrics::TASKS_STARTED && *value == 1 })
    );
    assert!(
        counters
            .iter()
            .any(|(name, value, _)| { name == metrics::TASKS_COMPLETED && *value == 1 })
    );
    assert!(
        counters
            .iter()
            .any(|(name, value, _)| { name == metrics::BYTES_DOWNLOADED && *value == 512 * 1024 })
    );
    let histograms = telemetry.histograms.lock().unwrap();
    assert!(
        histograms
            .iter()
            .any(|(name, _)| { name == metrics::TASK_DURATION_SECONDS })
    );
}

type RecordedCounter = (String, u64, Vec<(String, String)>);

#[derive(Default)]
struct RecordingTelemetry {
    counters: Mutex<Vec<RecordedCounter>>,
    histograms: Mutex<Vec<(String, f64)>>,
}

impl Telemetry for RecordingTelemetry {
    fn counter(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
        self.counters.lock().unwrap().push((
            name.to_string(),
            value,
            labels
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        ));
    }

    fn histogram(&self, name: &str, value: f64, _labels: &[(&str, &str)]) {
        self.histograms
            .lock()
            .unwrap()
            .push((name.to_string(), value));
    }
}

#[tokio::test]
async fn resumes_from_partial_part_file() {
    let data = test_data(2 * 1024 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Supported, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, store) = shaul::default_engine(config.clone()).unwrap();
    let url = server.file_url();
    let task = engine
        .add(
            Link::new(&url).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();

    let half = data.len() / 2;
    let output = config.download.dir.join(&server.filename);
    tokio::fs::write(part_path(&output), &data[..half])
        .await
        .unwrap();

    let sha = sha256_hex(&data);
    let mut resume = TaskResume::default();
    resume.files.insert(
        url.clone(),
        FileResume {
            url: url.clone(),
            final_path: server.filename.clone(),
            part_path: format!("{}.shaul.part", server.filename),
            validator: Validator {
                etag: Some(format!("\"{sha}\"")),
                last_modified: None,
                size: Some(data.len() as u64),
            },
            total_size: Some(data.len() as u64),
            segment_size: 512 * 1024,
            completed: vec![ByteRange::new(0, half as u64).unwrap()],
            checksum: Some(checksum(&data)),
            metadata: Default::default(),
            updated_at_ms: 0,
        },
    );
    store
        .update(
            task.id,
            Box::new(move |task| {
                task.resume = Some(resume);
                Ok(())
            }),
        )
        .await
        .unwrap();

    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(tokio::fs::read(&output).await.unwrap(), data);
}

#[tokio::test]
async fn falls_back_to_single_stream_when_range_is_ignored() {
    let data = test_data(1024 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Ignored, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join(&server.filename))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn falls_back_when_a_ranged_request_returns_200() {
    let data = test_data(2 * 1024 * 1024);
    let server =
        TestServer::spawn(data.clone(), RangeMode::IgnoredAfterProbe, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join(&server.filename))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn handles_200_with_content_range() {
    let data = test_data(2 * 1024 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Partial200, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join(&server.filename))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn sends_custom_headers() {
    let data = test_data(256 * 1024);
    let server = TestServer::spawn(
        data.clone(),
        RangeMode::Supported,
        Some("x-test".to_string()),
        "mock.bin",
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let mut options = TaskOptions::default();
    options.request.headers = vec![Header::new("X-Test", "yes").unwrap()];
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
}

#[tokio::test]
async fn follows_redirects() {
    let data = test_data(256 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Supported, None, "mock.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.redirect_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join(&server.filename))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn extracts_archives_after_verification() {
    let archive = tar_gz(b"hello");
    let server =
        TestServer::spawn(archive.clone(), RangeMode::Supported, None, "data.tar.gz").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let options = TaskOptions {
        extract: Some(true),
        checksum: Some(checksum(&archive)),
        ..TaskOptions::default()
    };
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read_to_string(config.download.dir.join("data/hello.txt"))
            .await
            .unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn applies_a_global_bandwidth_limit() {
    let data = test_data(256 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Supported, None, "limited.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.download.rate_limit = Some(1024 * 1024);
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let options = TaskOptions {
        connections: Some(1),
        ..TaskOptions::default()
    };
    let started = Instant::now();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert!(
        started.elapsed().as_millis() >= 180,
        "global rate limit was not applied"
    );
}

#[tokio::test]
async fn applies_a_per_task_bandwidth_limit() {
    let data = test_data(256 * 1024);
    let server = TestServer::spawn(data.clone(), RangeMode::Supported, None, "limited.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let options = TaskOptions {
        connections: Some(1),
        rate_limit: Some(1024 * 1024),
        ..TaskOptions::default()
    };
    let started = Instant::now();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert!(
        started.elapsed().as_millis() >= 180,
        "per-task rate limit was not applied"
    );
}

#[tokio::test]
async fn downloads_through_an_http_proxy() {
    let data = test_data(800 * 1024);
    let proxy = ProxyServer::spawn(data.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.network.proxy = Some(proxy.url());
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let options = TaskOptions {
        connections: Some(2),
        ..TaskOptions::default()
    };
    let task = engine
        .add(
            Link::new("http://proxy-only.invalid/file.bin").unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("proxied.bin"))
            .await
            .unwrap(),
        data
    );
    assert!(proxy.requests() >= 2, "expected probe and range requests");
}

struct TestCheckpoint {
    resume: Arc<Mutex<TaskResume>>,
}

#[async_trait::async_trait]
impl ResumeCheckpoint for TestCheckpoint {
    async fn save(&self, resume: &TaskResume) -> shaul::Result<()> {
        *self.resume.lock().unwrap() = resume.clone();
        Ok(())
    }
}

#[tokio::test]
async fn enforces_the_global_connection_budget() {
    let data = test_data(1024 * 1024);
    let server = TestServer::spawn_with_delay(
        data.clone(),
        RangeMode::Supported,
        None,
        "budget.bin",
        Duration::from_millis(25),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.download.global = 2;
    config.download.per_host_active = 16;
    config.download.connections = 4;
    config.download.segment_size = 128 * 1024;
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                connections: Some(4),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join(&server.filename))
            .await
            .unwrap(),
        data
    );
    assert!(
        server.max_active() <= 2,
        "global budget exceeded: {} concurrent requests",
        server.max_active()
    );
}

#[tokio::test]
async fn enforces_the_per_host_connection_budget() {
    let data = test_data(512 * 1024);
    let server = TestServer::spawn_with_delay(
        data.clone(),
        RangeMode::Supported,
        None,
        "host-budget.bin",
        Duration::from_millis(20),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.download.global = 64;
    config.download.per_host_active = 1;
    config.download.connections = 4;
    config.download.segment_size = 128 * 1024;
    let (engine, _) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.file_url()).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                connections: Some(4),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join(&server.filename))
            .await
            .unwrap(),
        data
    );
    assert!(
        server.max_active() <= 1,
        "per-host budget exceeded: {} concurrent requests",
        server.max_active()
    );
}

#[tokio::test]
async fn downloads_multiple_http_files_concurrently() {
    let first_data = test_data(192 * 1024);
    let second_data = test_data(256 * 1024);
    let first =
        TestServer::spawn(first_data.clone(), RangeMode::Supported, None, "second.bin").await;
    let second =
        TestServer::spawn(second_data.clone(), RangeMode::Supported, None, "first.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let options = config
        .resolve(
            &config.download.dir,
            &TaskOptions {
                file_jobs: Some(2),
                connections: Some(2),
                ..TaskOptions::default()
            },
        )
        .unwrap();
    let resource = Resource {
        kind: ResourceKind::Http,
        name: "multi".to_string(),
        files: vec![
            ResourceFile {
                path: "a.bin".to_string(),
                url: Url::parse(&first.file_url()).unwrap(),
                size: None,
                checksum: None,
                headers: Vec::new(),
                metadata: Default::default(),
                use_response_filename: false,
            },
            ResourceFile {
                path: "b.bin".to_string(),
                url: Url::parse(&second.file_url()).unwrap(),
                size: None,
                checksum: None,
                headers: Vec::new(),
                metadata: Default::default(),
                use_response_filename: false,
            },
        ],
        request: Default::default(),
        checksum: None,
        revision: None,
        metadata: Default::default(),
    };
    let shared = Arc::new(Mutex::new(TaskResume::default()));
    let (events, _receiver) = EventSink::channel(32);
    let request = DownloadRequest {
        task_id: shaul::TaskId::new_v4(),
        resource,
        destination: config.download.dir.clone(),
        options,
        resume: None,
        cancel: CancellationToken::new(),
        events,
        checkpoint: Arc::new(TestCheckpoint {
            resume: shared.clone(),
        }),
    };
    let backend = HttpBackend::new(&config).unwrap();
    let outcome = backend.download(request).await.unwrap();
    let DownloadOutcome::Completed(files) = outcome else {
        panic!("expected completed HTTP downloads");
    };
    assert_eq!(files.len(), 2);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("a.bin"))
            .await
            .unwrap(),
        first_data
    );
    assert_eq!(
        tokio::fs::read(config.download.dir.join("b.bin"))
            .await
            .unwrap(),
        second_data
    );
    let resume = shared.lock().unwrap();
    assert_eq!(resume.files.len(), 2);
    assert!(resume.files.values().all(|file| file.is_complete()));
}

fn tar_gz(contents: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "hello.txt", contents)
            .unwrap();
        builder.finish().unwrap();
    }
    encoder.finish().unwrap()
}
