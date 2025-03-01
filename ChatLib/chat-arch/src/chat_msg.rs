use crate::{proto::chat::ChatMessage, stream_protocol::MessageEncoding};
use anyhow::{anyhow, Result};
use prost::Message;

impl MessageEncoding for ChatMessage {
    fn encode_message(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        self.encode(&mut buf).expect("prost encode");
        buf
    }

    fn decode_message(bytes: &[u8]) -> Result<Self> {
        ChatMessage::decode(bytes).map_err(|e| anyhow!("Decode error: {}", e))
    }
}
