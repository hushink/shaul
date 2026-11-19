//! End-to-end HTTP benchmark against a local range-capable server.
//!
//! Run with:
//! ```console
//! cargo bench -p shaul --bench http
//! ```

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use shaul::{
    ByteRange, Config, DownloadEngine, FileResume, Link, TaskId, TaskOptions, TaskResume,
    TaskState, Validator, VerifyMode, default_engine, part_path,
};
use tempfile::TempDir;
use tokio::runtime::Runtime;

const FILE_SIZE: usize = 16 * 1024 * 1024;
const SEGMENT_SIZE: u64 = 1024 * 1024;
const ETAG: &str = "\"shaul-bench\"";

fn bench_data() -> Vec<u8> {
    (0..FILE_SIZE).map(|index| (index % 251) as u8).collect()
}

fn bench_http(c: &mut Criterion) {
    let data = Arc::new(bench_data());
    let server = BenchServer::start(data.clone());
    let url = server.url.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("http_download");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(5))
        .throughput(Throughput::Bytes(FILE_SIZE as u64));

    for connections in [1_usize, 4, 8] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("full_{connections}c")),
            &connections,
            |b, &connections| {
                b.iter_batched(
                    || prepare_download(&rt, &url, connections, data.len() as u64),
                    |(_dir, engine, task_id)| {
                        let task = rt.block_on(engine.run_task(task_id)).unwrap();
                        assert_eq!(task.state, TaskState::Completed);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.throughput(Throughput::Bytes((FILE_SIZE / 2) as u64));
    group.bench_function("resume_half_8c", |b| {
        b.iter_batched(
            || prepare_resume(&rt, &url, 8, &data),
            |(_dir, engine, task_id)| {
                let task = rt.block_on(engine.run_task(task_id)).unwrap();
                assert_eq!(task.state, TaskState::Completed);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn prepare_download(
    rt: &Runtime,
    url: &str,
    connections: usize,
    _total: u64,
) -> (TempDir, DownloadEngine, TaskId) {
    let dir = tempfile::tempdir().unwrap();
    let config = bench_config(dir.path(), connections);
    let (engine, _store) = default_engine(config).unwrap();
    let task = rt
        .block_on(engine.add(
            Link::new(url.to_string()).unwrap(),
            dir.path().to_path_buf(),
            task_options(connections),
        ))
        .unwrap();
    (dir, engine, task.id)
}

fn prepare_resume(
    rt: &Runtime,
    url: &str,
    connections: usize,
    data: &[u8],
) -> (TempDir, DownloadEngine, TaskId) {
    let dir = tempfile::tempdir().unwrap();
    let config = bench_config(dir.path(), connections);
    let (engine, store) = default_engine(config).unwrap();
    let mut task = rt
        .block_on(engine.add(
            Link::new(url.to_string()).unwrap(),
            dir.path().to_path_buf(),
            task_options(connections),
        ))
        .unwrap();
    let file = task.resource.as_ref().unwrap().files[0].clone();
    let total = data.len() as u64;
    let half = (data.len() / 2) as u64;
    let final_path = task.destination.join(&file.path);
    let part = part_path(&final_path);
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut part_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&part)
        .unwrap();
    part_file.set_len(total).unwrap();
    part_file.write_all(&data[..half as usize]).unwrap();
    part_file.sync_all().unwrap();

    let mut resume = TaskResume::default();
    resume.upsert(
        file.url.to_string(),
        FileResume {
            url: file.url.to_string(),
            final_path: file.path.clone(),
            part_path: format!("{}.shaul.part", file.path),
            validator: Validator {
                etag: Some(ETAG.to_string()),
                last_modified: None,
                size: Some(total),
            },
            total_size: Some(total),
            segment_size: SEGMENT_SIZE,
            completed: vec![ByteRange::new(0, half).unwrap()],
            checksum: None,
            metadata: BTreeMap::new(),
            updated_at_ms: 0,
        },
    );
    task.resume = Some(resume);
    task.state = TaskState::Pending;
    task.touch();
    rt.block_on(store.save(&task)).unwrap();
    (dir, engine, task.id)
}

fn bench_config(root: &std::path::Path, connections: usize) -> Config {
    let mut config = Config::default();
    config.download.dir = root.to_path_buf();
    config.download.connections = connections;
    config.download.segment_size = SEGMENT_SIZE;
    config.download.verify = VerifyMode::Disabled;
    config.network.retries = 0;
    config.state_dir = Some(root.join("state"));
    config
}

fn task_options(connections: usize) -> TaskOptions {
    TaskOptions {
        connections: Some(connections),
        ..TaskOptions::default()
    }
}

struct BenchServer {
    url: String,
    _thread: thread::JoinHandle<()>,
}

impl BenchServer {
    fn start(data: Arc<Vec<u8>>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    break;
                };
                let data = data.clone();
                thread::spawn(move || serve(stream, data));
            }
        });
        Self {
            url: format!("http://{address}/bench.bin"),
            _thread: handle,
        }
    }
}

fn serve(mut stream: TcpStream, data: Arc<Vec<u8>>) {
    let _ = stream.set_nodelay(true);
    let mut request = Vec::with_capacity(8 * 1024);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
            Err(_) => return,
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 64 * 1024 {
            return;
        }
    }

    let text = String::from_utf8_lossy(&request);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("GET");
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_string())
    });
    let total = data.len();
    let selected = range.as_deref().and_then(|value| parse_range(value, total));

    let (status, body, content_range) = match selected {
        Some((start, end)) => (
            "206 Partial Content",
            &data[start..=end],
            Some(format!("bytes {start}-{end}/{total}")),
        ),
        None => ("200 OK", &data[..], None),
    };
    let mut headers = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Length: {}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Disposition: attachment; filename=\"bench.bin\"\r\n\
         ETag: {ETAG}\r\n\
         Accept-Ranges: bytes\r\n\
         Connection: close\r\n",
        body.len()
    );
    if let Some(content_range) = content_range {
        headers.push_str(&format!("Content-Range: {content_range}\r\n"));
    }
    headers.push_str("\r\n");
    let _ = stream.write_all(headers.as_bytes());
    if method != "HEAD" {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
}

fn parse_range(value: &str, total: usize) -> Option<(usize, usize)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<usize>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

criterion_group!(benches, bench_http);
criterion_main!(benches);
