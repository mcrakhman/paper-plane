use crate::message_database::MessageDatabase;
use crate::models::DbMessage;
use crate::sync_engine::{MessageBroadcaster, SyncMessage};
use crate::{indexer::Indexer, repository_manager::RepositoryManager};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Weak;

pub struct Repository {
    pub id: String,
    db: Arc<MessageDatabase>,
    cur_counter: Arc<AtomicU64>,
    sync_engine: Weak<dyn MessageBroadcaster>,
    manager: Weak<RepositoryManager>,
    indexer: Arc<Indexer>,
}

impl Repository {
    pub async fn new(
        id: String,
        db: Arc<MessageDatabase>,
        indexer: Arc<Indexer>,
        sync_engine: Weak<dyn MessageBroadcaster>,
        manager: Weak<RepositoryManager>,
    ) -> anyhow::Result<Self> {
        let res = match db.get_highest_counter(&id).await {
            Ok(res) => res,
            Err(_) => 0,
        };
        let repo = Self {
            id,
            db,
            sync_engine,
            indexer,
            cur_counter: Arc::new(AtomicU64::new(res)),
            manager,
        };
        Ok(repo)
    }

    pub fn get_counter(&self) -> u64 {
        self.cur_counter.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn save_message(&self, message: &DbMessage) -> anyhow::Result<()> {
        let mut message = message.clone();
        if message.peer_id != self.id {
            return Err(anyhow::anyhow!(
                "Message peer_id does not match repository id"
            ));
        } else {
            let mut add = false;
            if message.counter == 0 {
                add = true;
                message.counter = self
                    .cur_counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1;
            } else if message.counter
                != self.cur_counter.load(std::sync::atomic::Ordering::SeqCst) + 1
            {
                return Err(anyhow::anyhow!("Message counter is invalid"));
            }
            self.db.save(&message).await?;
            if !add {
                self.cur_counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            if let Some(upgrade) = self.manager.upgrade() {
                upgrade.update_counter(&message).await?;
            }
        }
        self.indexer.index_message(&message).await?;
        self.sync_engine
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("SyncEngine is gone"))?
            .message_broadcast(SyncMessage {
                stored_messages: vec![message.clone()],
            })
            .await
    }

    pub async fn get_messages(&self, start_counter: u64) -> anyhow::Result<Vec<DbMessage>> {
        self.db.get_after(&self.id, start_counter).await
    }

    pub async fn insert_message_batch(&self, messages: &[DbMessage]) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let counter = self.cur_counter.load(std::sync::atomic::Ordering::SeqCst);
        let filtered: Vec<&DbMessage> = messages
            .iter()
            .filter(|&msg| msg.counter > counter)
            .collect();
        let mut total = 0;
        for (i, msg) in filtered.iter().enumerate() {
            if msg.peer_id != self.id {
                return Err(anyhow::anyhow!(
                    "Message peer_id does not match repository id"
                ));
            }
            if msg.counter != counter + i as u64 + 1 {
                return Err(anyhow::anyhow!("Message counter is invalid"));
            }
            total += 1;
        }
        if total == 0 {
            return Ok(());
        }
        self.db.save_many(filtered.clone()).await?;
        if let Some(upgrade) = self.manager.upgrade() {
            upgrade.update_counter_many(filtered.clone()).await?;
        }
        self.cur_counter
            .fetch_add(total as u64, std::sync::atomic::Ordering::SeqCst);
        self.indexer.index_messages(filtered.clone()).await?;
        self.sync_engine
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("SyncEngine is gone"))?
            .message_broadcast(SyncMessage {
                stored_messages: filtered.into_iter().map(|msg| msg.clone()).collect(),
            })
            .await;
        Ok(())
    }

    pub async fn get_state(&self) -> anyhow::Result<u64> {
        Ok(self.cur_counter.load(std::sync::atomic::Ordering::SeqCst))
    }
}
