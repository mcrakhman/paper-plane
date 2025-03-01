use crate::indexer::Indexer;
use crate::message_database::MessageDatabase;
use crate::models::DbMessage;
use crate::repository::Repository;
use crate::sync_engine::MessageBroadcaster;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct RepositoryManager {
    repositories: Arc<Mutex<HashMap<String, Arc<Mutex<Repository>>>>>,
    db: Arc<MessageDatabase>,
    indexer: Arc<Indexer>,
    sync_engine: std::sync::Weak<dyn MessageBroadcaster>,
    counter_lock: Arc<Mutex<u64>>,
}

#[derive(Clone, Debug)]
pub struct RepoState {
    pub counter: u64,
    pub peer_id: String,
}

impl RepositoryManager {
    pub fn new(
        db: Arc<MessageDatabase>,
        counter: u64,
        indexer: Arc<Indexer>,
        sync_engine: std::sync::Weak<dyn MessageBroadcaster>,
    ) -> Self {
        Self {
            repositories: Arc::new(Mutex::new(HashMap::new())),
            db,
            indexer,
            sync_engine,
            counter_lock: Arc::new(Mutex::new(counter)),
        }
    }

    pub async fn update_counter_many<'a, I>(&self, messages: I) -> Result<()>
    where
        I: IntoIterator<Item = &'a DbMessage>,
    {
        let mut cnt = self.counter_lock.lock().await;
        for message in messages {
            if message.order > *cnt {
                *cnt = message.order;
            }
        }
        Ok(())
    }

    pub async fn update_counter(&self, message: &DbMessage) -> Result<()> {
        let mut cnt = self.counter_lock.lock().await;
        if message.order > *cnt {
            *cnt = message.order;
        }
        Ok(())
    }

    pub async fn add_own_message(self: Arc<Self>, mut message: DbMessage) -> Result<DbMessage> {
        let self_clone = self.clone();
        let mut cnt = self_clone.counter_lock.lock().await;
        *cnt += 1;
        message.order = *cnt;
        drop(cnt);
        let repository = self.get_or_create_repository(&message.peer_id).await?;
        repository.lock().await.save_message(&message).await?;
        Ok(message)
    }

    async fn get_or_create_repository(
        self: Arc<Self>,
        peer_id: &str,
    ) -> Result<Arc<Mutex<Repository>>> {
        let mut repositories = self.repositories.lock().await;
        println!("creating repository: {}", peer_id);
        if let Some(repo) = repositories.get(peer_id) {
            Ok(repo.clone())
        } else {
            let weak_self = Arc::downgrade(&self);
            let repository = Repository::new(
                peer_id.to_string(),
                self.db.clone(),
                self.indexer.clone(),
                self.sync_engine.clone(),
                weak_self,
            )
            .await?;

            let repository = Arc::new(Mutex::new(repository));
            repositories.insert(peer_id.to_string(), repository.clone());

            Ok(repository.clone())
        }
    }

    pub async fn get_repo_states(self: Arc<Self>) -> Result<Vec<RepoState>> {
        let self_clone = self.clone();
        let peer_ids = self.db.get_peers().await?;
        let mut states = Vec::new();
        for peer_id in peer_ids {
            let repo = self_clone.clone().get_repository(&peer_id).await?;
            states.push(RepoState {
                counter: repo.lock().await.get_counter(),
                peer_id: peer_id.clone(),
            });
        }
        Ok(states)
    }

    pub async fn get_repository(self: Arc<Self>, peer_id: &str) -> Result<Arc<Mutex<Repository>>> {
        self.get_or_create_repository(peer_id).await
    }
}
