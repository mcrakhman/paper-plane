import UniFFI
import Foundation

public class ChatClient {
    private let chat: ChatManager
    
    public init(name: String, rootPath: String, port: UInt16) throws {
        chat = try ChatManager(name: name, rootPath: rootPath, port: port)
    }
    
    public func setDelegate(delegate: ChatDelegate) {
        chat.setDelegate(delegate: delegate)
    }
    
    public func setPeer(name: String, addr: String, peerId: String, pubKey: String) throws {
        try chat.setPeer(name: name, addr: addr, pubKey: pubKey)
    }
    
    public func resolveFile(fileId: String, peerId: String?) throws {
        try chat.resolveFile(fileId: fileId, peerId: peerId)
    }
    
    public func sendMessage(message: String?, file: String?) throws {
        try chat.sendMessage(message: message, fileId: file)
    }
    
    public func getName() -> String {
        chat.getName()
    }
    
    public func runLoop() {
        chat.runLoop()
    }
    
    public func runServer() {
        chat.runServer()
    }
    
    public func verifyRecord(data: Data) throws -> DnsRecord {
        try chat.verifyRecord(record: data)
    }
     
    public func getPeers() throws -> [UniFFI.Peer] {
        try chat.getPeers()
    }
    
    public func getFilePath(fileId: String) throws -> String {
        try chat.getFilePath(fileId: fileId)
    }
    
    public func setFilePath(fileId: String, format: String, filePath: String) throws {
        try chat.setFilePath(fileId: fileId, format: format, filePath: filePath)
    }
    
    public func getAllMessages() throws -> [UniFFI.Message] {
        try chat.getAllMessages()
    }
    
    public func getRecord() throws -> Data {
        chat.getDnsRecord()
    }
}

