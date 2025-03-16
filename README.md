# PaperPlane - Local-First CRDT Chat Application

Hey there! 👋 This is a simple local first chat app based on CRDTs which lets you share messages and files with people on your local network. Basically this is a demo for the video I hopefully release soon :-)

![PaperPlane Demo](assets/Demo.gif)

## In a nutshell

- **Local-First Architecture**: Works within your local network without requiring internet connectivity
- **Local Network Peer-to-Peer Communication**: Direct communication between devices in local network
- **CRDT Synchronization**: Ensures consistency across all connected devices
- **File Sharing**: Send and receive files between peers
- **Cross-Platform at its core**: Core library in Rust with Swift bindings for iOS

### ChatLib - Core Library

The main library containing all the chat functionality:

- **chat-arch**: The core Rust package implementing CRDT logic, networking, and data management
    - Handles connections, message synchronization, file transfers, and peer discovery
    - Implements the core CRDT algorithms for conflict resolution
    - Manages file and message databases

- **chat-platform**: Cross-platform library that end-user applications can use
    - Provides a unified API for different platforms
    - Contains UniFFI bindings for Swift integration

- **apple**: Swift package for iOS integration
    - Provides Swift wrappers around the Rust core
    - Makes the Rust library accessible from Swift code

### PaperPlane - iOS Application

The iOS application demonstrating the chat capabilities:

- Simple UI for sending/receiving messages and files
- Manages connections to peers on the local network
- Displays pictures and raw messages

## Technology Stack

- **Rust**: Core chat library implementation
- **Swift/UIKit**: iOS application
- **Protocol Buffers**: For serialized data exchange
- **UniFFI**: For Rust-to-Swift interoperability (based on [uniffi-starter](https://github.com/ianthetechie/uniffi-starter))
- **CRDTs**: For conflict-free data synchronization

## Getting Started

### Prerequisites

- Rust toolchain (see `rust-toolchain.toml` for version)
- Xcode (for iOS development)
- Protobuf compiler

### Building the Library

1. Build the arm64 lib and generate Swift bindings:
```bash
cd ChatLib/chat-platform
./build-ios.sh
```

2. Run Xcode (change credentials to whatever Apple account you have)

## Some implementation details

### CRDT

I built a very simple CRDT system where each participant (peer) keeps their own growing list of messages, tagged with their unique ID and a counter that increases with each new message. To ensure everyone gets the same messages, peers share summaries of what they have by exchanging IDs and counters, then only request the specific messages they're missing. To maintain a consistent ordering that respects when people saw each other's messages, each peer also tracks a global counter that increases whenever they receive a message, taking the highest value between their counter and the sender's counter. This creates a logical timestamp that captures the intent that "my message comes after messages I've seen," ensuring a consistent ordering across all peers.

### Networking

This probably the main thing I wanted to try with this project is to build some networking primitives from scratch in Rust.

- Custom TLS-like protocol for secure communication
- Custom Application level protocol for message and file transfer (based on Protobufs)

This is all based on existing libraries:
- Peer-to-peer connections with multiplexing using Yamux
- Local network discovery to find other instances using MDNS (on iOS side)

### UniFFI Integration

This project uses UniFFI for the Rust-to-Swift connection, which I adapted from [uniffi-starter](https://github.com/ianthetechie/uniffi-starter). The original repo works out of the box - I just tweaked it a bit to fit my folder structure.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

