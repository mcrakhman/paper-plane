use crate::{conn::EncryptedStream, peer::Peer, peer::PeerDelegate};
use async_trait::async_trait;
use log::info;
use std::{
    collections::HashMap, net::SocketAddr, sync::{Arc, Weak}, time::Duration
};
use tokio::{runtime::Runtime, sync::Mutex, time::timeout};
use tokio_yamux::Session;

pub type EncryptedSession = Arc<Mutex<Session<EncryptedStream<tokio::net::TcpStream>>>>;

#[async_trait]
pub trait Dialer: Send + Sync {
    async fn dial(&self, peer_id: &str) -> anyhow::Result<EncryptedSession>;
    async fn add(&self, peer_id: String, addr: String);
    async fn all_peers(&self) -> Vec<String>;
}

pub type EncryptedPool = PeerPool;
pub type EncryptedPeer = Peer<EncryptedStream<tokio::net::TcpStream>>;

#[derive(Clone)]
pub struct PeerPool {
    outgoing: Arc<Mutex<HashMap<String, Arc<EncryptedPeer>>>>,
    incoming: Arc<Mutex<HashMap<String, Arc<EncryptedPeer>>>>,
    delegate: Weak<dyn PeerDelegate + Send + Sync>,
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    dialer: Arc<dyn Dialer>,
    runtime: Arc<Runtime>,
}

impl PeerPool {
    pub fn new(
        dialer: Arc<dyn Dialer>,
        delegate: Weak<dyn PeerDelegate + Send + Sync>,
        runtime: Arc<Runtime>,
    ) -> Self {
        Self {
            outgoing: Arc::new(Mutex::new(HashMap::new())),
            incoming: Arc::new(Mutex::new(HashMap::new())),
            locks: Arc::new(Mutex::new(HashMap::new())),
            delegate,
            dialer,
            runtime,
        }
    }
    
    pub async fn all_peers(&self) -> Vec<String> {
        self.dialer.all_peers().await
    }

    pub async fn current_peers(&self) -> Vec<String> {
        let mut peers = Vec::new();
        let guard = self.outgoing.lock().await;
        for (peer_id, peer) in guard.iter() {
            if peer.is_alive().await {
                peers.push(peer_id.clone());
            }
        }
        drop(guard);
        let guard = self.incoming.lock().await;
        for (peer_id, peer) in guard.iter() {
            if peer.is_alive().await {
                if !peers.contains(&peer_id) {
                    peers.push(peer_id.clone());
                }
            }
        }
        peers
    }

    pub async fn insert(&self, peer_id: &str, addr: SocketAddr, session: EncryptedSession) -> anyhow::Result<()> {
        let delegate = self
            .delegate
            .upgrade()
            .ok_or(anyhow::anyhow!("No delegate"))?;
        let peer = Arc::new(Peer::new(
            session.clone(),
            peer_id.to_owned(),
            delegate,
            self.runtime.clone(),
        ));
        peer.clone().start_inbound_loop();
        self.dialer.add(peer_id.to_owned(), addr.to_string()).await;
        self.incoming.lock().await.insert(peer_id.to_owned(), peer);
        Ok(())
    }

    pub async fn get(&self, peer_id: &str) -> anyhow::Result<Arc<EncryptedPeer>> {
        let peer_id = peer_id.to_string();
        let mut guard = self.locks.lock().await;
        let lock_entry = guard
            .entry(peer_id.clone())
            .or_insert(Arc::new(Mutex::new(())))
            .clone();
        drop(guard);
        let _guard = lock_entry.lock().await;
        {
            let mut guard = self.outgoing.lock().await;
            if let Some(existing) = guard.get(&peer_id) {
                let clone = existing.clone();
                if clone.is_alive().await {
                    return Ok(clone);
                } else {
                    info!("removing dead peer from outgoing {}", &peer_id);
                    guard.remove(&peer_id);
                }
            }
        }
        {
            let mut guard = self.incoming.lock().await;
            if let Some(existing) = guard.get(&peer_id) {
                let clone = existing.clone();
                if clone.is_alive().await {
                    return Ok(clone);
                } else {
                    info!("removing dead peer from incoming {}", &peer_id);
                    guard.remove(&peer_id);
                }
            }
        }
        info!("dialing {}", &peer_id);
        let timeout_duration = Duration::from_secs(10);
        
        let session = timeout(timeout_duration, self.dialer.dial(&peer_id)).await??;
        let delegate = self
            .delegate
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Delegate is gone"))?;
        let peer = Arc::new(Peer::new(
            session,
            peer_id.to_owned(),
            delegate,
            self.runtime.clone(),
        ));
        self.outgoing
            .lock()
            .await
            .insert(peer_id.to_string(), peer.clone());
        peer.clone().start_inbound_loop();
        Ok(peer)
    }
}
