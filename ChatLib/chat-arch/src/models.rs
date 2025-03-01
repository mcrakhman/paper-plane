use serde::{Deserialize, Serialize};

use crate::proto::chat::{Message, MessagePayload};

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DbMessage {
    pub counter: u64,
    pub id: String,
    pub order: u64,
    pub timestamp: i64,
    pub payload: Vec<u8>,
    pub peer_id: String,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct IndexedMessage {
    pub id: String,
    pub order_id: String,
    pub mentions: Vec<String>,
    pub reply: Option<String>,
    pub text: String,
    pub file_id: Option<String>,
    pub file_path: Option<String>,
    pub peer_id: String,
}

impl From<Message> for DbMessage {
    fn from(message: Message) -> Self {
        DbMessage {
            id: message.id,
            counter: message.counter as u64,
            timestamp: message.timestamp,
            order: message.global_counter as u64,
            payload: message.payload,
            peer_id: message.peer_id,
        }
    }
}

impl Into<Message> for DbMessage {
    fn into(self) -> Message {
        Message {
            counter: self.counter as i32,
            timestamp: self.timestamp,
            global_counter: self.order as i64,
            payload: self.payload,
            peer_id: self.peer_id,
            id: self.id,
        }
    }
}

pub struct MessageBuilder {
    id: String,
    timestamp: i64,
    peer_id: String,
    text: Option<String>,
    file_id: Option<String>,
}

impl MessageBuilder {
    pub fn new(id: String, timestamp: i64, peer_id: String) -> Self {
        Self {
            id,
            timestamp,
            peer_id,
            text: None,
            file_id: None,
        }
    }

    pub fn text(mut self, text: String) -> Self {
        self.text = Some(text);
        self
    }

    pub fn file_id(mut self, file_id: String) -> Self {
        self.file_id = Some(file_id);
        self
    }

    pub fn build(self) -> DbMessage {
        let payload = MessagePayload {
            text: self.text.unwrap_or_default(),
            file_id: self.file_id.unwrap_or_default(),
            reply_id: String::new(),
            mentions: Vec::new(),
        };

        let payload_bytes = prost::Message::encode_to_vec(&payload);

        DbMessage {
            id: self.id,
            counter: 0,
            timestamp: self.timestamp,
            order: 0,
            payload: payload_bytes,
            peer_id: self.peer_id,
        }
    }
}
