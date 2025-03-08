use crate::{handshake::read_handshake, peer_pool::EncryptedPool};
use anyhow::Result;
use ed25519_dalek::SigningKey;
use log::{info, warn};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::{runtime::Runtime, select, sync::Mutex};
use tokio_yamux::{Config, Session};

pub struct Server {
    addr: String,
    signing_key: SigningKey,
    peer_pool: Arc<EncryptedPool>,
    runtime: Arc<Runtime>,
    stop_tx: Arc<watch::Sender<bool>>,
}

impl Server {
    pub fn new(
        addr: String,
        signing_key: SigningKey,
        peer_pool: Arc<EncryptedPool>,
        runtime: Arc<Runtime>,
    ) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Server {
            addr,
            peer_pool,
            signing_key,
            runtime,
            stop_tx: Arc::new(stop_tx),
        }
    }

    pub async fn run(&self) -> Result<()> {
        info!("Listening on: {}", &self.addr);
        let _ = self.stop_tx.send(false);
        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        let mut stop_rx = self.stop_tx.subscribe();
        loop {
            select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        info!("Stop signal received. Stopping server.");
                        return Ok(());
                    }
                }
                accept_result = listener.accept() => {
                    let (mut socket, _) = accept_result?;
                    let key = self.signing_key.clone();
                    let peer_pool = self.peer_pool.clone();
                    self.runtime.spawn(async move {
                        let res = read_handshake(&mut socket, &key).await;
                        if let Err(e) = res {
                            return;
                        }
                        let res = res.unwrap();
                        let addr = socket.peer_addr();
                        if addr.is_err() {
                            warn!("Failed to get peer address");
                            return;
                        }
                        let addr = addr.unwrap();
                        let socket = crate::conn::EncryptedStream::new(socket, &res.symmetric_key);
                        let session = Arc::new(Mutex::new(Session::new_server(socket, Config::default())));
                        if let Err(e) = peer_pool.insert(&res.hex_key(), addr, session).await {
                            warn!(
                                "Failed to open a session with {}, error {:?}",
                                &res.hex_key(),
                                e
                            );
                        }
                    });
                }
            }
        }
    }

    pub fn stop(&self) {
        info!("Stopping server.");
        let _ = self.stop_tx.send(true);
    }
}
