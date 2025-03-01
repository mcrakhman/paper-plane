use std::sync::Arc;

use crate::{
    events::Events, file_database::FileDatabase, index_database::IndexedMessageDatabase, models::{DbMessage, IndexedMessage}, proto::chat::MessagePayload
};
use anyhow::Result;
use log::info;
use prost::Message;

pub struct Indexer {
    db: IndexedMessageDatabase,
    file_db: Arc<FileDatabase>,
    events: Arc<Events>,
}

impl Indexer {
    pub fn new(db: IndexedMessageDatabase, file_db: Arc<FileDatabase>, events: Arc<Events>) -> Self {
        Self { db, file_db, events }
    }

    async fn process_message(&self, msg: &DbMessage) -> Result<IndexedMessage> {
        let payload = MessagePayload::decode(&*msg.payload)?;
        let file_path = if !payload.file_id.is_empty() {
            let file = self.file_db.get_by_id(&payload.file_id).await?;
            if let Some(descr) = file {
                Some(descr.local_path)
            } else {
                None
            }
        } else {
            None
        };
        let indexed_message = IndexedMessage {
            id: msg.id.clone(),
            order_id: format!("{:08}-{}", msg.order, msg.peer_id),
            mentions: payload.mentions.clone(),
            reply: if payload.reply_id.is_empty() {
                None
            } else {
                Some(payload.reply_id)
            },
            text: payload.text,
            file_id: if payload.file_id.is_empty() {
                None
            } else {
                Some(payload.file_id)
            },
            file_path,
            peer_id: msg.peer_id.clone(),
        };

        Ok(indexed_message)
    }
    
    pub async fn index_file_path(&self, file_id: String, file_path: String) -> Result<()> {
        info!("updating file path {}, {}", &file_id, &file_path);
        let messages = self.db.update_file_id(&file_id, &file_path).await?;
        for msg in messages {
            self.events.send_message(msg).await?;
        }
        Ok(())
    }

    pub async fn index_message(&self, msg: &DbMessage) -> Result<()> {
        let indexed_message = self.process_message(msg).await?;
        self.db.save(&indexed_message).await?;
        self.events.send_message(indexed_message).await?;
        Ok(())
    }

    pub async fn index_messages<'a, I>(&self, messages: I) -> Result<()>
    where
        I: IntoIterator<Item = &'a DbMessage>,
    {
        for msg in messages {
            self.index_message(msg).await?;
        }
        Ok(())
    }

    pub async fn get_all_after_order_id(&self, order_id: &str) -> Result<Vec<IndexedMessage>> {
        self.db.get_all_after_order_id(order_id).await
    }
}
