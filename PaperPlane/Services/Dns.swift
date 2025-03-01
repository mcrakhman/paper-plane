import Foundation
import Network

struct ServiceDescription {
    let name: String
    let txtRecord: Data?
    let ip: String?
}

enum ResolveResult {
    case added(ServiceDescription)
    case changed(ServiceDescription)
    case removed(String)
}

enum ResolveAction {
    case add
    case change
    case remove
}

protocol PeerResolver {
    func didResolvePeer(result: ResolveResult)
}

final class BonjourResolver: NSObject, NetServiceDelegate {
    typealias CompletionHandler = (Result<ServiceDescription, Error>) -> Void
    @discardableResult
    static func resolve(
        service: NetService, completionHandler: @escaping CompletionHandler
    ) -> BonjourResolver {
        precondition(Thread.isMainThread)
        let resolver = BonjourResolver(
            service: service, completionHandler: completionHandler)
        resolver.start()
        return resolver
    }

    private init(
        service: NetService, completionHandler: @escaping CompletionHandler
    ) {
        let copy = NetService(
            domain: service.domain, type: service.type, name: service.name)
        self.service = copy
        self.completionHandler = completionHandler
    }

    deinit {
        assert(self.service == nil)
        assert(self.completionHandler == nil)
        assert(self.selfRetain == nil)
    }

    private var service: NetService? = nil
    private var completionHandler: (CompletionHandler)? = nil
    private var selfRetain: BonjourResolver? = nil

    private func start() {
        precondition(Thread.isMainThread)
        guard let service = self.service else { fatalError() }
        service.delegate = self
        service.resolve(withTimeout: 10.0)
        selfRetain = self
    }

    func stop() {
        self.stop(with: .failure(CocoaError(.userCancelled)))
    }

    private func stop(with result: Result<ServiceDescription, Error>) {
        precondition(Thread.isMainThread)
        self.service?.delegate = nil
        self.service?.stop()
        self.service = nil
        let completionHandler = self.completionHandler
        self.completionHandler = nil
        completionHandler?(result)

        selfRetain = nil
    }

    func netServiceDidResolveAddress(_ sender: NetService) {
        let description = ServiceDescription(
            name: sender.name, txtRecord: sender.txtRecordData(),
            ip: getIp(sender.addresses))
        self.stop(with: .success(description))
    }

    func netService(
        _ sender: NetService, didNotResolve errorDict: [String: NSNumber]
    ) {
        let code =
            (errorDict[NetService.errorCode]?.intValue)
            .flatMap { NetService.ErrorCode.init(rawValue: $0) }
            ?? .unknownError
        let error = NSError(
            domain: NetService.errorDomain, code: code.rawValue, userInfo: nil)
        self.stop(with: .failure(error))
    }
}

func getIp(_ addresses: [Data]?) -> String? {
    guard let addresses = addresses else {
        return ""
    }

    var ipAddresses: [String] = []
    for addressData in addresses {
        addressData.withUnsafeBytes { rawBufferPointer in
            guard
                let sockaddrPointer = rawBufferPointer.baseAddress?
                    .assumingMemoryBound(to: sockaddr.self)
            else {
                return
            }

            let family = sockaddrPointer.pointee.sa_family
            var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))

            if family == sa_family_t(AF_INET) {  // IPv4
                let sockaddrIn = sockaddrPointer.withMemoryRebound(
                    to: sockaddr_in.self, capacity: 1
                ) { $0 }
                var address = sockaddrIn.pointee.sin_addr
                inet_ntop(AF_INET, &address, &buffer, socklen_t(buffer.count))
            } else if family == sa_family_t(AF_INET6) {  // IPv6
                let sockaddrIn6 = sockaddrPointer.withMemoryRebound(
                    to: sockaddr_in6.self, capacity: 1
                ) { $0 }
                var address = sockaddrIn6.pointee.sin6_addr  // Create a mutable copy
                inet_ntop(AF_INET6, &address, &buffer, socklen_t(buffer.count))
            }

            if let ip = String(validatingUTF8: buffer) {
                ipAddresses.append(ip)
            }
        }
    }

    return ipAddresses.filter({ ip in ip.contains("192.168") }).first
}

class DnsBrowser {
    private var browser: NWBrowser?
    var resolver: PeerResolver?
    private let type: String
    private let name: String

    init(name: String, type: String) {
        self.type = type
        self.name = name
    }

    func start() {
        print("browser will start")
        let descriptor = NWBrowser.Descriptor.bonjourWithTXTRecord(
            type: self.type, domain: "local.")
        let browser = NWBrowser(for: descriptor, using: .tcp)
        browser.stateUpdateHandler = { newState in
            print("browser did change state, new: \(newState)")
        }
        browser.browseResultsChangedHandler = { updated, changes in
            print("browser results did change:")
            for change in changes {
                switch change {
                case .added(let result):
                    self.resolve(endpoint: result.endpoint, action: .add)
                    print("+ \(result.metadata) \(result.interfaces)")
                case .removed(let result):
                    self.resolve(endpoint: result.endpoint, action: .remove)
                    print("- \(result.endpoint)")
                case .changed(let old, let new, flags: _):
                    self.resolve(endpoint: new.endpoint, action: .change)
                case .identical:
                    fallthrough
                @unknown default:
                    print("?")
                }
            }
        }
        browser.start(queue: .main)
        self.browser = browser
    }

    func resolve(endpoint: NWEndpoint, action: ResolveAction) {
        if case .service(let name, let type, let domain, _) = endpoint {
            if name == self.name {
                return
            }
            if case .remove = action {
                self.resolver?.didResolvePeer(result: .removed(name))
                return
            }
            let service = NetService(domain: domain, type: type, name: name)
            print("will resolve, service: \(service)")
            BonjourResolver.resolve(service: service) { result in
                switch result {
                case .success(let description):
                    self.resolver?.didResolvePeer(
                        result: action == .add
                            ? .added(description) : .changed(description))
                    print("did resolve, host: \(description.name)")
                case .failure(let error):
                    print("did not resolve, error: \(error)")
                }
            }
        }
    }

    func stop() {
        browser?.stateUpdateHandler = nil
        browser?.cancel()
    }

    func startStop() {
        if let browser = self.browser {
            self.browser = nil
            self.stop()
        } else {
            self.start()
        }
    }
}

class DnsSender {
    private var listener: NWListener? = nil
    private let name: String
    private let type: String
    private let record: Data

    init(name: String, type: String, record: Data) {
        self.type = type
        self.name = name
        self.record = record
    }

    func start() {
        guard let listener = try? NWListener(using: .tcp) else { return }
        listener.stateUpdateHandler = { newState in
            print("listener did change state, new: \(newState)")
        }
        listener.newConnectionHandler = { connection in
            connection.cancel()
        }

        listener.service = .init(name: name, type: type, txtRecord: self.record)
        listener.serviceRegistrationUpdateHandler = { change in
            print("ch", change)
        }
        listener.start(queue: .main)
        self.listener = listener
    }

    func stop() {
        listener?.stateUpdateHandler = nil
        listener?.cancel()
    }

    func startStop() {
        if let listener = self.listener {
            self.listener = nil
            self.stop()
        } else {
            self.start()
        }
    }
}
