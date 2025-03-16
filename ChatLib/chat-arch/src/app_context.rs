use crate::{
    dialer::Dialer, events::Events, file_resolver::{FileResolver, FileResolverStorage}, indexer::Indexer, message_database::create_pool, peer_database::Peer, peer_pool::PeerPool, repository_manager::RepositoryManager, server::Server, sync_engine::SyncEngine
};
use ed25519_dalek::SigningKey;
use std::sync::{Arc, Weak};
use anyhow::anyhow;

#[derive(Clone)]
pub struct AppContext {
    pub sync_engine: Arc<SyncEngine>,
    pub file_resolver: Arc<FileResolver>,
    pub server: Arc<Server>,
    pub indexer: Arc<Indexer>,
    pub events: Arc<Events>,
    pub dialer: Arc<Dialer>,
    pub signing_key: SigningKey,
    pub peer: Peer,
    pub peer_db: Arc<crate::peer_database::PeerDatabase>,
    pub file_db: Arc<crate::file_database::FileDatabase>,
}

pub async fn prepare_deps(
    name: &str,
    addr: &str,
    root_path: &str,
    runtime: Arc<tokio::runtime::Runtime>,
) -> anyhow::Result<AppContext> {
    let events = Arc::new(Events::new());
    let db_pool = create_pool(root_path).await?;
    
    let peer_db = Arc::new(crate::peer_database::PeerDatabase::new(db_pool.clone(), events.clone()));
    peer_db.init().await?;
    let existing_peer = match peer_db.get_local_peer().await? {
        Some(peer) => peer,
        None => peer_db.create_local_peer(Some(name.to_owned())).await?,
    };
    
    let message_db = Arc::new(crate::message_database::MessageDatabase::new(
        db_pool.clone(),
    ));
    let counter = message_db.init().await?.unwrap_or_else(|| 1);

    let file_db = Arc::new(crate::file_database::FileDatabase::new(db_pool.clone()));
    file_db.init().await?;
    let file_storage = Arc::new(FileResolverStorage::new(file_db.clone()));

    let index_db = crate::index_database::IndexedMessageDatabase::new(db_pool.clone());
    index_db.init().await?;
    let indexer = Arc::new(Indexer::new(index_db, file_db.clone(), events.clone()));
    let cloned_indexer = indexer.clone();

    let signing_key = existing_peer.signing_key.clone().ok_or(anyhow!("no signing key"))?;
    let peer_id = hex::encode(signing_key.verifying_key().to_bytes());

    let dialer = Arc::new(Dialer::new(signing_key.clone()));
    let dialer_clone = dialer.clone();

    let sync_engine = Arc::new_cyclic(|weak: &Weak<SyncEngine>| {
        let manager = Arc::new(RepositoryManager::new(
            message_db,
            counter,
            cloned_indexer,
            weak.clone(),
        ));
        let peer_pool = Arc::new(PeerPool::new(dialer_clone, weak.clone(), runtime.clone()));
        SyncEngine::new(
            peer_id.clone(),
            root_path.to_owned(),
            peer_pool.clone(),
            peer_db.clone(),
            manager,
            file_storage.clone(),
            events.clone(),
            runtime.clone(),
        )
    });

    let server = Server::new(
        addr.to_owned(),
        signing_key.clone(),
        sync_engine.peer_pool.clone(),
        runtime.clone(),
    );

    let file_resolver = Arc::new(FileResolver::new(
        runtime,
        indexer.clone(),
        file_storage,
        sync_engine.clone(),
    ));

    Ok(AppContext {
        indexer,
        sync_engine,
        file_resolver,
        server: Arc::new(server),
        events,
        dialer,
        signing_key,
        peer: existing_peer,
        peer_db,
        file_db,
    })
}