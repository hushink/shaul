#![cfg(feature = "ftp")]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use shaul::{
    ByteRange, CancellationToken, Config, DownloadBackend, DownloadOutcome, DownloadRequest,
    EventSink, FileResume, FtpBackend, FtpParser, Link, LinkParser, ParseOptions, Result,
    ResumeCheckpoint, TaskOptions, TaskResume, Validator, part_path,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

struct MockFtp {
    addr: SocketAddr,
    rest_offsets: Arc<Mutex<Vec<usize>>>,
}

impl MockFtp {
    async fn spawn(data: Vec<u8>, required_auth: Option<(&str, &str)>) -> Self {
        Self::spawn_inner(data, required_auth, false).await
    }

    async fn spawn_rejecting_rest(data: Vec<u8>, required_auth: Option<(&str, &str)>) -> Self {
        Self::spawn_inner(data, required_auth, true).await
    }

    async fn spawn_inner(
        data: Vec<u8>,
        required_auth: Option<(&str, &str)>,
        reject_rest: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let data = Arc::new(data);
        let rest_offsets = Arc::new(Mutex::new(Vec::new()));
        let offsets = rest_offsets.clone();
        let required_auth = required_auth.map(|(user, pass)| (user.to_string(), pass.to_string()));
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(handle_ftp(
                    socket,
                    data.clone(),
                    offsets.clone(),
                    required_auth.clone(),
                    reject_rest,
                ));
            }
        });
        Self { addr, rest_offsets }
    }

    fn url(&self, path: &str) -> String {
        format!("ftp://{}/{}", self.addr, path)
    }

    fn auth_url(&self, user: &str, password: &str, path: &str) -> String {
        format!("ftp://{user}:{password}@{}/{}", self.addr, path)
    }

    fn offsets(&self) -> Vec<usize> {
        self.rest_offsets.lock().unwrap().clone()
    }
}

async fn handle_ftp(
    socket: TcpStream,
    data: Arc<Vec<u8>>,
    offsets: Arc<Mutex<Vec<usize>>>,
    required_auth: Option<(String, String)>,
    reject_rest: bool,
) {
    let (read, mut write) = socket.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    let _ = write.write_all(b"220 mock FTP ready\r\n").await;
    let mut passive: Option<TcpListener> = None;
    let mut restart = 0usize;
    let mut user = String::new();
    let mut authenticated = required_auth.is_none();

    loop {
        line.clear();
        let Ok(read) = reader.read_line(&mut line).await else {
            return;
        };
        if read == 0 {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        let (command, argument) = line.split_once(' ').unwrap_or((line, ""));
        let command = command.to_ascii_uppercase();
        let response = match command.as_str() {
            "USER" => {
                user = argument.to_string();
                "331 password required\r\n".to_string()
            }
            "PASS" => {
                if let Some((required_user, required_password)) = &required_auth {
                    if &user == required_user && argument == required_password {
                        authenticated = true;
                        "230 logged in\r\n".to_string()
                    } else {
                        "530 login incorrect\r\n".to_string()
                    }
                } else {
                    authenticated = true;
                    "230 logged in\r\n".to_string()
                }
            }
            "SYST" => "215 UNIX Type: L8\r\n".to_string(),
            "FEAT" => {
                "211-Features:\r\n SIZE\r\n MDTM\r\n REST STREAM\r\n EPSV\r\n PASV\r\n211 End\r\n"
                    .to_string()
            }
            "TYPE" | "OPTS" => "200 ok\r\n".to_string(),
            "PWD" => "257 \"/\"\r\n".to_string(),
            "SIZE" => format!("213 {}\r\n", data.len()),
            "MDTM" => "213 20260101000000\r\n".to_string(),
            "REST" => {
                if reject_rest {
                    "502 REST not implemented\r\n".to_string()
                } else {
                    restart = argument.trim().parse().unwrap_or(0);
                    format!("350 restart at {restart}\r\n")
                }
            }
            "EPSV" | "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = listener.local_addr().unwrap().port();
                passive = Some(listener);
                if command == "EPSV" {
                    format!("229 Entering Extended Passive Mode (|||{port}|)\r\n")
                } else {
                    format!(
                        "227 Entering Passive Mode (127,0,0,1,{},{})\r\n",
                        port / 256,
                        port % 256
                    )
                }
            }
            "RETR" => {
                if !authenticated {
                    "530 login required\r\n".to_string()
                } else {
                    offsets.lock().unwrap().push(restart);
                    let start = restart.min(data.len());
                    let _ = write.write_all(b"150 opening data connection\r\n").await;
                    if let Some(listener) = passive.take()
                        && let Ok((mut data_socket, _)) = listener.accept().await
                    {
                        let _ = data_socket.write_all(&data[start..]).await;
                        let _ = data_socket.shutdown().await;
                    }
                    restart = 0;
                    "226 transfer complete\r\n".to_string()
                }
            }
            "ABOR" => "226 aborted\r\n".to_string(),
            "NOOP" => "200 ok\r\n".to_string(),
            "QUIT" => {
                let _ = write.write_all(b"221 goodbye\r\n").await;
                return;
            }
            _ => "502 command not implemented\r\n".to_string(),
        };
        if write.write_all(response.as_bytes()).await.is_err() {
            return;
        }
    }
}

struct TestCheckpoint {
    resume: Arc<Mutex<TaskResume>>,
}

#[async_trait::async_trait]
impl ResumeCheckpoint for TestCheckpoint {
    async fn save(&self, resume: &TaskResume) -> Result<()> {
        *self.resume.lock().unwrap() = resume.clone();
        Ok(())
    }
}

#[tokio::test]
async fn downloads_an_ftp_file_through_the_engine() {
    let data: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
    let server = MockFtp::spawn(data.clone(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.download.dir = dir.path().to_path_buf();
    config.state_dir = Some(dir.path().join("state"));
    config.download.connections = 1;
    let (engine, _store) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.url("file.bin")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(task.backend.as_deref(), Some("ftp"));
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, shaul::TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("file.bin"))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn downloads_an_empty_ftp_file() {
    let server = MockFtp::spawn(Vec::new(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.download.dir = dir.path().to_path_buf();
    config.state_dir = Some(dir.path().join("state"));
    let (engine, _store) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.url("empty.bin")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, shaul::TaskState::Completed);
    assert_eq!(
        tokio::fs::metadata(config.download.dir.join("empty.bin"))
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn passes_ftp_credentials_to_the_server() {
    let data = b"authenticated ftp data".to_vec();
    let server = MockFtp::spawn(data.clone(), Some(("user", "secret"))).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.download.dir = dir.path().to_path_buf();
    config.state_dir = Some(dir.path().join("state"));
    let (engine, _store) = shaul::default_engine(config.clone()).unwrap();
    let task = engine
        .add(
            Link::new(server.auth_url("user", "secret", "private.bin")).unwrap(),
            config.download.dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = engine.run_task(task.id).await.unwrap();
    assert_eq!(task.state, shaul::TaskState::Completed);
    assert_eq!(
        tokio::fs::read(config.download.dir.join("private.bin"))
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn restarts_from_zero_when_rest_is_rejected() {
    let data: Vec<u8> = (0..128 * 1024).map(|index| (index % 251) as u8).collect();
    let server = MockFtp::spawn_rejecting_rest(data.clone(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let link = Link::new(server.url("file.bin")).unwrap();
    let resource = FtpParser
        .parse(&link, &ParseOptions::default())
        .await
        .unwrap();
    let key = resource.files[0].url.as_str().to_string();
    let final_path = dir.path().join("file.bin");
    let part = part_path(&final_path);
    let split = data.len() / 2;
    tokio::fs::write(&part, &data[..split]).await.unwrap();

    let mut resume = TaskResume::default();
    resume.files.insert(
        key.clone(),
        FileResume {
            url: key,
            final_path: "file.bin".to_string(),
            part_path: "file.bin.shaul.part".to_string(),
            validator: Validator {
                etag: None,
                last_modified: None,
                size: Some(data.len() as u64),
            },
            total_size: Some(data.len() as u64),
            segment_size: 64 * 1024,
            completed: vec![ByteRange::new(0, split as u64).unwrap()],
            checksum: None,
            metadata: Default::default(),
            updated_at_ms: 0,
        },
    );
    let shared = Arc::new(Mutex::new(TaskResume::default()));
    let options = Config::default()
        .resolve(dir.path(), &TaskOptions::default())
        .unwrap();
    let (events, _receiver) = EventSink::channel(16);
    let request = DownloadRequest {
        task_id: shaul::TaskId::new_v4(),
        resource,
        destination: dir.path().to_path_buf(),
        options,
        resume: Some(resume),
        cancel: CancellationToken::new(),
        events,
        checkpoint: Arc::new(TestCheckpoint {
            resume: shared.clone(),
        }),
    };

    let outcome = FtpBackend::new().download(request).await.unwrap();
    let DownloadOutcome::Completed(files) = outcome else {
        panic!("expected a completed FTP download");
    };
    assert_eq!(files.len(), 1);
    assert_eq!(
        tokio::fs::read(final_path).await.unwrap(),
        data,
        "fallback download content differs"
    );
    assert_eq!(
        server.offsets(),
        vec![0],
        "the fallback transfer should start from zero"
    );
    assert_eq!(
        shared
            .lock()
            .unwrap()
            .files
            .values()
            .next()
            .unwrap()
            .completed_bytes(),
        data.len() as u64
    );
}

#[tokio::test]
async fn resumes_with_a_rest_offset() {
    let data: Vec<u8> = (0..128 * 1024).map(|index| (index % 251) as u8).collect();
    let server = MockFtp::spawn(data.clone(), None).await;
    let dir = tempfile::tempdir().unwrap();
    let link = Link::new(server.url("file.bin")).unwrap();
    let resource = FtpParser
        .parse(&link, &ParseOptions::default())
        .await
        .unwrap();
    let key = resource.files[0].url.as_str().to_string();
    let final_path = dir.path().join("file.bin");
    let part = part_path(&final_path);
    let split = data.len() / 2;
    tokio::fs::write(&part, &data[..split]).await.unwrap();

    let mut resume = TaskResume::default();
    resume.files.insert(
        key.clone(),
        FileResume {
            url: key,
            final_path: "file.bin".to_string(),
            part_path: "file.bin.shaul.part".to_string(),
            validator: Validator {
                etag: None,
                last_modified: None,
                size: Some(data.len() as u64),
            },
            total_size: Some(data.len() as u64),
            segment_size: 64 * 1024,
            completed: vec![ByteRange::new(0, split as u64).unwrap()],
            checksum: None,
            metadata: Default::default(),
            updated_at_ms: 0,
        },
    );
    let shared = Arc::new(Mutex::new(TaskResume::default()));
    let options = Config::default()
        .resolve(dir.path(), &TaskOptions::default())
        .unwrap();
    let (events, _receiver) = EventSink::channel(16);
    let request = DownloadRequest {
        task_id: shaul::TaskId::new_v4(),
        resource,
        destination: dir.path().to_path_buf(),
        options,
        resume: Some(resume),
        cancel: CancellationToken::new(),
        events,
        checkpoint: Arc::new(TestCheckpoint {
            resume: shared.clone(),
        }),
    };

    let outcome = FtpBackend::new().download(request).await.unwrap();
    let DownloadOutcome::Completed(files) = outcome else {
        panic!("expected a completed FTP download");
    };
    assert_eq!(files.len(), 1);
    assert_eq!(
        tokio::fs::read(final_path).await.unwrap(),
        data,
        "resumed file content differs"
    );
    assert_eq!(
        server.offsets(),
        vec![split],
        "backend did not send the expected REST offset"
    );
    assert_eq!(
        shared
            .lock()
            .unwrap()
            .files
            .values()
            .next()
            .unwrap()
            .completed_bytes(),
        data.len() as u64
    );
}
