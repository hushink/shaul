use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{
    AddTorrent, AddTorrentOptions, CreateTorrentOptions, ListenerOptions, ManagedTorrent, Session,
    SessionOptions, create_torrent,
};
use shaul_bt::{BtBackend, TorrentParser};
use shaul_core::{
    Config, DownloadBackend, DownloadEngine, Link, ParserRegistry, Router, SidecarStore,
    TaskOptions, TaskState, TaskStore, backend_arc, parser_arc,
};

fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

async fn wait_until_finished(handle: &Arc<ManagedTorrent>, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let stats = handle.stats();
        if stats.finished {
            return;
        }
        if let Some(error) = stats.error {
            panic!("{label} failed: {error}");
        }
        if Instant::now() >= deadline {
            panic!("{label} did not finish in time");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn downloads_a_local_torrent_from_a_local_peer() {
    let base = tempfile::tempdir().unwrap();
    let seed_dir = base.path().join("seed");
    tokio::fs::create_dir_all(&seed_dir).await.unwrap();
    let data = payload(512 * 1024);
    let payload_path = seed_dir.join("payload.bin");
    tokio::fs::write(&payload_path, &data).await.unwrap();

    let created = create_torrent(
        &payload_path,
        CreateTorrentOptions {
            name: Some("payload.bin"),
            trackers: Vec::new(),
            piece_length: Some(16 * 1024),
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let torrent_bytes = created.as_bytes().unwrap();
    let torrent_path = base.path().join("payload.torrent");
    tokio::fs::write(&torrent_path, &torrent_bytes)
        .await
        .unwrap();

    let seeder = Session::new_with_opts(
        seed_dir.clone(),
        SessionOptions {
            dht: None,
            disable_trackers: true,
            listen: Some(ListenerOptions {
                listen_addr: "127.0.0.1:0".parse().unwrap(),
                ..ListenerOptions::default()
            }),
            persistence: None,
            fastresume: false,
            ..SessionOptions::default()
        },
    )
    .await
    .unwrap();
    let seeder_handle = seeder
        .add_torrent(
            AddTorrent::from_bytes(torrent_bytes.to_vec()),
            Some(AddTorrentOptions {
                overwrite: true,
                output_folder: Some(seed_dir.to_string_lossy().to_string()),
                disable_trackers: true,
                ..AddTorrentOptions::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    wait_until_finished(&seeder_handle, "seeder").await;
    let seeder_addr = seeder.listen_addr().expect("seeder listen address");

    let download_dir = base.path().join("downloads");
    let state_dir = base.path().join("state");
    let mut config = Config::default();
    config.download.dir = download_dir.clone();
    config.state_dir = Some(state_dir.clone());
    config.bt.dht = false;
    config.bt.trackers = false;
    config.bt.listen = false;
    config.bt.persistence = false;
    config.bt.initial_peers = vec![seeder_addr.to_string()];

    let parsers = ParserRegistry::new().with(parser_arc(TorrentParser));
    let backends: Vec<Arc<dyn DownloadBackend>> =
        vec![backend_arc(BtBackend::new(&config).unwrap())];
    let router = Router::new(parsers, backends);
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(state_dir).unwrap());
    let engine = DownloadEngine::new(Arc::new(router), store, config.clone());

    let task = engine
        .add(
            Link::new(torrent_path.to_string_lossy()).unwrap(),
            download_dir.clone(),
            TaskOptions::default(),
        )
        .await
        .unwrap();
    let task = tokio::time::timeout(Duration::from_secs(60), engine.run_task(task.id))
        .await
        .expect("client download timed out")
        .unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(task.files.len(), 1);
    let output = download_dir.join(&task.files[0].path);
    assert_eq!(tokio::fs::read(&output).await.unwrap(), data);
}

#[tokio::test]
async fn selects_files_from_a_multi_file_torrent() {
    let base = tempfile::tempdir().unwrap();
    let seed_dir = base.path().join("seed");
    tokio::fs::create_dir_all(&seed_dir).await.unwrap();
    let data_a = payload(128 * 1024);
    let data_b = (0..128 * 1024)
        .map(|index| (index % 249) as u8)
        .collect::<Vec<u8>>();
    tokio::fs::write(seed_dir.join("a.bin"), &data_a)
        .await
        .unwrap();
    tokio::fs::write(seed_dir.join("b.bin"), &data_b)
        .await
        .unwrap();

    let created = create_torrent(
        &seed_dir,
        CreateTorrentOptions {
            name: Some("bundle"),
            trackers: Vec::new(),
            piece_length: Some(16 * 1024),
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let torrent_bytes = created.as_bytes().unwrap();
    let torrent_path = base.path().join("bundle.torrent");
    tokio::fs::write(&torrent_path, &torrent_bytes)
        .await
        .unwrap();

    let seeder = Session::new_with_opts(
        seed_dir.clone(),
        SessionOptions {
            dht: None,
            disable_trackers: true,
            listen: Some(ListenerOptions {
                listen_addr: "127.0.0.1:0".parse().unwrap(),
                ..ListenerOptions::default()
            }),
            persistence: None,
            fastresume: false,
            ..SessionOptions::default()
        },
    )
    .await
    .unwrap();
    let seeder_handle = seeder
        .add_torrent(
            AddTorrent::from_bytes(torrent_bytes.to_vec()),
            Some(AddTorrentOptions {
                overwrite: true,
                output_folder: Some(seed_dir.to_string_lossy().to_string()),
                disable_trackers: true,
                ..AddTorrentOptions::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    wait_until_finished(&seeder_handle, "seeder").await;
    let seeder_addr = seeder.listen_addr().expect("seeder listen address");

    let download_dir = base.path().join("downloads");
    let state_dir = base.path().join("state");
    let mut config = Config::default();
    config.download.dir = download_dir.clone();
    config.state_dir = Some(state_dir.clone());
    config.bt.dht = false;
    config.bt.trackers = false;
    config.bt.listen = false;
    config.bt.persistence = false;
    config.bt.initial_peers = vec![seeder_addr.to_string()];

    let parsers = ParserRegistry::new().with(parser_arc(TorrentParser));
    let backends: Vec<Arc<dyn DownloadBackend>> =
        vec![backend_arc(BtBackend::new(&config).unwrap())];
    let router = Router::new(parsers, backends);
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(state_dir).unwrap());
    let engine = DownloadEngine::new(Arc::new(router), store, config.clone());

    let options = TaskOptions {
        include: vec!["a.bin".to_string()],
        ..TaskOptions::default()
    };
    let task = engine
        .add(
            Link::new(torrent_path.to_string_lossy()).unwrap(),
            download_dir.clone(),
            options,
        )
        .await
        .unwrap();
    let task = tokio::time::timeout(Duration::from_secs(60), engine.run_task(task.id))
        .await
        .expect("client download timed out")
        .unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(task.files.len(), 1);
    let output = download_dir.join(&task.files[0].path);
    assert_eq!(tokio::fs::read(&output).await.unwrap(), data_a);
}
