use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use log::{debug, info, warn};
use std::path::Path;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};
use tokio_yamux::StreamHandle;

use crate::{
    events::Events,
    file_resolver::{FileResolverStorage, ResolveResult, ResolveWant},
    models::DbMessage,
    peer::PeerDelegate,
    peer_pool::EncryptedPool,
    proto::{
        self,
        chat::{chat_message, ChatMessage, ComparePayload},
    },
    repository_manager::{RepoState, RepositoryManager},
    request_queue::{AsyncFn, BoxFuture, PeriodicTaskScheduler, RequestQueue, Task},
    stream_protocol::StreamProtocol,
};

#[async_trait]
pub trait FileProvider: Send + Sync {
    async fn download_file(
        self: Arc<Self>,
        peer_ids: Vec<String>,
        file_id: &str,
        to_index_send: Arc<flume::Sender<ResolveResult>>,
        to_resolve_send: Arc<flume::Sender<ResolveWant>>,
    ) -> anyhow::Result<()>;
}

#[async_trait]
pub trait MessageBroadcaster: Send + Sync {
    async fn message_broadcast(self: Arc<Self>, sync_message: SyncMessage) -> anyhow::Result<()>;
}

#[derive(Debug, Clone)]
pub struct SyncMessage {
    pub stored_messages: Vec<DbMessage>,
}

pub struct SyncEngine {
    id: String,
    root_path: String,
    request_queue: Arc<RequestQueue>,
    task_scheduler: PeriodicTaskScheduler,
    pub peer_pool: Arc<EncryptedPool>,
    repos: Arc<RepositoryManager>,
    runtime: Arc<tokio::runtime::Runtime>,
    file_storage: Arc<FileResolverStorage>,
}

impl SyncEngine {
    pub fn new(
        id: String,
        root_path: String,
        peer_pool: Arc<EncryptedPool>,
        manager: Arc<RepositoryManager>,
        file_storage: Arc<FileResolverStorage>,
        events: Arc<Events>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        let cloned_repos = manager.clone();
        let rq = Arc::new(RequestQueue::new(10, runtime.clone()));
        let cloned_rq = rq.clone();
        let pool = peer_pool.clone();
        let events_clone = events.clone();
        let file_storage_clone = file_storage.clone();
        let async_task: Arc<AsyncFn> = Arc::new(move || {
            let repo_clone = cloned_repos.clone();
            let rq = cloned_rq.clone();
            let pool_clone = pool.clone();
            let file_storage = file_storage_clone.clone();
            let events = events_clone.clone();
            Box::pin(async move {
                debug!("getting repo states");
                let file_ids = file_storage.get_need_resolve().await;
                if let Ok(repo_states) = repo_clone.clone().get_repo_states().await {
                    info!("got repo states {:?}", &repo_states);
                    let current_peers = pool_clone.all_peers().await;
                    info!("current peers are {:?}", &current_peers);
                    for peer in current_peers {
                        let task = CompareStateTask {
                            peer_id: peer.clone(),
                            repo_states: repo_states.clone(),
                            pool: pool_clone.clone(),
                            rq: rq.clone(),
                            manager: repo_clone.clone(),
                        };
                        rq.enqueue(Arc::new(task)).await?;
                        let task = FileWantTask {
                            peer_id: peer.clone(),
                            file_ids: file_ids.clone(),
                            pool: pool_clone.clone(),
                            file_storage: file_storage.clone(),
                        };
                        rq.enqueue(Arc::new(task)).await?;
                    }
                }
                Ok(())
            })
        });
        let task_scheduler = PeriodicTaskScheduler::new(async_task, 10, runtime.clone());
        SyncEngine {
            id,
            root_path,
            request_queue: rq,
            peer_pool,
            repos: manager,
            task_scheduler,
            file_storage,
            runtime,
        }
    }

    pub fn get_manager(&self) -> Arc<RepositoryManager> {
        self.repos.clone()
    }

    pub fn run(&self) {
        self.task_scheduler.signal_start();
        self.request_queue.start();
    }

    pub async fn handle_request(
        self: Arc<Self>,
        stream: &mut StreamHandle,
        peer_id: String,
    ) -> anyhow::Result<()> {
        let protocol = StreamProtocol::new();
        let req = protocol.read_request::<_, ChatMessage>(stream).await?;
        let req = req.variant.unwrap();
        match req {
            chat_message::Variant::FileDownloadRequest(req) => {
                info!("receive download request: {:?}", req);
                let full_path = self
                    .file_storage
                    .file_db
                    .get_by_id(&req.file_id)
                    .await?
                    .ok_or(anyhow::anyhow!("file not found"))?;
                let full_path = Path::new(&self.root_path)
                    .join(&full_path.local_path)
                    .to_string_lossy()
                    .to_string();
                return upload_file(stream, &full_path).await;
            }
            chat_message::Variant::Messages(msg) => {
                let repo = self.repos.clone().get_repository(&msg.peer_id).await?;
                let guard = repo.lock().await;
                let db_messages: Vec<DbMessage> =
                    msg.messages.into_iter().map(|m| m.into()).collect();
                if let Err(err) = guard.insert_message_batch(&db_messages).await {
                    info!("failed to save messages: {} {:?}", &peer_id, err);
                }
                let resp = ChatMessage {
                    variant: Some(chat_message::Variant::MessageAccept(
                        crate::proto::chat::MessageAccept {
                            counter: guard.get_counter() as i32,
                        },
                    )),
                };
                drop(guard);
                protocol
                    .send_response::<_, ChatMessage>(stream, &resp)
                    .await?;
                protocol.send_eof(stream).await?;
                close_stream(stream).await?;
                return Ok(());
            }
            chat_message::Variant::FileWantRequest(msg) => {
                let all_file_ids = self.file_storage.file_db.all_file_ids().await?;
                let mut hash_set = HashSet::with_capacity(all_file_ids.len());
                for file_id in all_file_ids.iter() {
                    hash_set.insert(file_id);
                }
                let mut result = Vec::with_capacity(all_file_ids.len());
                for file_id in &msg.file_id {
                    if hash_set.contains(file_id) {
                        result.push(file_id.clone());
                    }
                }
                let resp = ChatMessage {
                    variant: Some(chat_message::Variant::FileWantResponse(
                        crate::proto::chat::FileWantResponse { file_id: result },
                    )),
                };
                protocol.send_response(stream, &resp).await?;
                protocol.send_eof(stream).await?;
                close_stream(stream).await?;
                return Ok(());
            }
            chat_message::Variant::BatchMessageRequest(msg) => {
                let repo = self.repos.clone().get_repository(&msg.peer_id).await?;
                let guard = repo.lock().await;
                let my_counter = guard.get_counter();
                let their_counter = msg.my_counter as u64;
                println!(
                    "counters: {}, {}, {}",
                    my_counter, msg.my_counter, msg.peer_id
                );
                let resp: ChatMessage;
                if their_counter >= my_counter {
                    resp = ChatMessage {
                        variant: Some(chat_message::Variant::BatchMessageResponse(
                            crate::proto::chat::BatchMessageResponse { messages: vec![] },
                        )),
                    };
                } else {
                    let messages = guard.get_messages(their_counter).await?;
                    let resp_messages = messages.into_iter().map(|m| m.into()).collect();
                    resp = ChatMessage {
                        variant: Some(chat_message::Variant::BatchMessageResponse(
                            crate::proto::chat::BatchMessageResponse {
                                messages: resp_messages,
                            },
                        )),
                    };
                }
                drop(guard);
                protocol.send_response(stream, &resp).await?;
                protocol.send_eof(stream).await?;
                close_stream(stream).await?;
                return Ok(());
            }
            chat_message::Variant::CompareRequest(msg) => {
                let my_states = self.repos.clone().get_repo_states().await?;
                let mut peer_ids = vec![];
                for state in my_states {
                    let mut spotted = false;
                    let state_id = state.peer_id.clone();
                    for other_state in &msg.compare_payload {
                        if other_state.peer_id == state.peer_id {
                            spotted = true;
                            if other_state.counter < state.counter as i32 {
                                peer_ids.push(state_id);
                                break;
                            }
                        }
                    }
                    if !spotted {
                        peer_ids.push(state.peer_id.clone());
                    }
                }
                let resp = ChatMessage {
                    variant: Some(chat_message::Variant::CompareResponse(
                        crate::proto::chat::CompareResponse { peer_ids },
                    )),
                };
                protocol.send_response(stream, &resp).await?;
                protocol.send_eof(stream).await?;
                close_stream(stream).await?;
                return Ok(());
            }
            _ => {
                warn!("unknown message");
                return Err(anyhow::anyhow!("something went wrong!"));
            }
        };
    }
}

#[async_trait]
impl FileProvider for SyncEngine {
    async fn download_file(
        self: Arc<Self>,
        peer_ids: Vec<String>,
        file_id: &str,
        to_index_send: Arc<flume::Sender<ResolveResult>>,
        to_resolve_send: Arc<flume::Sender<ResolveWant>>,
    ) -> anyhow::Result<()> {
        info!(
            "resolve: downloading file {} to {}, peer_ids {:?}",
            file_id, &self.root_path, peer_ids
        );
        let task = FileTask {
            index_sender: to_index_send,
            resolve_sender: to_resolve_send,
            file_id: file_id.to_string(),
            file_storage: self.file_storage.clone(),
            folder: self.root_path.clone(),
            peer_ids,
            pool: self.peer_pool.clone(),
        };
        self.request_queue.enqueue(Arc::new(task)).await?;
        Ok(())
    }
}

#[async_trait]
impl MessageBroadcaster for SyncEngine {
    async fn message_broadcast(self: Arc<Self>, sync_message: SyncMessage) -> anyhow::Result<()> {
        let current_peers = self.peer_pool.current_peers().await;
        if sync_message.stored_messages.is_empty() {
            panic!("empty messages");
        }
        for peer in current_peers {
            let task = MessageTask {
                peer_id: peer.clone(),
                messages: sync_message.stored_messages.clone(),
                pool: self.peer_pool.clone(),
            };
            self.request_queue.enqueue(Arc::new(task)).await?;
        }
        Ok(())
    }
}

impl PeerDelegate for SyncEngine {
    fn handle_inbound_stream(
        self: Arc<Self>,
        mut stream: StreamHandle,
        peer_id: String,
    ) -> anyhow::Result<()> {
        let self_clone = self.clone();
        self.clone().runtime.spawn(async move {
            if let Err(e) = self_clone.handle_request(&mut stream, peer_id).await {
                warn!("error handling download request: {:?}", e);
            }
            stream.flush().await;
            stream.shutdown().await;
        });
        Ok(())
    }
}

async fn close_stream(stream: &mut StreamHandle) -> anyhow::Result<()> {
    stream.flush().await?;
    stream.shutdown().await?;
    Ok(())
}

pub async fn upload_file(stream: &mut StreamHandle, filename: &str) -> anyhow::Result<()> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let protocol = StreamProtocol::new();
    let mut file = tokio::fs::File::open(&filename).await?;
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            let final_chunk = ChatMessage {
                variant: Some(chat_message::Variant::FileDownloadResponse(
                    crate::proto::chat::FileDownloadResponse {
                        ext: ext.to_string(),
                        chunk: vec![],
                        last_chunk: true,
                    },
                )),
            };
            protocol.send_response(stream, &final_chunk).await?;
            protocol.send_eof(stream).await?;
            break;
        }
        let chunk_proto = ChatMessage {
            variant: Some(chat_message::Variant::FileDownloadResponse(
                crate::proto::chat::FileDownloadResponse {
                    ext: ext.to_string(),
                    chunk: buffer[..n].to_vec(),
                    last_chunk: false,
                },
            )),
        };
        protocol.send_response(stream, &chunk_proto).await?;
    }
    Ok(())
}

pub struct BatchRequestTask {
    pub counter: u64,
    pub peer_id: String,
    pub repo_id: String,
    pub pool: Arc<EncryptedPool>,
    pub repo_manager: Arc<RepositoryManager>,
}

impl Task for BatchRequestTask {
    fn run(self: Arc<Self>) -> BoxFuture<'static, anyhow::Result<()>> {
        let self_clone = self.clone();
        Box::pin(async move {
            let pool = self_clone.pool.clone();
            let peer = pool.get(&self_clone.peer_id).await?;
            let mut stream = peer.open_stream().await?;
            let protocol = StreamProtocol::new();
            let req = ChatMessage {
                variant: Some(chat_message::Variant::BatchMessageRequest(
                    crate::proto::chat::BatchMessageRequest {
                        my_counter: self_clone.counter as i32,
                        peer_id: self_clone.repo_id.clone(),
                    },
                )),
            };
            protocol.send_request(&mut stream, &req).await?;
            debug!(
                "sent request {:?}, peer {}, repo {}",
                &req, &self_clone.peer_id, &self_clone.repo_id
            );
            let resp = protocol
                .read_response::<_, ChatMessage>(&mut stream)
                .await?
                .and_then(|r| r.variant);
            if resp.is_none() {
                return Err(anyhow::anyhow!("unexpected response"));
            }
            match resp.unwrap() {
                chat_message::Variant::BatchMessageResponse(resp) => {
                    let messages: Vec<DbMessage> =
                        resp.messages.into_iter().map(|m| m.into()).collect();
                    info!(
                        "received response, peer {}, repo {}",
                        &self_clone.peer_id, &self_clone.repo_id
                    );
                    let repo = self_clone
                        .repo_manager
                        .clone()
                        .get_repository(&self_clone.repo_id)
                        .await?;
                    let guard = repo.lock().await;
                    guard.insert_message_batch(&messages).await?;
                }
                _ => return Err(anyhow::anyhow!("unexpected response")),
            }
            Ok(())
        })
    }
}

pub struct MessageTask {
    pub peer_id: String,
    pub messages: Vec<DbMessage>,
    pub pool: Arc<EncryptedPool>,
}

impl Task for MessageTask {
    fn run(self: Arc<Self>) -> BoxFuture<'static, anyhow::Result<()>> {
        let self_clone = self.clone();
        Box::pin(async move {
            let pool = self_clone.pool.clone();
            let peer_id = self_clone.peer_id.clone();
            let peer = match pool.get(&peer_id).await {
                Ok(peer) => peer,
                Err(e) => {
                    warn!("Failed to get peer: {:?}", e);
                    return Err(e);
                }
            };
            let mut stream = peer.open_stream().await?;
            let protocol = StreamProtocol::new();
            let peer_id = self_clone.messages[0].peer_id.clone();
            let req = ChatMessage {
                variant: Some(chat_message::Variant::Messages(proto::chat::Messages {
                    messages: self_clone
                        .messages
                        .iter()
                        .map(|m| m.clone().into())
                        .collect(),
                    peer_id,
                })),
            };
            protocol.send_request(&mut stream, &req).await?;
            let resp = protocol
                .read_response::<_, ChatMessage>(&mut stream)
                .await?
                .and_then(|r| r.variant);
            if resp.is_none() {
                return Err(anyhow::anyhow!("unexpected response"));
            }
            match resp.unwrap() {
                chat_message::Variant::MessageAccept(resp) => {
                    info!(
                        "received response, {:?}, peer {}",
                        resp, &self_clone.peer_id
                    );
                    return Ok(());
                }
                _ => return Err(anyhow::anyhow!("unexpected response")),
            }
        })
    }
}

pub struct FileTask {
    file_id: String,
    folder: String,
    peer_ids: Vec<String>,
    index_sender: Arc<flume::Sender<ResolveResult>>,
    resolve_sender: Arc<flume::Sender<ResolveWant>>,
    file_storage: Arc<FileResolverStorage>,
    pool: Arc<EncryptedPool>,
}

impl FileTask {
    async fn download_file(self: Arc<Self>, path: &str, peer_id: String) -> anyhow::Result<String> {
        let pool = self.pool.clone();
        let peer = pool.get(&peer_id).await?;
        let mut stream = peer.open_stream().await?;
        let protocol = StreamProtocol::new();
        let req = ChatMessage {
            variant: Some(chat_message::Variant::FileDownloadRequest(
                crate::proto::chat::FileDownloadRequest {
                    file_id: self.file_id.clone(),
                    peer_id: peer_id.clone(),
                },
            )),
        };
        protocol.send_request(&mut stream, &req).await?;
        let mut file = tokio::fs::File::create(&path).await?;
        let mut ext: String = "".to_string();
        loop {
            let resp = protocol
                .read_response::<_, ChatMessage>(&mut stream)
                .await?;
            if resp.is_none() {
                break;
            }
            match resp.and_then(|r| r.variant) {
                Some(resp) => match resp {
                    chat_message::Variant::FileDownloadResponse(resp) => {
                        ext = resp.ext.clone();
                        file.write_all(&resp.chunk).await?;
                    }
                    _ => return Err(anyhow::anyhow!("unexpected response")),
                },
                _ => return Err(anyhow::anyhow!("unexpected response")),
            }
        }
        let new_path = format!("{}.{}", &path, &ext);
        fs::rename(&path, &new_path).await?;
        info!("renaming {} to {}", &path, &new_path);
        let local_path = &new_path[self.folder.len() + 1..];
        self.file_storage
            .file_db
            .save(&crate::file_database::FileDescription {
                id: self.file_id.clone(),
                format: ext.clone(),
                local_path: local_path.to_owned(),
                timestamp: chrono::Utc::now().timestamp(),
            })
            .await?;
        Ok(local_path.to_string())
    }
}

impl Task for FileTask {
    fn run(self: Arc<Self>) -> BoxFuture<'static, anyhow::Result<()>> {
        let self_clone = self.clone();
        Box::pin(async move {
            tokio::fs::create_dir_all(&self.folder).await?;
            let path = Path::new(&self.folder).join(&self.file_id);
            for peer_id in self.peer_ids.iter() {
                match self_clone
                    .clone()
                    .download_file(&path.to_string_lossy(), peer_id.clone())
                    .await
                {
                    Ok(res) => {
                        self.index_sender
                            .send_async(ResolveResult {
                                file_id: self.file_id.clone(),
                                file_path: res,
                            })
                            .await;
                        return Ok(());
                    }
                    Err(e) => {
                        info!("failed to download file: {:?}, {}", e, &peer_id);
                        tokio::fs::remove_file(path.clone()).await;
                    }
                };
            }
            let res = self
                .resolve_sender
                .send_async(ResolveWant {
                    file_id: self.file_id.clone(),
                    failed_peers: self.peer_ids.clone(),
                })
                .await;
            Ok(())
        })
    }
}

pub struct CompareStateTask {
    peer_id: String,
    repo_states: Vec<RepoState>,
    pool: Arc<EncryptedPool>,
    rq: Arc<RequestQueue>,
    manager: Arc<RepositoryManager>,
}

impl Task for CompareStateTask {
    fn run(self: Arc<Self>) -> BoxFuture<'static, anyhow::Result<()>> {
        let self_clone = self.clone();
        Box::pin(async move {
            let peer_id = self_clone.peer_id.clone();
            let pool = self_clone.pool.clone();
            let peer = match pool.get(&peer_id).await {
                Ok(peer) => peer,
                Err(e) => {
                    warn!("Failed to get peer: {:?}", e);
                    return Err(e);
                }
            };
            let mut stream = peer.open_stream().await?;
            let protocol = StreamProtocol::new();
            let payloads = self_clone
                .repo_states
                .iter()
                .map(|s| ComparePayload {
                    counter: s.counter as i32,
                    peer_id: s.peer_id.clone(),
                })
                .collect::<Vec<crate::proto::chat::ComparePayload>>();
            let req = ChatMessage {
                variant: Some(chat_message::Variant::CompareRequest(
                    crate::proto::chat::CompareRequest {
                        compare_payload: payloads,
                    },
                )),
            };
            protocol.send_request(&mut stream, &req).await?;
            let resp = protocol
                .read_response::<_, ChatMessage>(&mut stream)
                .await?
                .and_then(|r| r.variant);
            if resp.is_none() {
                return Err(anyhow::anyhow!("unexpected response"));
            }
            match resp.unwrap() {
                chat_message::Variant::CompareResponse(resp) => {
                    info!(
                        "received response, {:?}, peer {}",
                        resp, &self_clone.peer_id
                    );
                    let repo_states_iter = self_clone
                        .repo_states
                        .iter()
                        .filter(|state| resp.peer_ids.contains(&state.peer_id));
                    for state in repo_states_iter {
                        let task = BatchRequestTask {
                            repo_id: state.peer_id.clone(),
                            counter: state.counter,
                            peer_id: self_clone.peer_id.clone(),
                            pool: pool.clone(),
                            repo_manager: self_clone.manager.clone(),
                        };
                        self_clone.rq.enqueue(Arc::new(task)).await?;
                    }
                    let peer_iter = resp.peer_ids.iter().filter(|id| {
                        !self_clone
                            .repo_states
                            .iter()
                            .any(|state| state.peer_id == **id)
                    });
                    for peer_id in peer_iter {
                        let task = BatchRequestTask {
                            counter: 0,
                            repo_id: peer_id.clone(),
                            peer_id: self_clone.peer_id.clone(),
                            pool: pool.clone(),
                            repo_manager: self_clone.manager.clone(),
                        };
                        self_clone.rq.enqueue(Arc::new(task)).await?;
                    }
                    return Ok(());
                }
                _ => return Err(anyhow::anyhow!("unexpected response")),
            }
        })
    }
}

struct FileWantTask {
    peer_id: String,
    file_ids: Vec<String>,
    pool: Arc<EncryptedPool>,
    file_storage: Arc<FileResolverStorage>,
}

impl Task for FileWantTask {
    fn run(self: Arc<Self>) -> BoxFuture<'static, anyhow::Result<()>> {
        let self_clone = self.clone();
        Box::pin(async move {
            let peer_id = self_clone.peer_id.clone();
            debug!("file want {} {:?}", &peer_id, &self.file_ids);
            let pool = self_clone.pool.clone();
            let peer = match pool.get(&peer_id).await {
                Ok(peer) => peer,
                Err(e) => {
                    warn!("Failed to get peer: {:?}", e);
                    return Err(e);
                }
            };
            let mut stream = peer.open_stream().await?;
            let protocol = StreamProtocol::new();
            let req = ChatMessage {
                variant: Some(chat_message::Variant::FileWantRequest(
                    crate::proto::chat::FileWantRequest {
                        file_id: self_clone.file_ids.clone(),
                    },
                )),
            };
            protocol.send_request(&mut stream, &req).await?;
            let resp = protocol
                .read_response::<_, ChatMessage>(&mut stream)
                .await?
                .and_then(|r| r.variant);
            if resp.is_none() {
                return Err(anyhow::anyhow!("unexpected response"));
            }
            match resp.unwrap() {
                chat_message::Variant::FileWantResponse(resp) => {
                    info!(
                        "received response, {:?}, peer {}",
                        resp, &self_clone.peer_id
                    );
                    self.file_storage
                        .add_peer_have_many(resp.file_id, &peer_id)
                        .await;
                    return Ok(());
                }
                _ => return Err(anyhow::anyhow!("unexpected response")),
            }
        })
    }
}
