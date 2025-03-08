use crate::models::IndexedMessage;
use log::warn;
use crate::peer_database::Peer;

pub enum ChatEvent {
    Message(IndexedMessage),
    Peer(Peer),
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
                ChatEvent::Peer(peer) => {
                    warn!("peer received: {:?}", peer);
                }
            }
        }
    }

    pub async fn send_message(&self, message: IndexedMessage) -> anyhow::Result<()> {
        self.tx.send_async(ChatEvent::Message(message)).await?;
        Ok(())
    }
    
    pub async fn send_peer(&self, peer: Peer) -> anyhow::Result<()> {
        self.tx.send_async(ChatEvent::Peer(peer)).await?;
        Ok(())
    }
}
