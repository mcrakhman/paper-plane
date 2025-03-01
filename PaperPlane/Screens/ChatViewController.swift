import UIKit
import UniFFI
import MessageKit
import InputBarAccessoryView
import BTree

struct ImageMediaItem: MediaItem {
    var url: URL?
    var image: UIImage?
    var placeholderImage: UIImage
    var size: CGSize
    var id: String?
    
    init(image: UIImage?, id: String) {
        self.image = image
        self.id = id
        print("setting image for id", id, image == nil)
        size = CGSize(width: 240, height: 240)
        placeholderImage = UIImage(imageLiteralResourceName: "image_message_placeholder")
    }
    
    init(imageURL: URL) {
        url = imageURL
        size = CGSize(width: 240, height: 240)
        placeholderImage = UIImage(imageLiteralResourceName: "image_message_placeholder")
    }
    
    init() {
        size = CGSize(width: 240, height: 240)
        placeholderImage = UIImage(imageLiteralResourceName: "image_message_placeholder")
    }
}

struct Message: MessageType {
    let sender: SenderType
    let messageId: String
    let sentDate: Date
    let kind: MessageKind
    let orderId: String
}

extension Message {
    init?(msg: UniFFI.Message, documentsDirectory: URL, currentUser: Peer) {
        let kind: MessageKind
        if msg.text != "" {
            kind = .text(msg.text)
        } else if msg.fileId != nil {
            if let filePath = msg.filePath {
                let full = documentsDirectory.appending(path: filePath)
                print("filepath is", full, msg.peerId)
                let image = UIImage(contentsOfFile: full.path)
                kind = .photo(ImageMediaItem(image: image, id: msg.fileId ?? ""))
            } else {
                kind = .photo(ImageMediaItem())
            }
        } else {
            return nil
        }
        self.init(sender: currentUser, messageId: msg.id, sentDate: Date(), kind: kind, orderId: msg.order)
    }
}

extension Message: Equatable, Hashable {
    static func == (lhs: Message, rhs: Message) -> Bool {
        return lhs.messageId == rhs.messageId
    }
    
    func hash(into hasher: inout Hasher) {
        hasher.combine(messageId)
    }
}

extension Message: Comparable {
    static func < (lhs: Message, rhs: Message) -> Bool {
        return lhs.orderId < rhs.orderId
    }
}

extension ChatViewController: ChatFeedDelegate {
    func updatePeers(_ peers: [Peer]) {
        for peer in peers {
            self.peers[peer.id] = peer
        }
    }
    
    func newMessage(_ msg: UniFFI.Message) {
        if let message = Message(msg: msg, documentsDirectory: getDocumentsDirectory(), currentUser: self.peers[msg.peerId] ?? self.currentUser) {
            DispatchQueue.main.async {
                let shouldScroll: Bool
                if self.messages.contains(message) {
                    self.messages.remove(message)
                    self.messages.insert(message)
                    shouldScroll = false
                } else {
                    self.messages.insert(message)
                    shouldScroll = self.messages.last == message
                }
                self.messagesCollectionView.reloadData()
                if shouldScroll {
                    self.messagesCollectionView.scrollToLastItem()
                }
            }
        }
    }
}

class ChatViewController: MessagesViewController {
    
    private let currentUser: Peer
    private let chatManager: ChatManager
    private var peers: [String: Peer] = [:]
    private let dir: URL
    var messages: SortedSet<Message> = SortedSet()
    
    init(currentUser: Peer, peers: [Peer], chatManager: ChatManager) {
        self.currentUser = currentUser
        self.chatManager = chatManager
        self.dir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        super.init(nibName: nil, bundle: nil)
        for peer in peers {
            self.peers[peer.id] = peer
        }
        var fileIds: [String] = []
        for message in chatManager.getAllMessages() {
            if let fileId = message.fileId, message.filePath == nil {
                fileIds.append(fileId)
            }
            let peer = self.peers[message.peerId] ?? currentUser
            if let msg = Message(msg: message, documentsDirectory: getDocumentsDirectory(), currentUser: peer) {
                messages.insert(msg)
            }
        }
        chatManager.resolveFiles(fileIds: fileIds)
        chatManager.chatDelegate = self
    }
    
    required init?(coder: NSCoder) {
        fatalError("Use init instead")
    }
    
    override func viewDidLoad() {
        super.viewDidLoad()
        setupChatInterface()
        title = currentUser.displayName
        messagesCollectionView.reloadData()
    }
    
    private func setupChatInterface() {
        messagesCollectionView.messagesDataSource = self
        messagesCollectionView.messagesLayoutDelegate = self
        messagesCollectionView.messagesDisplayDelegate = self
        messageInputBar.delegate = self
        
        configureMessageInputBar()
        configureMessageCollectionView()
    }
    
    private func configureMessageInputBar() {
        messageInputBar.inputTextView.placeholder = "Type a message..."
        messageInputBar.sendButton.setTitle("Send", for: .normal)
        messageInputBar.sendButton.tintColor = .systemBlue
        
        let cameraButton = InputBarButtonItem(type: .system)
        cameraButton.image = UIImage(systemName: "camera")
        cameraButton.setSize(CGSize(width: 36, height: 36), animated: false)
        cameraButton.tintColor = .systemBlue
        
        cameraButton.onTouchUpInside { [weak self] _ in
            self?.presentImageSourceOptions()
        }
        
        messageInputBar.leftStackView.alignment = .center
        messageInputBar.setLeftStackViewWidthConstant(to: 36, animated: false)
        messageInputBar.leftStackView.addArrangedSubview(cameraButton)
    }
    
    private func presentImageSourceOptions() {
        let alert = UIAlertController(title: "Choose Image Source", message: nil, preferredStyle: .actionSheet)
        
        let cameraAction = UIAlertAction(title: "Take from Camera", style: .default) { [weak self] _ in
            self?.presentImagePicker(sourceType: .camera)
        }
        
        let photoLibraryAction = UIAlertAction(title: "Choose from Photos", style: .default) { [weak self] _ in
            self?.presentImagePicker(sourceType: .photoLibrary)
        }
        
        let cancelAction = UIAlertAction(title: "Cancel", style: .cancel, handler: nil)
        
        alert.addAction(cameraAction)
        alert.addAction(photoLibraryAction)
        alert.addAction(cancelAction)
        
        present(alert, animated: true, completion: nil)
    }
    
    private func presentImagePicker(sourceType: UIImagePickerController.SourceType) {
        guard UIImagePickerController.isSourceTypeAvailable(sourceType) else {
            let alert = UIAlertController(title: "Source Not Available",
                                          message: "This source is not available on your device.",
                                          preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "Ok", style: .default, handler: nil))
            present(alert, animated: true, completion: nil)
            return
        }
        
        let picker = UIImagePickerController()
        picker.delegate = self
        picker.sourceType = sourceType
        picker.allowsEditing = true
        present(picker, animated: true, completion: nil)
    }
    
    private func saveImageToDocuments(image: UIImage) -> (String, String)? {
        guard let data = image.jpegData(compressionQuality: 0.8) else {
            print("Failed to convert image to data.")
            return nil
        }
        
        let fileId = UUID().uuidString
        let fileName = "image_\(fileId).jpg"
        let documentsDirectory = getDocumentsDirectory()
        let fileURL = documentsDirectory.appendingPathComponent(fileName)
        
        guard let _ = try? data.write(to: fileURL) else {
            return nil
        }
        let documentsPath = documentsDirectory.path
        var relativePath = fileURL.path
        if relativePath.hasPrefix(documentsPath) {
            relativePath = String(relativePath.dropFirst(documentsPath.count))
        }
        if relativePath.hasPrefix("/") {
            relativePath.removeFirst()
        }
        return (fileId, relativePath)
    }
    
    private func getDocumentsDirectory() -> URL {
        self.dir
    }
    
    private func configureMessageCollectionView() {
        scrollsToLastItemOnKeyboardBeginsEditing = true
        maintainPositionOnInputBarHeightChanged = true
        
        if let layout = messagesCollectionView.collectionViewLayout as? MessagesCollectionViewFlowLayout {
            layout.textMessageSizeCalculator.outgoingAvatarSize = .zero
            layout.textMessageSizeCalculator.incomingAvatarSize = CGSize(width: 30, height: 30)
        }
    }
    
    private func configureInputBarPadding() {
        messageInputBar.padding.bottom = 8
        messageInputBar.middleContentViewPadding.right = -38
        messageInputBar.inputTextView.textContainerInset.bottom = 8
    }
    
}

extension ChatViewController: MessagesDataSource {
    var currentSender: any MessageKit.SenderType {
        return currentUser
    }
    
    func messageForItem(at indexPath: IndexPath, in messagesCollectionView: MessagesCollectionView) -> MessageType {
        return messages[indexPath.section]
    }
    
    func numberOfSections(in messagesCollectionView: MessagesCollectionView) -> Int {
        return messages.count
    }
}

extension ChatViewController: MessagesDisplayDelegate {
    func backgroundColor(for message: MessageType, at indexPath: IndexPath, in messagesCollectionView: MessagesCollectionView) -> UIColor {
        return isFromCurrentSender(message: message) ? .systemBlue : .systemGray5
    }
    
    func textColor(for message: MessageType, at indexPath: IndexPath, in messagesCollectionView: MessagesCollectionView) -> UIColor {
        return isFromCurrentSender(message: message) ? .white : .darkText
    }
    
    func configureAvatarView(_ avatarView: AvatarView, for message: MessageType, at indexPath: IndexPath, in messagesCollectionView: MessagesCollectionView) {
        let avatar = Avatar(initials: String(message.sender.displayName.first ?? "?"))
        avatarView.set(avatar: avatar)
    }
}

extension ChatViewController: MessagesLayoutDelegate {
    func footerViewSize(for section: Int, in messagesCollectionView: MessagesCollectionView) -> CGSize {
        return CGSize(width: 0, height: 8)
    }
}

extension ChatViewController: InputBarAccessoryViewDelegate {
    func inputBar(_ inputBar: InputBarAccessoryView, didPressSendButtonWith text: String) {
        chatManager.sendMessage(message: text, file: nil)
        inputBar.inputTextView.text = ""
    }
}

extension ChatViewController: UIImagePickerControllerDelegate, UINavigationControllerDelegate {
    
    func imagePickerController(_ picker: UIImagePickerController,
                               didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
        let selectedImage: UIImage? = {
            if let edited = info[.editedImage] as? UIImage {
                return edited
            } else if let original = info[.originalImage] as? UIImage {
                return original
            }
            return nil
        }()
        
        if let image = selectedImage {
            processImage(image: image)
        }
        picker.dismiss(animated: true, completion: nil)
    }
    
    private func processImage(image: UIImage) {
        guard let (fileId, filePath) = saveImageToDocuments(image: image) else {
            print("failed to save image")
            return
        }
        guard let _ = try? chatManager.setFilePath(fileId: fileId, format: "jpg", path: filePath) else {
            print("failed to set file path")
            return
        }
        chatManager.sendMessage(message: nil, file: fileId)
    }
    
    func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
        picker.dismiss(animated: true, completion: nil)
    }
}
