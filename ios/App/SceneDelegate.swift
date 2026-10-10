import UIKit

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = (scene as? UIWindowScene) else { return }
        let window = UIWindow(windowScene: windowScene)
        window.rootViewController = AppStatusViewController()
        self.window = window
        window.makeKeyAndVisible()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        // App became active/foreground: ensure WarmIPCService is polling
        WarmIPCService.shared.startPolling()
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        // Background transition: retain polling only if active recording is underway
        if AudioCaptureController.shared.status != .recording {
            WarmIPCService.shared.stopPolling()
        }
    }
}
