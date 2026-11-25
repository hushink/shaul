use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use shaul_core::{Config, DownloadEngine, Link, SidecarStore, TaskOptions, TaskState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TEMPLATE_MPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT12S">
  <Period>
    <AdaptationSet id="video" mimeType="video/mp4" contentType="video">
      <SegmentTemplate timescale="1000" duration="4000"
          initialization="init-$RepresentationID$.mp4"
          media="seg-$RepresentationID$-$Number%05d$.m4s"/>
      <Representation id="720p" bandwidth="800000" width="1280" height="720" codecs="avc1.4d401f"/>
      <Representation id="1080p" bandwidth="2400000" width="1920" height="1080" codecs="avc1.640028"/>
    </AdaptationSet>
    <AdaptationSet id="audio" mimeType="audio/mp4" contentType="audio" lang="en">
      <SegmentTemplate timescale="1000" duration="4000"
          initialization="audio-init.mp4" media="audio-$Number$.m4s"/>
      <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>
"#;

const TIMELINE_MPD: &str = r#"<MPD type="static"><Period>
  <AdaptationSet mimeType="video/mp4">
    <Representation id="v" bandwidth="1">
      <SegmentTemplate initialization="init.mp4" media="t$Time$.m4s" timescale="1000">
        <SegmentTimeline>
          <S t="0" d="4000" r="1"/>
          <S d="2000"/>
        </SegmentTimeline>
      </SegmentTemplate>
    </Representation>
  </AdaptationSet>
</Period></MPD>"#;

const LIST_MPD: &str = r#"<MPD type="static"><Period>
  <AdaptationSet mimeType="video/mp4">
    <Representation id="v" bandwidth="1">
      <BaseURL>movie.mp4</BaseURL>
      <SegmentList>
        <Initialization range="0-3"/>
        <SegmentURL mediaRange="4-6"/>
        <SegmentURL mediaRange="7-9"/>
      </SegmentList>
    </Representation>
  </AdaptationSet>
</Period></MPD>"#;

const SINGLE_MPD: &str = r#"<MPD type="static"><Period>
  <AdaptationSet mimeType="video/webm">
    <Representation id="v" bandwidth="1">
      <BaseURL>movie.mp4</BaseURL>
      <SegmentBase indexRange="0-3"/>
    </Representation>
  </AdaptationSet>
</Period></MPD>"#;

const FAIL_MPD: &str = r#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
  <AdaptationSet mimeType="video/mp4">
    <Representation id="v" bandwidth="1">
      <SegmentTemplate initialization="fail-init.mp4"
          media="fseg-$Number$.m4s" timescale="1000" duration="4000"/>
    </Representation>
  </AdaptationSet>
</Period></MPD>"#;

const DRM_MPD: &str = r#"<MPD type="static"><Period><AdaptationSet>
  <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011"/>
  <Representation id="v"><BaseURL>movie.mp4</BaseURL></Representation>
</AdaptationSet></Period></MPD>"#;

const DYNAMIC_NO_TIMELINE_MPD: &str = r#"<MPD type="dynamic"><Period>
  <AdaptationSet mimeType="video/mp4">
    <Representation id="v" bandwidth="1">
      <SegmentTemplate media="s-$Number$.m4s" duration="2000" timescale="1000"/>
    </Representation>
  </AdaptationSet>
</Period></MPD>"#;

const MULTI_SHARED_INIT_MPD: &str = r#"<MPD type="static"><Period id="p1">
  <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
    <SegmentTemplate media="p1-$Number$.m4s" initialization="multi-init-a.mp4"
        timescale="1000" startNumber="1">
      <SegmentTimeline><S t="0" d="2000" r="1"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation></AdaptationSet>
</Period><Period id="p2">
  <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
    <SegmentTemplate media="p2-$Number$.m4s" initialization="multi-init-a.mp4"
        timescale="1000" startNumber="1">
      <SegmentTimeline><S t="0" d="2000" r="1"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation></AdaptationSet>
</Period></MPD>"#;

const MULTI_NO_INIT_MPD: &str = r#"<MPD type="static"><Period id="p1">
  <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
    <SegmentTemplate media="p1-$Number$.m4s" timescale="1000" startNumber="1">
      <SegmentTimeline><S t="0" d="2000" r="1"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation></AdaptationSet>
</Period><Period id="p2">
  <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
    <SegmentTemplate media="p2-$Number$.m4s" timescale="1000" startNumber="1">
      <SegmentTimeline><S t="0" d="2000" r="1"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation></AdaptationSet>
</Period></MPD>"#;

const MULTI_DIFFERENT_INIT_MPD: &str = r#"<MPD type="static"><Period id="p1">
  <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
    <SegmentTemplate media="p1-$Number$.m4s" initialization="multi-init-a.mp4"
        timescale="1000" startNumber="1">
      <SegmentTimeline><S t="0" d="2000" r="1"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation></AdaptationSet>
</Period><Period id="p2">
  <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
    <SegmentTemplate media="p2-$Number$.m4s" initialization="multi-init-b.mp4"
        timescale="1000" startNumber="1">
      <SegmentTimeline><S t="0" d="2000" r="1"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation></AdaptationSet>
</Period></MPD>"#;

const RANGE_DATA: &[u8] = b"IIIIAAABBB";

struct DashServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
}

impl DashServer {
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
    if path == "/movie.mp4" {
        let total = RANGE_DATA.len() as u64;
        if let Some((start, end)) = headers
            .get("range")
            .and_then(|value| parse_range(value, total))
        {
            let body = &RANGE_DATA[start as usize..=end as usize];
            respond_range(&mut socket, body, start, end, total).await;
        } else {
            respond(&mut socket, "200 OK", RANGE_DATA).await;
        }
        return;
    }

    let body: Vec<u8> = match path.as_str() {
        "/manifest.mpd" => TEMPLATE_MPD.as_bytes().to_vec(),
        "/timeline.mpd" => TIMELINE_MPD.as_bytes().to_vec(),
        "/list.mpd" => LIST_MPD.as_bytes().to_vec(),
        "/single.mpd" => SINGLE_MPD.as_bytes().to_vec(),
        "/fail.mpd" => FAIL_MPD.as_bytes().to_vec(),
        "/drm.mpd" => DRM_MPD.as_bytes().to_vec(),
        "/no-timeline.mpd" => DYNAMIC_NO_TIMELINE_MPD.as_bytes().to_vec(),
        "/multi-shared.mpd" => MULTI_SHARED_INIT_MPD.as_bytes().to_vec(),
        "/multi-no-init.mpd" => MULTI_NO_INIT_MPD.as_bytes().to_vec(),
        "/multi-different-init.mpd" => MULTI_DIFFERENT_INIT_MPD.as_bytes().to_vec(),
        "/multi-init-a.mp4" => b"INIT-A".to_vec(),
        "/multi-init-b.mp4" => b"INIT-B".to_vec(),
        "/p1-1.m4s" => b"A1".to_vec(),
        "/p1-2.m4s" => b"A2".to_vec(),
        "/p2-1.m4s" => b"B1".to_vec(),
        "/p2-2.m4s" => b"B2".to_vec(),
        "/init-720p.mp4" => b"INIT-720".to_vec(),
        "/init-1080p.mp4" => b"INIT-1080".to_vec(),
        "/seg-720p-00001.m4s" => b"L1".to_vec(),
        "/seg-720p-00002.m4s" => b"L2".to_vec(),
        "/seg-720p-00003.m4s" => b"L3".to_vec(),
        "/seg-1080p-00001.m4s" => b"H1".to_vec(),
        "/seg-1080p-00002.m4s" => b"H2".to_vec(),
        "/seg-1080p-00003.m4s" => b"H3".to_vec(),
        "/audio-init.mp4" => b"AUDIO-INIT".to_vec(),
        "/audio-1.m4s" => b"A1".to_vec(),
        "/audio-2.m4s" => b"A2".to_vec(),
        "/audio-3.m4s" => b"A3".to_vec(),
        "/init.mp4" => b"TINIT".to_vec(),
        "/t0.m4s" => b"T0".to_vec(),
        "/t4000.m4s" => b"T4".to_vec(),
        "/t8000.m4s" => b"T8".to_vec(),
        "/fail-init.mp4" => b"FINIT".to_vec(),
        "/fseg-1.m4s" => b"F1".to_vec(),
        "/fseg-2.m4s" => b"F2".to_vec(),
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
    let router = shaul_dash::default_router(config).unwrap();
    let store = Arc::new(SidecarStore::new(config.state_dir().unwrap()).unwrap());
    DownloadEngine::new(Arc::new(router), store, config.clone())
}

async fn download(
    engine: &DownloadEngine,
    config: &Config,
    url: String,
    options: TaskOptions,
) -> shaul_core::Task {
    let task = engine
        .add(
            Link::new(url).unwrap(),
            config.download.dir.clone(),
            options,
        )
        .await
        .unwrap();
    engine.run_task(task.id).await.unwrap()
}

struct LiveDashServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
}

impl LiveDashServer {
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
                    handle_live_dash(socket, requests, fetches).await;
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

fn dynamic_mpd(segment_count: usize, static_manifest: bool) -> String {
    let mut timeline = String::new();
    for index in 0..segment_count {
        let time = index as u64 * 2000;
        timeline.push_str(&format!("<S t=\"{time}\" d=\"2000\"/>"));
    }
    let kind = if static_manifest { "static" } else { "dynamic" };
    format!(
        r#"<MPD type="{kind}" minimumUpdatePeriod="PT1S"><Period>
<AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
<SegmentTemplate media="seg-$Time$.m4s" initialization="init.mp4" timescale="1000">
<SegmentTimeline>{timeline}</SegmentTimeline>
</SegmentTemplate>
</Representation></AdaptationSet></Period></MPD>"#
    )
}

fn live_dash_payload(time: u64) -> Vec<u8> {
    format!("D{time}").into_bytes()
}

fn live_dash_expected(times: &[u64]) -> Vec<u8> {
    let mut output = b"INIT".to_vec();
    for time in times {
        output.extend_from_slice(&live_dash_payload(*time));
    }
    output
}

async fn handle_live_dash(
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

    if path == "/live.mpd" {
        let fetch = fetches.fetch_add(1, Ordering::SeqCst) + 1;
        let (count, static_manifest) = match fetch {
            1 => (2, false),
            2 => (4, false),
            _ => (5, true),
        };
        respond(
            &mut socket,
            "200 OK",
            dynamic_mpd(count, static_manifest).as_bytes(),
        )
        .await;
        return;
    }
    if path == "/init.mp4" {
        respond(&mut socket, "200 OK", b"INIT").await;
        return;
    }
    for time in [0, 2000, 4000, 6000, 8000] {
        if path == format!("/seg-{time}.m4s") {
            respond(&mut socket, "200 OK", &live_dash_payload(time)).await;
            return;
        }
    }
    respond(&mut socket, "404 Not Found", b"not found").await;
}

struct ConstantLiveDashServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
}

impl ConstantLiveDashServer {
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
                    handle_constant_dash(socket, requests, fetches).await;
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

fn constant_dynamic_mpd(publish_offset: u64, static_manifest: bool) -> String {
    let attributes = if static_manifest {
        r#"type="static" mediaPresentationDuration="PT20S""#.to_string()
    } else {
        format!(
            r#"type="dynamic" minimumUpdatePeriod="PT1S" availabilityStartTime="2026-01-01T00:00:00Z" publishTime="2026-01-01T00:00:{publish_offset:02}Z" suggestedPresentationDelay="PT0S" timeShiftBufferDepth="PT30S""#
        )
    };
    format!(
        r#"<MPD {attributes}>
<Period start="PT0S"><AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
<SegmentTemplate media="cseg-$Number$.m4s" initialization="cinit.mp4"
    timescale="1000" duration="2000" startNumber="1"/>
</Representation></AdaptationSet></Period></MPD>"#
    )
}

fn constant_payload(number: u64) -> Vec<u8> {
    format!("C{number}").into_bytes()
}

async fn handle_constant_dash(
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

    if path == "/constant.mpd" {
        let fetch = fetches.fetch_add(1, Ordering::SeqCst) + 1;
        let body = match fetch {
            1 => constant_dynamic_mpd(10, false),
            2 => constant_dynamic_mpd(14, false),
            _ => constant_dynamic_mpd(20, true),
        };
        respond(&mut socket, "200 OK", body.as_bytes()).await;
        return;
    }
    if path == "/cinit.mp4" {
        respond(&mut socket, "200 OK", b"INIT").await;
        return;
    }
    if let Some(number) = path
        .strip_prefix("/cseg-")
        .and_then(|value| value.strip_suffix(".m4s"))
        .and_then(|value| value.parse::<u64>().ok())
    {
        respond(&mut socket, "200 OK", &constant_payload(number)).await;
        return;
    }
    respond(&mut socket, "404 Not Found", b"not found").await;
}

#[tokio::test]
async fn downloads_a_segment_template_vod() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/manifest.mpd"),
        TaskOptions::default(),
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    let output = config.download.dir.join("manifest.mp4");
    assert_eq!(
        tokio::fs::read(&output).await.unwrap(),
        b"INIT-1080H1H2H3".to_vec()
    );
    assert!(
        !tokio::fs::try_exists(config.download.dir.join("manifest.mp4.shaul.dash.parts"))
            .await
            .unwrap()
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/init-1080p.mp4"));
    assert!(requests.iter().any(|path| path == "/seg-1080p-00001.m4s"));
    assert!(!requests.iter().any(|path| path == "/audio-init.mp4"));
}

#[tokio::test]
async fn honors_include_selection() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/manifest.mpd"),
        TaskOptions {
            include: vec!["*720*".to_string()],
            ..TaskOptions::default()
        },
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("manifest.mp4"))
            .await
            .unwrap(),
        b"INIT-720L1L2L3".to_vec()
    );
    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/init-720p.mp4"));
    assert!(!requests.iter().any(|path| path == "/init-1080p.mp4"));
}

#[tokio::test]
async fn downloads_a_segment_timeline() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/timeline.mpd"),
        TaskOptions::default(),
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("timeline.mp4"))
            .await
            .unwrap(),
        b"TINITT0T4T8".to_vec()
    );
}

#[tokio::test]
async fn downloads_a_multi_period_manifest_with_a_shared_init() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/multi-shared.mpd"),
        TaskOptions::default(),
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("multi-shared.mp4"))
            .await
            .unwrap(),
        b"INIT-AA1A2B1B2".to_vec()
    );
}

#[tokio::test]
async fn downloads_a_multi_period_manifest_without_initialization() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/multi-no-init.mpd"),
        TaskOptions::default(),
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("multi-no-init.mp4"))
            .await
            .unwrap(),
        b"A1A2B1B2".to_vec()
    );
}

#[tokio::test]
async fn downloads_a_segment_list_with_byte_ranges() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/list.mpd"),
        TaskOptions::default(),
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("list.mp4"))
            .await
            .unwrap(),
        b"IIIIAAABBB".to_vec()
    );
}

#[tokio::test]
async fn downloads_a_segment_base_object_and_uses_the_mime_extension() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = download(
        &engine,
        &config,
        server.url("/single.mpd"),
        TaskOptions::default(),
    )
    .await;
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("single.webm"))
            .await
            .unwrap(),
        RANGE_DATA.to_vec()
    );
}

#[tokio::test]
async fn resumes_completed_segments() {
    let server = DashServer::spawn(Some("/fseg-2.m4s")).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.download.connections = 1;
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/fail.mpd")).unwrap(),
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

    let temp_dir = config.download.dir.join("fail.mp4.shaul.dash.parts");
    assert!(
        tokio::fs::try_exists(temp_dir.join("segment-00000.bin"))
            .await
            .unwrap()
    );
    assert!(
        !tokio::fs::try_exists(temp_dir.join("segment-00001.bin"))
            .await
            .unwrap()
    );

    let _ = server.take_requests();
    engine.resume(task.id).await.unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("fail.mp4"))
            .await
            .unwrap(),
        b"FINITF1F2".to_vec()
    );

    let requests = server.take_requests();
    assert!(!requests.iter().any(|path| path == "/fail-init.mp4"));
    assert!(!requests.iter().any(|path| path == "/fseg-1.m4s"));
    assert!(requests.iter().any(|path| path == "/fseg-2.m4s"));
}

#[tokio::test]
async fn rejects_unsupported_manifests() {
    let server = DashServer::spawn(None).await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    for path in ["/drm.mpd", "/no-timeline.mpd", "/multi-different-init.mpd"] {
        let result = engine
            .add(
                Link::new(server.url(path)).unwrap(),
                config.download.dir.clone(),
                TaskOptions::default(),
            )
            .await;
        assert!(result.is_err(), "{path} should be rejected");
    }
}

#[tokio::test]
async fn records_a_dynamic_manifest_until_the_segment_limit() {
    let server = LiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.mpd")).unwrap(),
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

    assert_eq!(
        tokio::fs::read(config.download.dir.join("live.mp4"))
            .await
            .unwrap(),
        live_dash_expected(&[0, 2000, 4000, 6000])
    );
    assert!(
        !tokio::fs::try_exists(config.download.dir.join("live.mp4.shaul.dash.parts"))
            .await
            .unwrap()
    );

    let requests = server.take_requests();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() == "/live.mpd")
            .count(),
        2
    );
    assert!(requests.iter().any(|path| path == "/init.mp4"));
    assert!(!requests.iter().any(|path| path == "/seg-8000.m4s"));
}

#[tokio::test]
async fn records_a_dynamic_manifest_until_the_duration_limit() {
    let server = LiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.mpd")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_duration_secs: Some(4),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("live.mp4"))
            .await
            .unwrap(),
        live_dash_expected(&[0, 2000])
    );

    let requests = server.take_requests();
    assert!(!requests.iter().any(|path| path == "/seg-4000.m4s"));
}

#[tokio::test]
async fn stops_a_dynamic_capture_when_the_manifest_becomes_static() {
    let server = LiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.mpd")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_segments: Some(100),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("live.mp4"))
            .await
            .unwrap(),
        live_dash_expected(&[0, 2000, 4000, 6000, 8000])
    );

    let requests = server.take_requests();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() == "/live.mpd")
            .count(),
        3
    );
}

#[tokio::test]
async fn records_a_constant_duration_dynamic_manifest() {
    let server = ConstantLiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/constant.mpd")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_segments: Some(3),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    let mut expected = b"INIT".to_vec();
    for number in [7_u64, 8, 9] {
        expected.extend_from_slice(&constant_payload(number));
    }
    assert_eq!(
        tokio::fs::read(config.download.dir.join("constant.mp4"))
            .await
            .unwrap(),
        expected
    );
    assert!(
        !tokio::fs::try_exists(config.download.dir.join("constant.mp4.shaul.dash.parts"))
            .await
            .unwrap()
    );

    let requests = server.take_requests();
    assert!(requests.iter().any(|path| path == "/cseg-7.m4s"));
    assert!(requests.iter().any(|path| path == "/cseg-8.m4s"));
    assert!(requests.iter().any(|path| path == "/cseg-9.m4s"));
    assert!(!requests.iter().any(|path| path == "/cseg-10.m4s"));
}

#[tokio::test]
async fn constant_dynamic_capture_until_the_duration_limit() {
    let server = ConstantLiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/constant.mpd")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_duration_secs: Some(2),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("constant.mp4"))
            .await
            .unwrap(),
        b"INITC7".to_vec()
    );
}

#[tokio::test]
async fn finishes_a_constant_dynamic_capture_when_the_manifest_becomes_static() {
    let server = ConstantLiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/constant.mpd")).unwrap(),
            config.download.dir.clone(),
            TaskOptions {
                live_segments: Some(100),
                ..TaskOptions::default()
            },
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, TaskState::Completed);

    let mut expected = b"INIT".to_vec();
    for number in [7_u64, 8, 9, 10] {
        expected.extend_from_slice(&constant_payload(number));
    }
    assert_eq!(
        tokio::fs::read(config.download.dir.join("constant.mp4"))
            .await
            .unwrap(),
        expected
    );
}

#[tokio::test]
async fn dynamic_capture_requires_a_limit() {
    let server = LiveDashServer::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(dir.path());
    let engine = engine(&config);

    let task = engine
        .add(
            Link::new(server.url("/live.mpd")).unwrap(),
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
    assert!(!requests.iter().any(|path| path == "/init.mp4"));
    assert!(!requests.iter().any(|path| path == "/seg-0.m4s"));
}
