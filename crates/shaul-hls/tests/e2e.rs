use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aes::Aes128;
use cbc::cipher::generic_array::GenericArray;
use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use shaul_core::{Config, DownloadEngine, Link, SidecarStore, TaskOptions, TaskState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const AES_KEY: [u8; 16] = [0x11; 16];
const ROT_KEY1: [u8; 16] = [0x22; 16];
const ROT_KEY2: [u8; 16] = [0x33; 16];
const AES_IV: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
const AES_PLAIN_0: &[u8] = b"segment-zero";
const AES_PLAIN_1: &[u8] = b"segment-one!";
const ROT_PLAIN_0: &[u8] = b"rotated-zero";
const ROT_PLAIN_1: &[u8] = b"rotated-one!";
const INIT_PLAIN: &[u8] = b"INIT-DATA";
const MAP_PLAIN: &[u8] = b"map-segment";

const AES_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-KEY:METHOD=AES-128,URI="key.bin",IV=0x000102030405060708090a0b0c0d0e0f
#EXTINF:4.0,
enc-0.ts
#EXTINF:4.0,
enc-1.ts
#EXT-X-ENDLIST
"#;

const ROTATION_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-MEDIA-SEQUENCE:100
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-KEY:METHOD=AES-128,URI="rot-key1.bin"
#EXTINF:4.0,
rot-0.ts
#EXT-X-KEY:METHOD=AES-128,URI="rot-key2.bin"
#EXTINF:4.0,
rot-1.ts
#EXT-X-ENDLIST
"#;

const AES_MAP_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-KEY:METHOD=AES-128,URI="key.bin",IV=0x000102030405060708090a0b0c0d0e0f
#EXT-X-MAP:URI="enc-init.mp4"
#EXTINF:4.0,
map-seg-0.ts
#EXT-X-ENDLIST
"#;

const SAMPLE_AES_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="key.bin"
#EXTINF:4.0,
enc-0.ts
#EXT-X-ENDLIST
"#;

const BYTERANGE_DATA: &[u8] = b"AAAAABBBBBCCCCC";

const BYTERANGE_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-BYTERANGE:5@0
#EXTINF:4.0,
byterange.bin
#EXT-X-BYTERANGE:5
#EXTINF:4.0,
byterange.bin
#EXT-X-ENDLIST
"#;

const BYTERANGE_MAP_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-MAP:URI="byterange.bin",BYTERANGE="2@0"
#EXT-X-BYTERANGE:3@2
#EXTINF:4.0,
byterange.bin
#EXT-X-ENDLIST
"#;

const RANGE_IGNORED_PLAYLIST: &str = r#"#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-BYTERANGE:5@0
#EXTINF:4.0,
full.bin
#EXT-X-BYTERANGE:5
#EXTINF:4.0,
full.bin
#EXT-X-ENDLIST
"#;

fn encrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
    cbc::Encryptor::<Aes128>::new(GenericArray::from_slice(key), GenericArray::from_slice(iv))
        .encrypt_padded_vec_mut::<Pkcs7>(data)
}

fn sequence_iv(sequence: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&sequence.to_be_bytes());
    iv
}

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

    let mut headers = HashMap::new();
    for line in request.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    if path == "/byterange.bin" {
        let total = BYTERANGE_DATA.len() as u64;
        if let Some((start, end)) = headers
            .get("range")
            .and_then(|value| parse_range(value, total))
        {
            let body = &BYTERANGE_DATA[start as usize..=end as usize];
            respond_range(&mut socket, body, start, end, total).await;
        } else {
            respond(&mut socket, "200 OK", BYTERANGE_DATA).await;
        }
        return;
    }

    let body: Vec<u8> = match path.as_str() {
        "/master.m3u8" => MASTER.as_bytes().to_vec(),
        "/high.m3u8" => HIGH.as_bytes().to_vec(),
        "/low.m3u8" => LOW.as_bytes().to_vec(),
        "/fmpeg.m3u8" => FMP4.as_bytes().to_vec(),
        "/live.m3u8" => LIVE.as_bytes().to_vec(),
        "/seg-000.ts" => vec![1u8; 4096],
        "/seg-001.ts" => vec![2u8; 4096],
        "/seg-002.ts" => vec![3u8; 4096],
        "/low-000.ts" => vec![9u8; 1024],
        "/init.mp4" => b"INIT".to_vec(),
        "/fseg-000.m4s" => b"A".to_vec(),
        "/fseg-001.m4s" => b"B".to_vec(),
        "/aes.m3u8" => AES_PLAYLIST.as_bytes().to_vec(),
        "/key.bin" => AES_KEY.to_vec(),
        "/enc-0.ts" => encrypt(AES_PLAIN_0, &AES_KEY, &AES_IV),
        "/enc-1.ts" => encrypt(AES_PLAIN_1, &AES_KEY, &AES_IV),
        "/rot.m3u8" => ROTATION_PLAYLIST.as_bytes().to_vec(),
        "/rot-key1.bin" => ROT_KEY1.to_vec(),
        "/rot-key2.bin" => ROT_KEY2.to_vec(),
        "/rot-0.ts" => encrypt(ROT_PLAIN_0, &ROT_KEY1, &sequence_iv(100)),
        "/rot-1.ts" => encrypt(ROT_PLAIN_1, &ROT_KEY2, &sequence_iv(101)),
        "/aes-map.m3u8" => AES_MAP_PLAYLIST.as_bytes().to_vec(),
        "/enc-init.mp4" => encrypt(INIT_PLAIN, &AES_KEY, &AES_IV),
        "/map-seg-0.ts" => encrypt(MAP_PLAIN, &AES_KEY, &AES_IV),
        "/sample-aes.m3u8" => SAMPLE_AES_PLAYLIST.as_bytes().to_vec(),
        "/byterange.m3u8" => BYTERANGE_PLAYLIST.as_bytes().to_vec(),
        "/byterange-map.m3u8" => BYTERANGE_MAP_PLAYLIST.as_bytes().to_vec(),
        "/range-ignored.m3u8" => RANGE_IGNORED_PLAYLIST.as_bytes().to_vec(),
        "/full.bin" => BYTERANGE_DATA.to_vec(),
        _ => {
            respond(&mut socket, "404 Not Found", b"not found").await;
            return;
        }
    };
    respond(&mut socket, "200 OK", &body).await;
}

async fn respond_range(socket: &mut TcpStream, body: &[u8], start: u64, end: u64, total: u64) {
    let headers = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(headers.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let range = value.trim().strip_prefix("bytes=")?;
    let (start, end) = range.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() {
        total.saturating_sub(1)
    } else {
        end.parse().ok()?
    };
    (start <= end && end < total).then_some((start, end))
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

struct LiveHlsServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
}

impl LiveHlsServer {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let fetches = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let requests = server_requests.clone();
                let fetches = fetches.clone();
                tokio::spawn(async move {
                    handle_live(socket, requests, fetches).await;
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

fn live_playlist(sequence: u64, end_list: bool) -> String {
    let mut text = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n\
         #EXT-X-KEY:METHOD=AES-128,URI=\"live-key.bin\",\
         IV=0x000102030405060708090a0b0c0d0e0f\n\
         #EXTINF:4.0,\nlive-{sequence}.ts\n#EXTINF:4.0,\nlive-{}.ts\n",
        sequence + 1
    );
    if end_list {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    text
}

fn live_plaintext(sequence: u64) -> Vec<u8> {
    format!("L{sequence}").into_bytes()
}

async fn handle_live(
    mut socket: TcpStream,
    requests: Arc<Mutex<Vec<String>>>,
    fetches: Arc<AtomicUsize>,
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
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    requests.lock().unwrap().push(path.clone());

    if path == "/live.m3u8" {
        let fetch = fetches.fetch_add(1, Ordering::SeqCst) + 1;
        let (sequence, end_list) = match fetch {
            1 => (100, false),
            2 => (102, false),
            _ => (104, true),
        };
        respond(
            &mut socket,
            "200 OK",
            live_playlist(sequence, end_list).as_bytes(),
        )
        .await;
        return;
    }
    if path == "/live-key.bin" {
        respond(&mut socket, "200 OK", &AES_KEY).await;
        return;
    }
    for sequence in 100..=105 {
        if path == format!("/live-{sequence}.ts") {
            let encrypted = encrypt(&live_plaintext(sequence), &AES_KEY, &AES_IV);
            respond(&mut socket, "200 OK", &encrypted).await;
            return;
        }
    }
    respond(&mut socket, "404 Not Found", b"not found").await;
}

fn live_expected(sequences: &[u64]) -> Vec<u8> {
    sequences
        .iter()
        .flat_map(|sequence| live_plaintext(*sequence))
        .collect()
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
async fn downloads_an_aes_128_playlist_with_an_explicit_iv() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/aes.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("aes.ts"))
            .await
            .unwrap(),
        [AES_PLAIN_0, AES_PLAIN_1].concat()
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/key.bin"));
}

#[tokio::test]
async fn downloads_key_rotation_with_default_ivs() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/rot.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("rot.ts"))
            .await
            .unwrap(),
        [ROT_PLAIN_0, ROT_PLAIN_1].concat()
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/rot-key1.bin"));
    assert!(requests.iter().any(|path| path == "/rot-key2.bin"));
}

#[tokio::test]
async fn downloads_an_encrypted_fmp4_init_segment() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/aes-map.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("aes-map.mp4"))
            .await
            .unwrap(),
        [INIT_PLAIN, MAP_PLAIN].concat()
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/enc-init.mp4"));
}

#[tokio::test]
async fn resumes_an_aes_128_playlist() {
    let server = HlsServer::spawn(Some("/enc-1.ts")).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/aes.m3u8")).unwrap(),
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

    let temp_dir = config.download.dir.join("aes.ts.shaul.hls.parts");
    assert!(
        tokio::fs::try_exists(temp_dir.join("segment-00000.bin"))
            .await
            .unwrap()
    );

    let _ = server.take_requests();
    engine.resume(task.id).await.unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("aes.ts"))
            .await
            .unwrap(),
        [AES_PLAIN_0, AES_PLAIN_1].concat()
    );

    let requests = server.take_requests();
    assert!(!requests.iter().any(|path| path == "/enc-0.ts"));
    assert!(requests.iter().any(|path| path == "/enc-1.ts"));
}

#[tokio::test]
async fn downloads_byterange_segments() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/byterange.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("byterange.ts"))
            .await
            .unwrap(),
        b"AAAAABBBBB".to_vec()
    );
}

#[tokio::test]
async fn downloads_byterange_segments_when_range_is_ignored() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/range-ignored.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("range-ignored.ts"))
            .await
            .unwrap(),
        b"AAAAABBBBB".to_vec()
    );
}

#[tokio::test]
async fn downloads_a_byterange_fmp4_init_segment() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/byterange-map.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("byterange-map.mp4"))
            .await
            .unwrap(),
        b"AAAAA".to_vec()
    );
}

#[tokio::test]
async fn rejects_sample_aes_at_add_time() {
    let server = HlsServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let result = engine
        .add(
            Link::new(server.url("/sample-aes.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn rejects_live_playlists_without_a_target_duration_at_add_time() {
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
async fn records_a_live_playlist_until_the_segment_limit() {
    let server = LiveHlsServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_segments: Some(4),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    let output = config.download.dir.join("live.ts");
    assert_eq!(
        tokio::fs::read(&output).await.unwrap(),
        live_expected(&[102, 103, 104, 105])
    );
    assert!(
        !tokio::fs::try_exists(config.download.dir.join("live.ts.shaul.hls.parts"))
            .await
            .unwrap()
    );

    let requests = server.take_requests();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() == "/live.m3u8")
            .count(),
        3
    );
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() == "/live-key.bin")
            .count(),
        1
    );
    assert!(requests.iter().any(|path| path == "/live-105.ts"));
    assert!(!requests.iter().any(|path| path == "/live-106.ts"));
}

#[tokio::test]
async fn records_a_live_playlist_until_the_duration_limit() {
    let server = LiveHlsServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_duration_secs: Some(8),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("live.ts"))
            .await
            .unwrap(),
        live_expected(&[102, 103])
    );

    let requests = server.take_requests();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() == "/live.m3u8")
            .count(),
        2
    );
    assert!(!requests.iter().any(|path| path == "/live-104.ts"));
}

#[tokio::test]
async fn stops_a_live_capture_when_the_playlist_ends() {
    let server = LiveHlsServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_segments: Some(10),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("live.ts"))
            .await
            .unwrap(),
        live_expected(&[102, 103, 104, 105])
    );
}

#[tokio::test]
async fn live_capture_requires_a_limit() {
    let server = LiveHlsServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.m3u8")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let result = engine.run_task(task.id).await;
    assert!(result.is_err());
    let task = engine.status(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Failed);

    let requests = server.take_requests();
    assert!(!requests.iter().any(|path| path == "/live-102.ts"));
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
