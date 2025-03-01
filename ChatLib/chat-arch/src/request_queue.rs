use anyhow::Result;
use log::{debug, warn};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    runtime::Runtime,
    time::{self, timeout},
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Task: Send + Sync + 'static {
    fn run(self: Arc<Self>) -> BoxFuture<'static, Result<()>>;
}

pub struct RequestQueue {
    sender: flume::Sender<Arc<dyn Task>>,
    receiver: Arc<flume::Receiver<Arc<dyn Task>>>,
    worker_count: usize,
    runtime: Arc<Runtime>,
}

impl RequestQueue {
    pub fn new(worker_count: usize, runtime: Arc<Runtime>) -> Self {
        let (tx, rx) = flume::unbounded();
        let queue = RequestQueue {
            sender: tx,
            receiver: Arc::new(rx),
            worker_count,
            runtime,
        };
        queue
    }

    pub fn start(&self) {
        for i in 0..self.worker_count {
            let worker_rx = self.receiver.clone();
            self.runtime.spawn(async move {
                worker_loop(worker_rx).await;
            });
            debug!("Spawned worker #{}", i);
        }
    }

    pub async fn enqueue(&self, req: Arc<dyn Task>) -> anyhow::Result<()> {
        self.sender.send_async(req).await.map_err(|e| e.into())
    }
}

async fn worker_loop(rx: Arc<flume::Receiver<Arc<dyn Task>>>) {
    while let Ok(request) = rx.as_ref().recv_async().await {
        let tm = timeout(Duration::from_secs(30), request.run());
        match tm.await {
            Ok(res) => {
                if let Err(e) = res {
                    warn!("Error processing request: {:?}", e);
                }
            }
            Err(_) => {
                warn!("Request timed out");
            }
        }
    }
    debug!("worker loop ending (channel closed).");
}

pub type AsyncFn =
    dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> + Send + Sync;

pub struct PeriodicTaskScheduler {
    interval_seconds: u64,
    task_fn: Arc<AsyncFn>,
    runtime: Arc<Runtime>,
}

impl PeriodicTaskScheduler {
    pub fn new(task_fn: Arc<AsyncFn>, interval_seconds: u64, runtime: Arc<Runtime>) -> Self {
        Self {
            interval_seconds,
            task_fn: Arc::clone(&task_fn),
            runtime,
        }
    }

    pub fn signal_start(&self) {
        let task_fn = Arc::clone(&self.task_fn);
        let seconds = self.interval_seconds;
        self.runtime.spawn(async move {
            PeriodicTaskScheduler::start(seconds, task_fn).await;
        });
    }

    async fn start(interval_seconds: u64, task_fn: Arc<AsyncFn>) {
        let mut interval = time::interval(Duration::from_secs(interval_seconds));
        loop {
            interval.tick().await;
            if let Err(e) = task_fn().await {
                warn!("Periodic task failed: {e}");
            }
        }
    }
}
