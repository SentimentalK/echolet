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

        // Cold launch through the echolet:// deep link: the URL contexts
        // arrive with the scene connection options.
        for context in connectionOptions.urlContexts {
            WarmIPCService.shared.handleDictationActivationURL(context.url, source: "cold_launch")
        }
        // J5 manual foreground hand: a fresh unfulfilled keyboard demand
        // (attempted open was refused) is honored the same as a deep link.
        WarmIPCService.shared.activatePendingDemandIfFresh(source: "cold_connect")
    }

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        for context in URLContexts {
            WarmIPCService.shared.handleDictationActivationURL(context.url, source: "openURLContexts")
        }
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        // App became active/foreground: ensure WarmIPCService is polling
        WarmIPCService.shared.startPolling()
        // Manual foreground hand for pending keyboard launch demands.
        WarmIPCService.shared.activatePendingDemandIfFresh(source: "foreground")
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        // Background transition: retain polling only if active recording is underway
        if AudioCaptureController.shared.status != .recording {
            WarmIPCService.shared.stopPolling()
        }
    }
}
