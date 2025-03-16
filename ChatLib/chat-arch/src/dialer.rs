use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
};

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use log::info;
use tokio::sync::Mutex;
use tokio_yamux::{Config, Session};

use crate::{
    handshake::write_handshake,
    peer_pool::{self, EncryptedSession},
};

pub struct Dialer {
    signing_key: SigningKey,
    addrs: Arc<Mutex<HashMap<String, String>>>,
}

impl Dialer {
    pub fn new(signing_key: SigningKey) -> Self {
        Self {
            signing_key,
            addrs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get(&self, peer_id: &str) -> Option<String> {
        let peer_id = peer_id.to_string();
        self.addrs
            .lock()
            .await
            .get(&peer_id)
            .map(|entry| entry.clone())
    }
}

#[async_trait]
impl peer_pool::Dialer for Dialer {
    async fn dial(&self, peer_id: &str) -> anyhow::Result<EncryptedSession> {
        let addr = self.get(peer_id).await.ok_or(anyhow::anyhow!(
            "failed to find addr for peer_id {}",
            peer_id
        ))?;
        info!("dialing {}", addr);
        let sock_addr = addr.parse::<SocketAddr>()?;
        let mut socket = tokio::net::TcpStream::connect(sock_addr).await?;
        socket.peer_addr()?;
        info!("connected {:?}", &socket.peer_addr());
        let res = write_handshake(&mut socket, &self.signing_key).await?;
        let socket = crate::conn::EncryptedStream::new(socket, &res.symmetric_key);
        let session = std::sync::Arc::new(tokio::sync::Mutex::new(Session::new_client(
            socket,
            Config::default(),
        )));
        Ok(session)
    }

    async fn add(&self, peer_id: String, addr: String) {
        self.addrs.lock().await.insert(peer_id, addr);
    }

    async fn all_peers(&self) -> Vec<String> {
        self.addrs.lock().await.keys().cloned().collect()
    }
}
