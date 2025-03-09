use std::collections::HashMap;
use std::io::{self, Write};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chat::{ChatDelegate, ChatError, ChatManager, DnsRecord, Event, Message, Peer};
use log::info;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use uuid::uuid;

struct ChatClient {
    manager: Arc<ChatManager>,
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    messages: Arc<Mutex<Vec<Message>>>,
}

impl ChatClient {
    fn new(name: String, root_path: String, port: u16) -> Result<Self, ChatError> {
        let manager = Arc::new(ChatManager::new(name, root_path, port)?);
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let existing_peers = manager.get_peers()?;
        for peer in existing_peers {
            peers.lock().unwrap().insert(peer.id.clone(), peer);
        }
        let existing_messages = manager.get_all_messages()?;
        Ok(Self {
            manager,
            peers,
            messages: Arc::new(Mutex::new(existing_messages)),
        })
    }

    fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mdns = ServiceDaemon::new()?;

        let dns_record = self.manager.get_dns_record_map();
        let hostname = format!("peer-{}.local.", self.manager.get_name());
        let service_type = "_myapp._tcp.local.";
        let instance_name = format!("Chat-{}", self.manager.get_name());

        let service_info = ServiceInfo::new(
            service_type,
            &instance_name,
            &hostname,
            "0.0.0.0",
            self.get_port_from_dns_record(&dns_record)?,
            dns_record,
        )?
        .enable_addr_auto();
        mdns.register(service_info.clone())?;
        let refresh_mdns = mdns.clone();
        let refresh_service = service_info.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(20));
                if let Err(e) = refresh_mdns.register(refresh_service.clone()) {
                    eprintln!("Failed to refresh mDNS registration: {:?}", e);
                }
            }
        });

        let browse_handle = mdns.browse(service_type)?;
        let peers_clone = self.peers.clone();
        let manager_clone = self.manager.clone();

        thread::spawn(move || {
            while let Ok(event) = browse_handle.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let props = info.get_properties().clone();
                        let record = props.into_property_map_str();
                        match manager_clone.verify_hashmap_record(&record) {
                            Ok(dns_record) => {
                                if dns_record.pub_key != manager_clone.get_pub_key() {
                                    for addr in info.get_addresses() {
                                        info!("print address: {}, {}", dns_record.name, addr);
                                    }
                                    let peer_addr = info
                                        .get_addresses()
                                        .iter()
                                        .filter(|addr| match addr {
                                            IpAddr::V4(addr) => addr.is_link_local(),
                                            IpAddr::V6(_) => false,
                                        })
                                        .next()
                                        .map(|ip| format!("{}:{}", ip, info.get_port()))
                                        .unwrap_or_default();

                                    let peer_addr = peer_addr.split(':').next().unwrap();
                                    let peer_addr = format!("{}:{}", peer_addr, dns_record.port);
                                    info!("Found peer: {}, {}", dns_record.name, peer_addr);
                                    if !peer_addr.is_empty() {
                                        if let Err(e) = manager_clone.set_peer(
                                            dns_record.name.clone(),
                                            peer_addr,
                                            dns_record.pub_key.clone(),
                                        ) {
                                            eprintln!("Failed to set peer: {:?}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => eprintln!("Failed to verify record: {:?}", e),
                        }
                    }
                    _ => {}
                }
            }
        });

        let server_manager = self.manager.clone();
        thread::spawn(move || {
            server_manager.run_server();
        });

        let delegate = ChatClientDelegate {
            peers: self.peers.clone(),
            messages: self.messages.clone(),
            manager: self.manager.clone(),
        };

        self.manager.set_delegate(Arc::new(delegate));
        let loop_manager = self.manager.clone();
        thread::spawn(move || {
            loop_manager.run_loop();
        });
        self.console_loop()?;
        Ok(())
    }

    fn get_port_from_dns_record(
        &self,
        record: &HashMap<String, String>,
    ) -> Result<u16, Box<dyn std::error::Error>> {
        record
            .get("port")
            .ok_or("Port not found in DNS record")?
            .parse::<u16>()
            .map_err(|e| e.into())
            .map(|port| port)
    }

    fn console_loop(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("P2P Chat Console");
        println!("Type 'help' for available commands");
        loop {
            print!("> ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim();

            if input.is_empty() {
                continue;
            }

            match input {
                "help" => {
                    println!("Available commands:");
                    println!("  help         - Show this help");
                    println!("  peers        - List connected peers");
                    println!("  messages     - Show all messages");
                    println!("  send <text>  - Send a message");
                    println!("  file <path>  - Send a file");
                    println!("  exit         - Exit the application");
                }
                "peers" => {
                    let peers = self.peers.lock().unwrap();
                    println!("Connected peers:");
                    for (id, peer) in peers.iter() {
                        println!("  {} ({})", peer.name, id);
                    }
                }
                "messages" => {
                    let messages = self.messages.lock().unwrap();
                    println!("Messages:");
                    for msg in messages.iter() {
                        let peers = self.peers.lock().unwrap();
                        let sender_name = peers
                            .get(&msg.peer_id)
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| {
                                if msg.peer_id == self.manager.get_name() {
                                    "You".to_string()
                                } else {
                                    "Unknown".to_string()
                                }
                            });

                        if let Some(file_id) = &msg.file_id {
                            println!("  {} sent a file (ID: {})", sender_name, file_id);
                            if let Some(file_path) = &msg.file_path {
                                println!("    File saved at: {}", file_path);
                            } else {
                                println!("    File is being downloaded...");
                            }
                        } else {
                            println!("  {}: {}", sender_name, msg.text);
                        }
                    }
                }
                cmd if cmd.starts_with("send ") => {
                    let text = &cmd[5..];
                    if !text.is_empty() {
                        match self.manager.send_message(Some(text.to_string()), None) {
                            Ok(_) => println!("Message sent"),
                            Err(e) => println!("Failed to send message: {:?}", e),
                        }
                    } else {
                        println!("Message cannot be empty");
                    }
                }
                cmd if cmd.starts_with("file ") => {
                    let file_path = &cmd[5..];
                    if !file_path.is_empty() {
                        let file_id = uuid::Uuid::new_v4().to_string();
                        let format = file_path.split('.').last().unwrap_or("bin").to_string();

                        match self.manager.set_file_path(
                            file_id.clone(),
                            format,
                            file_path.to_string(),
                        ) {
                            Ok(_) => match self.manager.send_message(None, Some(file_id)) {
                                Ok(_) => println!("File message sent"),
                                Err(e) => println!("Failed to send file message: {:?}", e),
                            },
                            Err(e) => println!("Failed to prepare file: {:?}", e),
                        }
                    } else {
                        println!("File path cannot be empty");
                    }
                }
                "exit" => {
                    println!("Exiting...");
                    self.manager.stop_server();
                    break;
                }
                _ => println!("Unknown command. Type 'help' for available commands."),
            }
        }

        Ok(())
    }
}

struct ChatClientDelegate {
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    messages: Arc<Mutex<Vec<Message>>>,
    manager: Arc<ChatManager>,
}

impl ChatDelegate for ChatClientDelegate {
    fn on_event(&self, event: Event) {
        match event {
            Event::Peer(peer) => {
                let mut peers = self.peers.lock().unwrap();
                peers.insert(peer.id.clone(), peer);
            }
            Event::Message(message) => {
                let peers = self.peers.lock().unwrap();
                let sender_name = peers
                    .get(&message.peer_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| {
                        if message.peer_id == self.manager.get_name() {
                            "You".to_string()
                        } else {
                            "Unknown".to_string()
                        }
                    });

                if let Some(file_id) = &message.file_id {
                    if message.file_path.is_some() {
                        println!(
                            "\n{} sent a file (saved at: {})",
                            sender_name,
                            message.file_path.as_ref().unwrap()
                        );
                    } else {
                        println!("\n{} sent a file (downloading...)", sender_name);
                    }
                } else {
                    println!("\n{}: {}", sender_name, message.text);

                    if message.text.to_lowercase().contains("explain")
                        && message.peer_id != self.manager.get_name()
                    {
                        thread::sleep(Duration::from_millis(500));

                        match self
                            .manager
                            .send_message(Some("will do!".to_string()), None)
                        {
                            Ok(_) => {}
                            Err(e) => eprintln!("Failed to send response: {:?}", e),
                        }
                    }
                }

                print!("> ");
                io::stdout().flush().unwrap();

                let mut messages = self.messages.lock().unwrap();
                messages.push(message);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = if let Some(arg) = std::env::args().nth(1) {
        arg
    } else {
        format!("User-{}", uuid::Uuid::new_v4().to_string()[..8].to_string())
    };

    let root_path = if let Some(arg) = std::env::args().nth(2) {
        arg
    } else {
        "./chat_data".to_string()
    };

    let port = if let Some(arg) = std::env::args().nth(3) {
        arg.parse::<u16>().unwrap_or(8080)
    } else {
        8080
    };

    std::fs::create_dir_all(&root_path)?;

    let client = ChatClient::new(name.clone(), root_path, port)?;
    println!("Started chat client with name: {}", name);

    client.run()?;

    Ok(())
}
