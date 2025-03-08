import ChatLib
import MessageKit
import Foundation
import UniFFI
import os

extension UserInfo: Identifiable {
    var id: String { pubKey }
}

extension Peer: @retroactive SenderType {
    public var senderId: String {
        return id
    }
    
    public var displayName: String {
        return name
    }
}

struct UserInfo: SenderType {
    var senderId: String {
        return pubKey
    }
    
    var displayName: String {
        return name
    }
    
    let address: String
    let port: UInt16
    let name: String
    let pubKey: String
}

func generateRandomName() -> String {
    let wordDictionary = [
        "Swift", "Mountain", "Red", "Ocean", "Golden",
        "Forest", "Sky", "Dark", "Bright", "River",
        "Stone", "Iron", "Silver", "Wind", "Night",
        "Sun", "Moon", "Star", "Fire", "Water"
    ]
    guard wordDictionary.count >= 2 else {
        return ""
    }
    let shuffledWords = wordDictionary.shuffled()
    return "\(shuffledWords[0])\(shuffledWords[1])"
}

extension ChatManager: PeerResolver {
    func didResolvePeer(result: ResolveResult) {
        switch result {
        case .added(let peer), .changed(let peer):
            if let rec = peer.txtRecord,
               let ip = peer.ip,
               let verify = try? client.verifyRecord(data: rec)
            {
                peers[peer.name] = UserInfo(
                    address: ip, port: verify.port, name: peer.name,
                    pubKey: verify.pubKey)
                print("Updating \(String(describing: peers[peer.name]))")
                let addr = "\(ip):\(verify.port)"
                try? client.setPeer(name: peer.name, addr: addr, peerId: verify.pubKey, pubKey: verify.pubKey)
            }
        case .removed(let name):
            peers[name] = nil
        }
    }
}

extension ChatManager: ChatDelegate {
    func onEvent(event: UniFFI.Event) {
        DispatchQueue.main.async {
            switch event {
            case .message(let msg):
                self.chatDelegate?.newMessage(msg)
            case .peer(let peer):
                self.chatDelegate?.updatePeers((try? self.client.getPeers()) ?? [])
            }
        }
    }
}

protocol ChatFeedDelegate {
    func newMessage(_ msg: UniFFI.Message)
    func updatePeers(_ peers: [Peer])
}

class ChatManager {
    private var client: ChatClient
    private var peers: [String: UserInfo] = [:]
    private var messages: [String: [String]] = [:]
    var chatDelegate: ChatFeedDelegate?
    private let browser: DnsBrowser
    private let sender: DnsSender
    let myUserInfo: UserInfo
    let myPeer: Peer
    var isRunning = false

    init(name: String) throws {
        let documentsDirectory = FileManager.default.urls(
            for: .documentDirectory,
            in: .userDomainMask
        ).first!
        var cl: ChatClient?
        DispatchQueue.global(qos: .background).sync {
            cl = try! ChatClient(name: name, rootPath: documentsDirectory.path, port: 6364)
        }
        let name = cl?.getName() ?? "Anonymous"
        self.client = cl!
        let rec = try client.getRecord()
        let dnsRec = try client.verifyRecord(data: rec)
        myUserInfo = UserInfo(address: "", port: dnsRec.port, name: name, pubKey: dnsRec.pubKey)
        peers = [myUserInfo.pubKey: myUserInfo]
        myPeer = Peer(id: dnsRec.pubKey, name: name)
        self.sender = DnsSender(name: name, type: "_myapp._tcp", record: rec)
        self.browser = DnsBrowser(name: name, type: "_myapp._tcp")
        self.browser.resolver = self
    }
    
    public func sendMessage(message: String?, file: String?) {
        DispatchQueue.global(qos: .userInteractive).async {
            try? self.client.sendMessage(message: message, file: file)
        }
    }
    
    public func getFilePath(fileId: String) -> String? {
        try? client.getFilePath(fileId: fileId)
    }
    
    public func setFilePath(fileId: String, format: String, path: String) throws {
        try client.setFilePath(fileId: fileId, format: format, filePath: path)
    }
    
    public func resolveFiles(fileIds: [String]) {
        for fileId in fileIds {
            try! client.resolveFile(fileId: fileId, peerId: nil)
        }
    }
    
    public func getAllMessages() -> [UniFFI.Message] {
        return (try? client.getAllMessages()) ?? []
    }
    
    public func getAllPeers() -> [UniFFI.Peer] {
        return (try? client.getPeers()) ?? []
    }

    func prepare() {
        self.client.setDelegate(delegate: self)
        Thread(block: {
            self.client.runLoop()
        }).start()
    }

    func start() {
        guard !isRunning else {
            return
        }
        isRunning = true
        self.sender.startStop()
        self.browser.startStop()
        Thread(block: {
            self.client.runServer()
        }).start()
    }

    func stop() {
        guard isRunning else {
            return
        }
        isRunning = false
        self.sender.startStop()
        self.browser.startStop()
        self.client.stopServer()
    }
}
