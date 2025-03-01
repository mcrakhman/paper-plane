use anyhow::anyhow;
use futures::StreamExt;
use log::{debug, info, warn};
use std::sync::Arc;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{
        watch::{Receiver, Sender},
        Mutex,
    },
};
use tokio_yamux::{Session, StreamHandle};

pub struct Peer<T> {
    session: Arc<Mutex<Session<T>>>,
    pub peer_id: String,
    delegate: Arc<dyn PeerDelegate + Send + Sync>,
    tx: Sender<i32>,
    rx: Receiver<i32>,
    open_lock: Arc<Mutex<()>>,
    pub is_alive: Arc<Mutex<bool>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

pub trait PeerDelegate: 'static {
    fn handle_inbound_stream(
        self: Arc<Self>,
        stream: StreamHandle,
        peer_id: String,
    ) -> anyhow::Result<()>;
}

impl<T> Peer<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(
        session: Arc<Mutex<Session<T>>>,
        peer_id: String,
        delegate: Arc<dyn PeerDelegate + Send + Sync>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        let (tx, rx) = tokio::sync::watch::channel(0);
        let is_alive = Arc::new(Mutex::new(true));
        Peer {
            session,
            peer_id,
            delegate,
            tx,
            rx,
            open_lock: Arc::new(Mutex::new(())),
            is_alive,
            runtime,
        }
    }

    pub async fn is_alive(&self) -> bool {
        *self.is_alive.lock().await
    }

    pub async fn open_stream(self: Arc<Self>) -> anyhow::Result<StreamHandle> {
        debug!("opening stream");
        let _guard = self.open_lock.lock().await;
        self.tx.send(1)?;
        let mut sess = self.session.lock().await;
        let stream = sess.open_stream();
        if stream.is_err() {
            *self.is_alive.lock().await = false;
        }
        stream.map_err(|e| anyhow!("received error openeing stream: {:?}", e))
    }

    pub fn start_inbound_loop(self: Arc<Self>) {
        let self_clone = self.clone();
        let mut rx = self.rx.clone();
        debug!("starting inbound loop");
        self.runtime.spawn(async move {
            loop {
                let mut sess = self_clone.session.lock().await;
                tokio::select! {
                    Ok(_) = rx.changed() => {
                        rx.borrow_and_update();
                        drop(sess);
                        let _guard = self_clone.open_lock.lock().await;
                        continue;
                    }
                    res = async {
                        sess.next().await
                    } => {
                        match res {
                            Some(Ok(stream)) => {
                                debug!("got stream");
                                if let Err(res) = self_clone.delegate.clone().handle_inbound_stream(stream, self_clone.peer_id.clone()) {
                                    warn!("error handling stream: {:?}", res);
                                    continue;
                                }
                            },
                            Some(Err(e)) => {
                                debug!("error reading from session: {:?}", e);
                                break;
                            },
                            None => {
                                debug!("got None from session");
                                break;
                            },
                        };
                    }
                };
            };
            *self_clone.is_alive.lock().await = false;
            debug!("exiting inbound loop");
        });
    }
}
