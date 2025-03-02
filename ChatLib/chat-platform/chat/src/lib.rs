use chat_arch::app_context::{self, AppContext};
use chat_arch::events::ChatEvent;
use chat_arch::peer_pool::Dialer;
use chat_arch::{file_database, models, peer_database};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use uniffi::deps::anyhow;
use uniffi::deps::log::info;

uniffi::setup_scaffolding!();

#[derive(uniffi::Record, Clone, Debug)]
pub struct Message {
    pub order: String,
    pub id: String,
    pub text: String,
    pub file_id: Option<String>,
    pub file_path: Option<String>,
    pub peer_id: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Peer {
    pub id: String,
    pub name: String,
}

impl From<chat_arch::peer_database::Peer> for Peer {
    fn from(peer: chat_arch::peer_database::Peer) -> Self {
        Peer {
            id: peer.id,
            name: peer.name.unwrap_or("Default".to_owned()),
        }
    }
}

#[derive(uniffi::Enum)]
pub enum Event {
    Message(Message),
}

#[derive(Debug, PartialEq, thiserror::Error, uniffi::Error)]
pub enum ChatError {
    #[error("Failed to create a TCP listener.")]
    FailedToCreateNew(String),
    #[error("Failed to decode a TXT record.")]
    FailedToDecodeTxtRecord,
    #[error("Failed to connect.")]
    FailedToConnect,
    #[error("Failed to send.")]
    FailedToSend,
    #[error("Failed to download.")]
    FailedToDownload(String),
}

impl ChatError {
    fn create_new_error<T: std::fmt::Display>(e: T) -> ChatError {
        ChatError::FailedToCreateNew(format!("{}", e))
    }

    fn failed_to_download<T: std::fmt::Display>(e: T) -> ChatError {
        ChatError::FailedToDownload(format!("{}", e))
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct DnsRecord {
    pub port: u16,
    pub name: String,
    pub pub_key: String,
}

#[derive(uniffi::Object)]
pub struct ChatManager {
    context: AppContext,
    runtime: Arc<Runtime>,
    signing_key: SigningKey,
    root_path: String,
    txt_record: Vec<u8>,
    delegate: Arc<Mutex<Option<Arc<dyn ChatDelegate>>>>,
}

#[uniffi::export(with_foreign)]
pub trait ChatDelegate: Send + Sync {
    fn on_event(&self, event: Event);
}

#[uniffi::export]
impl ChatManager {
    #[uniffi::constructor]
    pub fn new(name: String, root_path: String, port: u16) -> Result<Self, ChatError> {
        unsafe {
            env::set_var("RUST_LOG", "DEBUG");
        }
        env_logger::init();
        let runtime = tokio::runtime::Runtime::new().map_err(|e| ChatError::create_new_error(e))?;
        let runtime = Arc::new(runtime);
        let addr = format!("0.0.0.0:{}", port);
        let deps = runtime.block_on(async {
            app_context::prepare_deps(&name, &addr, &root_path, runtime.clone())
                .await
                .map_err(|e| ChatError::create_new_error(e))
        })?;
        let name = deps.peer.get_name();
        let mut map = HashMap::new();
        let key = deps.signing_key.clone();
        let signature = key.sign(name.as_bytes());
        map.insert("signature".to_string(), hex::encode(signature.to_bytes()));
        map.insert("port".to_string(), port.to_string());
        map.insert("name".to_string(), name);
        map.insert(
            "pub_key".to_string(),
            hex::encode(key.verifying_key().to_bytes()),
        );
        let txt_record = encode_txt_record(map).unwrap();
        let mgr = ChatManager {
            root_path,
            context: deps,
            runtime,
            signing_key: key,
            delegate: Arc::new(Mutex::new(None)),
            txt_record,
        };
        Ok(mgr)
    }

    pub fn set_delegate(&self, delegate: Arc<dyn ChatDelegate>) {
        let mut guard = self.delegate.lock().unwrap();
        *guard = Some(delegate);
    }

    pub fn run_server(&self) {
        let ctx = self.context.clone();
        self.runtime.block_on(async {
            match ctx.server.run().await {
                Ok(_) => {
                    info!("Server exited");
                }
                Err(e) => {
                    info!("Error: {:?}", e);
                }
            }
        });
    }

    pub fn run_loop(&self) {
        self.context.sync_engine.run();
        self.context.file_resolver.clone().run();
        let rx = self.context.events.get_rx();
        while let Ok(event) = rx.recv() {
            match event {
                ChatEvent::Message(msg) => {
                    let file_id = msg.file_id.clone();
                    let file_path = msg.file_path.clone();
                    let peer_id = msg.peer_id.clone();
                    let message = Message {
                        order: msg.order_id,
                        id: msg.id,
                        text: msg.text,
                        file_id: msg.file_id,
                        file_path: msg.file_path,
                        peer_id: msg.peer_id,
                    };
                    let event = Event::Message(message);
                    let guard = self.delegate.lock().unwrap();
                    if file_id.is_some() && file_path.is_none() {
                        self.resolve_file(file_id.unwrap(), Some(peer_id));
                    }
                    if let Some(delegate) = &*guard {
                        delegate.on_event(event);
                    }
                }
            }
        }
    }

    pub fn get_peers(&self) -> Result<Vec<Peer>, ChatError> {
        self.runtime
            .block_on(async { self.context.peer_db.get_all_peers().await })
            .map(|peers| peers.into_iter().map(|peer| peer.into()).collect())
            .map_err(|e| ChatError::create_new_error(e))
    }

    pub fn set_peer(&self, name: String, addr: String, pub_key: String) -> Result<(), ChatError> {
        self.runtime.block_on(async {
            let peer = match peer_database::Peer::new(pub_key.clone(), name, pub_key.clone()) {
                Ok(peer) => peer,
                Err(e) => return Err(ChatError::create_new_error(e)),
            };
            self.context
                .peer_db
                .save_peer(&peer)
                .await
                .map_err(|e| ChatError::create_new_error(e))?;
            self.context.dialer.add(pub_key.clone(), addr).await;
            Ok(())
        })
    }
    
    pub fn get_name(&self) -> String {
        self.context.peer.get_name()
    }

    pub fn get_all_messages(&self) -> Result<Vec<Message>, ChatError> {
        let ctx = self.context.clone();
        self.runtime
            .block_on(async {
                ctx.indexer.get_all_after_order_id("").await.map(|msgs| {
                    msgs.into_iter()
                        .map(|msg| Message {
                            order: msg.order_id,
                            id: msg.id,
                            text: msg.text,
                            file_id: msg.file_id,
                            file_path: msg.file_path,
                            peer_id: msg.peer_id,
                        })
                        .collect::<Vec<Message>>()
                })
            })
            .map_err(|e| ChatError::create_new_error(e))
    }

    pub fn resolve_file(&self, file_id: String, peer_id: Option<String>) -> Result<(), ChatError> {
        let ctx = self.context.clone();
        self.runtime.block_on(async {
            ctx.file_resolver.add_need_resolve(&file_id, peer_id).await;
        });
        Ok(())
    }

    pub fn get_file_path(&self, file_id: String) -> Result<String, ChatError> {
        self.runtime
            .block_on(async {
                self.context
                    .file_db
                    .get_by_id(&file_id)
                    .await
                    .map_err(|_| ChatError::FailedToDownload("Failed to get file path".to_string()))
            })
            .and_then(|file| {
                file.ok_or(ChatError::FailedToDownload(
                    "Failed to get file path".to_string(),
                ))
            })
            .map(|file| file.local_path)
    }

    pub fn set_file_path(
        &self,
        file_id: String,
        format: String,
        file_path: String,
    ) -> Result<(), ChatError> {
        self.runtime
            .block_on(async {
                let description = file_database::FileDescription {
                    id: file_id,
                    local_path: file_path,
                    format,
                    timestamp: chrono::Utc::now().timestamp(),
                };
                self.context.file_db.save(&description).await
            })
            .map_err(|e| ChatError::FailedToDownload(format!("failed to set file path {:?}", e)))
    }

    pub fn send_message(
        &self,
        message: Option<String>,
        file_id: Option<String>,
    ) -> Result<(), ChatError> {
        self.runtime
            .block_on(async {
                let manager = self.context.sync_engine.get_manager();
                let builder = models::MessageBuilder::new(
                    uuid::Uuid::new_v4().to_string(),
                    chrono::Utc::now().timestamp(),
                    self.context.peer.id.clone(),
                );
                let builder = if let Some(msg) = message {
                    builder.text(msg)
                } else if let Some(file_id) = file_id {
                    builder.file_id(file_id)
                } else {
                    return Err(anyhow::anyhow!("No message or filename"));
                };
                let message = builder.build();
                manager.add_own_message(message).await
            })
            .map(|_| ())
            .map_err(|_| ChatError::FailedToSend)
    }

    pub fn verify_record(&self, record: &[u8]) -> Result<DnsRecord, ChatError> {
        let record = decode_txt_record(record).unwrap();
        let signature = record
            .get("signature")
            .ok_or(ChatError::FailedToDecodeTxtRecord)?;
        let name = record
            .get("name")
            .ok_or(ChatError::FailedToDecodeTxtRecord)?;
        let port = record
            .get("port")
            .ok_or(ChatError::FailedToDecodeTxtRecord)?;
        let pub_key = record
            .get("pub_key")
            .ok_or(ChatError::FailedToDecodeTxtRecord)?;
        let signature_bytes = hex::decode(signature)
            .map_err(|_| ChatError::FailedToDecodeTxtRecord)?
            .try_into()
            .map_err(|_| ChatError::FailedToDecodeTxtRecord)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let pub_key_bytes = hex::decode(pub_key)
            .map_err(|_| ChatError::FailedToDecodeTxtRecord)?
            .try_into()
            .map_err(|_| ChatError::FailedToDecodeTxtRecord)?;
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_bytes)
            .map_err(|_| ChatError::FailedToDecodeTxtRecord)?;
        verifying_key
            .verify(name.as_bytes(), &signature)
            .map_err(|_| ChatError::FailedToDecodeTxtRecord)?;
        Ok(DnsRecord {
            port: port
                .parse()
                .map_err(|_| ChatError::FailedToDecodeTxtRecord)?,
            name: name.clone(),
            pub_key: pub_key.clone(),
        })
    }

    pub fn get_dns_record(&self) -> Vec<u8> {
        self.txt_record.clone()
    }
}

fn encode_txt_record(txt_record: HashMap<String, String>) -> Option<Vec<u8>> {
    let mut result = Vec::new();
    for (key, value) in txt_record {
        let entry = format!("{}={}", key, value);
        let entry_bytes = entry.into_bytes();
        if entry_bytes.len() > 255 {
            return None;
        }
        result.push(entry_bytes.len() as u8);
        result.extend(entry_bytes);
    }

    Some(result)
}

fn decode_txt_record(data: &[u8]) -> Option<HashMap<String, String>> {
    let mut result = HashMap::new();
    let mut i = 0;

    while i < data.len() {
        if i + 1 > data.len() {
            return None;
        }
        let length = data[i] as usize;
        i += 1;
        if i + length > data.len() {
            return None;
        }
        let entry_bytes = &data[i..i + length];
        i += length;
        if let Ok(entry) = String::from_utf8(entry_bytes.to_vec()) {
            if let Some((key, value)) = entry.split_once('=') {
                result.insert(key.to_string(), value.to_string());
            } else {
                return None;
            }
        } else {
            return None;
        }
    }
    Some(result)
}
