use std::sync::Arc;

use chat_arch::app_context::AppContext;
use chat_arch::peer_pool::Dialer;
use chat_arch::{file_database, models};
use log::{info, warn};
use tokio::io::{self, AsyncBufReadExt, BufReader};

fn main() {
    env_logger::init();
    if std::env::args().nth(1) == Some("server".to_string()) {
        info!("Starting server ......");
        run_server("Alice", "127.0.0.1:6262", "server");
    } else {
        info!("Starting client ......");
        run_server("Bob", "127.0.0.1:6363", "client");
    }
}

fn run_server(name: &str, addr: &str, folder: &str) {
    let rt = Arc::new(tokio::runtime::Runtime::new().unwrap());
    rt.clone().block_on(async move {
        if let Err(e) = server(name, addr, folder, rt).await {
            warn!("Error: {:?}", e);
        }
    });
}

async fn server(
    name: &str,
    addr: &str,
    folder: &str,
    rt: Arc<tokio::runtime::Runtime>,
) -> anyhow::Result<()> {
    let deps = chat_arch::app_context::prepare_deps(name, addr, folder, rt.clone()).await?;
    println!("My peer id is {}", &deps.peer.id);
    let cloned_deps = deps.clone();
    let event_deps = deps.clone();
    let events_handle = rt.spawn(async move {
        event_deps.events.start_loop().await;
    });
    let server_handle = rt.spawn(async move {
        match cloned_deps.server.run().await {
            Ok(_) => {
                info!("Server exited");
            }
            Err(e) => {
                warn!("Server start error: {:?}", e);
            }
        }
    });
    deps.sync_engine.run();
    deps.file_resolver.clone().run();
    read_loop(deps).await;
    events_handle.await?;
    server_handle.await?;
    Ok(())
}

async fn read_loop(deps: AppContext) {
    let mut stdin = BufReader::new(io::stdin());
    let mut input = String::new();
    println!("Enter text (type 'exit' to quit):");
    loop {
        input.clear();
        stdin
            .read_line(&mut input)
            .await
            .expect("Failed to read line");

        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("exit") {
            println!("Exiting...");
            break;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        match parts[0] {
            "write" => {
                if parts.len() < 2 {
                    println!("write command requires a message");
                    continue;
                }
                let manager = deps.sync_engine.get_manager();
                let msg = models::MessageBuilder::new(
                    uuid::Uuid::new_v4().to_string(),
                    chrono::Utc::now().timestamp(),
                    deps.peer.id.clone(),
                )
                .text(parts[1..].join(" "))
                .build();
                if let Err(e) = manager.add_own_message(msg).await {
                    println!("Failed to add message: {:?}", e);
                } else {
                    println!("Message added");
                }
            }
            "file_save" => {
                if parts.len() != 3 {
                    println!("file_save command requires a filename");
                    continue;
                }
                let file_id = parts[1];
                let path = parts[2];
                let description = file_database::FileDescription {
                    id: file_id.to_owned(),
                    local_path: path.to_owned(),
                    format: "txt".to_owned(),
                    timestamp: chrono::Utc::now().timestamp(),
                };
                if let Err(e) = deps.file_db.save(&description).await {
                    println!("Failed to save file: {:?}", e);
                } else {
                    println!("File saved");
                }
            }
            "file_resolve" => {
                if parts.len() < 2 {
                    println!("file_resolve command requires a filename");
                    continue;
                }
                let file_id = parts[1];
                if parts.len() == 3 {
                    let peer_id = parts[2];
                    deps.file_resolver
                        .add_need_resolve(file_id, Some(peer_id.to_owned()))
                        .await;
                } else {
                    deps.file_resolver.add_need_resolve(file_id, None).await;
                }
            }
            "file_msg" => {
                if parts.len() != 2 {
                    println!("file command requires a filename");
                    continue;
                }
                let manager = deps.sync_engine.get_manager();
                let msg = models::MessageBuilder::new(
                    uuid::Uuid::new_v4().to_string(),
                    chrono::Utc::now().timestamp(),
                    deps.peer.id.clone(),
                )
                .file_id(parts[1].to_owned())
                .build();
                if let Err(e) = manager.add_own_message(msg).await {
                    println!("Failed to add message: {:?}", e);
                } else {
                    println!("Message added");
                }
            }
            "dial_add" => {
                if parts.len() != 3 {
                    println!("dial command requires a peer id and address");
                    continue;
                }
                let dialer = deps.dialer.clone();
                dialer.add(parts[1].to_owned(), parts[2].to_owned()).await;
            }
            "read_all" => {
                if parts.len() != 1 {
                    println!("read_all command should be empty");
                    continue;
                }
                for msg in deps.indexer.get_all_after_order_id("").await.unwrap() {
                    println!("{}: {}", msg.order_id, msg.text);
                }
            }
            _ => {
                println!("Unknown command: {}", parts[0]);
            }
        }
    }
}
