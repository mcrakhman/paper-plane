use crate::models::IndexedMessage;
use log::warn;

pub enum ChatEvent {
    Message(IndexedMessage),
}

pub struct Events {
    tx: flume::Sender<ChatEvent>,
    rx: flume::Receiver<ChatEvent>,
}

impl Events {
    pub fn new() -> Self {
        let (tx, rx) = flume::unbounded();
        Self { tx, rx }
    }

    pub fn get_rx(&self) -> flume::Receiver<ChatEvent> {
        self.rx.clone()
    }

    pub async fn start_loop(&self) {
        while let Ok(event) = self.rx.recv_async().await {
            match event {
                ChatEvent::Message(message) => {
                    warn!("message received: {:?}", message);
                }
            }
        }
    }

    pub async fn send_message(&self, message: IndexedMessage) -> anyhow::Result<()> {
        self.tx.send_async(ChatEvent::Message(message)).await?;
        Ok(())
    }
}
