use futures::TryFutureExt;
use log::info;
use tokio::time::sleep;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;

use crate::file_database::FileDatabase;
use crate::indexer::Indexer;
use crate::sync_engine::{FileProvider, SyncEngine};

struct ResolverData {
    need_resolve: HashSet<String>,
    peers_have: HashMap<String, Vec<String>>,
}

pub struct FileResolverStorage {
    data: Arc<Mutex<ResolverData>>,
    pub file_db: Arc<FileDatabase>,
    to_resolve_send: Arc<flume::Sender<ResolveWant>>,
    to_resolve_recv: Arc<flume::Receiver<ResolveWant>>,
}

#[derive(Clone)]
pub struct ResolveWant {
    pub file_id: String,
    pub failed_peers: Vec<String>,
}

impl From<String> for ResolveWant {
    fn from(value: String) -> Self {
        Self {
            file_id: value,
            failed_peers: vec![],
        }
    }
}

impl FileResolverStorage {
    pub fn new(file_db: Arc<FileDatabase>) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            data: Arc::new(Mutex::new(ResolverData {
                need_resolve: HashSet::new(),
                peers_have: HashMap::new(),
            })),
            file_db,
            to_resolve_recv: Arc::new(receiver),
            to_resolve_send: Arc::new(sender),
        }
    }

    pub async fn add_need_resolve(&self, file_id: &str, peer_id: Option<String>) {
        let mut data = self.data.lock().await;
        data.need_resolve.insert(file_id.to_string());
        if let Some(peer_id) = peer_id {
            data.peers_have
                .entry(file_id.to_string())
                .or_insert(Vec::new())
                .push(peer_id);
        }
        if let Err(e) = self
            .to_resolve_send
            .send_async(file_id.to_owned().into())
            .await
        {
            log::warn!("failed to send to resolve: {}", e);
        }
    }

    pub async fn add_peer_have(&self, file_id: &str, peer_id: &str) {
        let mut data = self.data.lock().await;
        data.peers_have
            .entry(file_id.to_string())
            .or_insert(Vec::new())
            .push(peer_id.to_string());
        if let Err(e) = self
            .to_resolve_send
            .send_async(file_id.to_owned().into())
            .await
        {
            log::warn!("failed to send to resolve: {}", e);
        }
    }

    pub async fn add_peer_have_many(&self, file_ids: Vec<String>, peer_id: &str) {
        let mut data = self.data.lock().await;
        for file_id in file_ids {
            data.peers_have
                .entry(file_id.to_string())
                .or_insert(Vec::new())
                .push(peer_id.to_string());
            if let Err(e) = self
                .to_resolve_send
                .send_async(file_id.to_owned().into())
                .await
            {
                log::warn!("failed to send to resolve: {}", e);
            }
        }
    }

    pub async fn get_peers_have(&self, file_id: &str) -> Vec<String> {
        let data = self.data.lock().await;
        data.peers_have.get(file_id).cloned().unwrap_or_default()
    }
    
    pub async fn get_need_resolve(&self) -> Vec<String> {
        let data = self.data.lock().await;
        data.need_resolve.iter().cloned().collect()
    }

    pub async fn db_contains(&self, file_id: &str) -> anyhow::Result<bool> {
        self.file_db.contains(file_id).await
    }
}

pub struct ResolveResult {
    pub file_id: String,
    pub file_path: String,
}

pub struct FileResolver {
    storage: Arc<FileResolverStorage>,
    indexer: Arc<Indexer>,
    runtime: Arc<Runtime>,
    to_index_send: Arc<flume::Sender<ResolveResult>>,
    to_index_recv: Arc<flume::Receiver<ResolveResult>>,
    sync_engine: Arc<SyncEngine>,
}

impl FileResolver {
    pub fn new(
        runtime: Arc<Runtime>,
        indexer: Arc<Indexer>,
        storage: Arc<FileResolverStorage>,
        sync_engine: Arc<SyncEngine>,
    ) -> Self {
        let (to_index_send, to_index_recv) = flume::unbounded();
        Self {
            storage,
            indexer,
            runtime,
            to_index_recv: Arc::new(to_index_recv),
            to_index_send: Arc::new(to_index_send),
            sync_engine,
        }
    }

    pub fn run(self: Arc<Self>) {
        let resolver = self.clone();
        let indexer = self.clone();
        self.runtime.spawn(async move {
            let _ = resolver.clone().runtime.spawn(async move {
                resolver.run_resolve_async().await;
            });
            let _ = indexer.clone().runtime.spawn(async move {
                indexer.run_index_async().await;
            });
        });
    }

    async fn run_resolve_async(self: Arc<Self>) {
        while let Ok(want) = self.storage.to_resolve_recv.recv_async().await {
            let file_id = want.file_id;
            info!("resolve file: {}", &file_id);
            if let Some(descr) = self
                .storage
                .file_db
                .get_by_id(&file_id)
                .await
                .ok()
                .and_then(|x| x)
            {
                {
                    let mut guard = self.storage.data.lock().await;
                    guard.need_resolve.remove(&file_id);
                }
                info!("already have the file {}", &file_id);
                if let Err(e) = self
                    .to_index_send
                    .send_async(ResolveResult {
                        file_id: file_id.clone(),
                        file_path: descr.local_path,
                    })
                    .await
                {
                    log::warn!("failed to send to index: {}", e);
                }
                continue;
            }
            let mut guard = self.storage.data.lock().await;
            if let Some(peers_have) = guard.peers_have.get(&file_id) {
                let mut peers_have = peers_have.clone();
                if !want.failed_peers.is_empty() {
                    peers_have.retain(|x| !want.failed_peers.contains(x));
                    guard.peers_have.insert(file_id.clone(), peers_have.clone());
                }
                if !guard.need_resolve.contains(&file_id) || peers_have.is_empty() {
                    if peers_have.is_empty() {
                        let self_clone = self.clone();
                        let file_id = file_id.clone();
                        tokio::spawn(async move {
                            sleep(Duration::from_secs(5)).await;
                            self_clone.storage.add_need_resolve(&file_id, None).await;
                        });
                    }
                    continue;
                }
                guard.need_resolve.remove(&file_id);
                drop(guard);
                if let Err(e) = self
                    .sync_engine
                    .clone()
                    .download_file(
                        peers_have.clone(),
                        &file_id,
                        self.to_index_send.clone(),
                        self.storage.to_resolve_send.clone(),
                    )
                    .await
                {
                    log::warn!("resolve: failed to download file: {}", e);
                }
            } else {
                log::info!("resolve: no peers have the file: {}", file_id);
            }
        }
        info!("resolve finished");
    }

    async fn run_index_async(self: Arc<Self>) {
        while let Ok(res) = self.to_index_recv.recv_async().await {
            if let Err(e) = self
                .indexer
                .index_file_path(res.file_id, res.file_path)
                .await
            {
                log::warn!("failed to index file: {}", e);
            }
        }
    }

    pub async fn add_need_resolve(&self, file_id: &str, peer_id: Option<String>) {
        self.storage.add_need_resolve(file_id, peer_id).await;
    }

    pub async fn add_peer_have(&self, file_id: &str, peer_id: &str) {
        self.storage.add_peer_have(file_id, peer_id).await;
    }
}
