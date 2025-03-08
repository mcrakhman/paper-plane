import UIKit
import os

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    var chatManager: ChatManager?
    
    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options connectionOptions: UIScene.ConnectionOptions) {
        guard let windowScene = (scene as? UIWindowScene) else { return }
        
        window = UIWindow(windowScene: windowScene)
        let chatManager = try! ChatManager(name: generateRandomName())
        let root = ChatViewController(currentUser: chatManager.myPeer, peers: chatManager.getAllPeers(), chatManager: chatManager)
        let navController = UINavigationController(rootViewController: root)
        
        let appearance = UINavigationBarAppearance()
        appearance.configureWithOpaqueBackground()
        appearance.backgroundColor = .systemBackground
        appearance.titleTextAttributes = [.foregroundColor: UIColor.label]
        appearance.largeTitleTextAttributes = [.foregroundColor: UIColor.label]
        
        navController.navigationBar.standardAppearance = appearance
        navController.navigationBar.scrollEdgeAppearance = appearance
        navController.navigationBar.compactAppearance = appearance
        
        navController.navigationBar.prefersLargeTitles = true
        navController.navigationBar.tintColor = .systemBlue
        
        window?.rootViewController = navController
        window?.makeKeyAndVisible()
        chatManager.prepare()
        chatManager.start()
        self.chatManager = chatManager
    }

    func sceneWillResignActive(_ scene: UIScene) {
        os_log("will resign", type: .info)
        chatManager?.stop()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        os_log("enter foreground", type: .info)
        chatManager?.start()
    }
}

