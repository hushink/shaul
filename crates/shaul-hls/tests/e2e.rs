use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use shaul_core::{Config, DownloadEngine, Link, SidecarStore, TaskOptions, TaskState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MASTER: &str = "#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=640x360
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080
high.m3u8
";

const HIGH: &str = "#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXTINF:4.0,
seg-000.ts
#EXTINF:4.0,
seg-001.ts
#EXTINF:4.0,
seg-002.ts
#EXT-X-ENDLIST
";

const LOW: &str = "#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXTINF:4.0,
low-000.ts
#EXT-X-ENDLIST
";

const FMP4: &str = "#EXTM3U
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:4.0,
fseg-000.m4s
#EXTINF:4.0,
fseg-001.m4s
#EXT-X-ENDLIST
";

const LIVE: &str = "#EXTM3U
#EXTINF:4.0,
seg-000.ts
";

struct HlsServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
}

impl HlsServer {
    async fn spawn(fail_once: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let fail_once = fail_once.map(str::to_string);
        let failed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let requests = server_requests.clone();
                let fail_once = fail_once.clone();
                let failed = failed.clone();
                tokio::spawn(async move {
                    handle(socket, requests, fail_once, failed).await;
                });
            }
        });
        Self { addr, requests }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn take_requests(&self) -> Vec<String> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

async fn handle(
    mut socket: TcpStream,
    requests: Arc<Mutex<Vec<String>>>,
    fail_once: Option<String>,
    failed: Arc<AtomicBool>,
) {
    let mut buffer = vec![0u8; 16 * 1024];
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
    let request = String::from_utf8_lossy(&buffer[..read]);
    let request_line = request.lines().next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    requests.lock().unwrap().push(path.clone());

    if fail_once.as_deref() == Some(path.as_str()) && !failed.swap(true, Ordering::SeqCst) {
        respond(&mut socket, "500 Internal Server Error", b"failed").await;
        return;
    }

    match path.as_str() {
        "/master.m3u8" => respond(&mut socket, "200 OK", MASTER.as_bytes()).await,
        "/high.m3u8" => respond(&mut socket, "200 OK", HIGH.as_bytes()).await,
        "/low.m3u8" => respond(&mut socket, "200 OK", LOW.as_bytes()).await,
        "/fmpeg.m3u8" => respond(&mut socket, "200 OK", FMP4.as_bytes()).await,
        "/live.m3u8" => respond(&mut socket, "200 OK", LIVE.as_bytes()).await,
        "/seg-000.ts" => respond(&mut socket, "200 OK", &[1u8; 4096]).await,
        "/seg-001.ts" => respond(&mut socket, "200 OK", &[2u8; 4096]).await,
        "/seg-002.ts" => respond(&mut socket, "200 OK", &[3u8; 4096]).await,
        "/low-000.ts" => respond(&mut socket, "200 OK", &[9u8; 1024]).await,
        "/init.mp4" => respond(&mut socket, "200 OK", b"INIT").await,
        "/fseg-000.m4s" => respond(&mut socket, "200 OK", b"A").await,
        "/fseg-001.m4s" => respond(&mut socket, "200 OK", b"B").await,
        _ => respond(&mut socket, "404 Not Found", b"not found").await,
    }
}

async fn respond(socket: &mut TcpStream, status: &str, body: &[u8]) {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(headers.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}

fn test_config(dir: &Path) -> Config {
    let mut config = Config::default();
    config.state_dir = Some(dir.join("state"));
    config.download.dir = dir.join("downloads");
    config.download.connections = 2;
    config.network.timeout_secs = 10;
    config.network.connect_timeout_secs = 5;
    config.network.retries = 0;
    std::fs::create_dir_all(&config.download.dir).unwrap();
    config
}

fn engine(config: &Config) -> DownloadEngine {
    let router = shaul_hls::default_router(config).unwrap();
    let store = Arc::new(SidecarStore::new(config.state_dir().unwrap()).unwrap());
    DownloadEngine::new(Arc::new(router), store, config.clone())
}

#[tokio::test]
async fn downloads_a_master_playlist_and_concatenates_the_highest_variant() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/master.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    let output = config.download.dir.join("master.ts");
    let expected: Vec<u8> = [1u8; 4096]
        .into_iter()
        .chain([2u8; 4096])
        .chain([3u8; 4096])
        .collect();
    assert_eq!(tokio::fs::read(&output).await.unwrap(), expected);
    assert!(
        !tokio::fs::try_exists(config.download.dir.join("master.ts.shaul.hls.parts"))
            .await
            .unwrap()
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/high.m3u8"));
    assert!(!requests.iter().any(|path| path == "/low.m3u8"));
    assert!(requests.iter().any(|path| path == "/seg-000.ts"));
}

#[tokio::test]
async fn include_globs_select_a_lower_variant() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/master.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                include: vec!["*360*".to_string()],
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("master.ts"))
            .await
            .unwrap(),
        vec![9u8; 1024]
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/low.m3u8"));
    assert!(!requests.iter().any(|path| path == "/high.m3u8"));
}

#[tokio::test]
async fn downloads_fmp4_with_an_init_segment() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/fmpeg.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    let output = config.download.dir.join("fmpeg.mp4");
    assert_eq!(tokio::fs::read(&output).await.unwrap(), b"INITAB".to_vec());
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/init.mp4"));
}

#[tokio::test]
async fn rejects_live_playlists_at_add_time() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let result = engine
        .add(
            Link::new(server.url("/live.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn resumes_from_completed_segment_files() {
    let server = HlsServer::spawn(Some("/seg-002.ts")).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/master.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                connections: Some(1),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let result = engine.run_task(task.id).await;
    assert!(result.is_err());
    let task = engine.status(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Failed);

    let temp_dir = config.download.dir.join("master.ts.shaul.hls.parts");
    let mut completed = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&temp_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with("segment-") {
                completed.push(name.to_string());
            }
        }
    }
    assert!(
        !completed.is_empty(),
        "at least one segment should have completed before the failure"
    );

    let _ = server.take_requests();
    engine.resume(task.id).await.unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("master.ts"))
            .await
            .unwrap(),
        [1u8; 4096]
            .into_iter()
            .chain([2u8; 4096])
            .chain([3u8; 4096])
            .collect::<Vec<_>>()
    );

    let requests = server.take_requests();
    for name in &completed {
        let index = name
            .strip_prefix("segment-")
            .and_then(|name| name.strip_suffix(".bin"))
            .and_then(|name| name.parse::<usize>().ok())
            .unwrap();
        let path = format!("/seg-{index:03}.ts");
        assert!(
            !requests.iter().any(|request| request == &path),
            "{path} should not be requested again"
        );
    }
}
