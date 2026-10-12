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

        // Cold launch coordination:
        // Handle URL contexts if present, or check fresh demand / ordered warm prepare.
        let coldURL = connectionOptions.urlContexts.first?.url
        let sourceApp = connectionOptions.urlContexts.first?.options.sourceApplication
        let urlCount = connectionOptions.urlContexts.count
        print("[Echolet-APP] SCENE_COLD_CONNECT(contexts=\(urlCount),sourceApp=\(sourceApp ?? "nil"))")
        if let source = sourceApp, !source.isEmpty, !source.contains("echolet") {
            WarmIPCService.shared.setLastReceivedSourceApp(source)
        }
        WarmIPCService.shared.coordinatedForegroundBootstrap(
            url: coldURL,
            source: "cold_connect",
            allowStandbyIfNoDemand: true
        )
    }

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        guard let context = URLContexts.first else { return }
        let url = context.url
        let sourceApp = context.options.sourceApplication
        print("[Echolet-APP] OPEN_URL_CONTEXTS(url=\(url),sourceApp=\(sourceApp ?? "nil"))")
        if let source = sourceApp, !source.isEmpty, !source.contains("echolet") {
            WarmIPCService.shared.setLastReceivedSourceApp(source)
        }
        WarmIPCService.shared.coordinatedForegroundBootstrap(
            url: url,
            source: "openURLContexts",
            allowStandbyIfNoDemand: false
        )
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        print("[Echolet-APP] SCENE_WILL_ENTER_FOREGROUND")
        // App became active/foreground: ensure WarmIPCService is polling
        WarmIPCService.shared.startPolling()
        // Idempotent foreground coordination: honors keyboard demand if pending,
        // or executes ordered warm bootstrap if idle and no demand.
        WarmIPCService.shared.coordinatedForegroundBootstrap(
            url: nil,
            source: "foreground",
            allowStandbyIfNoDemand: true
        )
    }

    func sceneDidBecomeActive(_ scene: UIScene) {
        print("[Echolet-APP] SCENE_DID_BECOME_ACTIVE")
    }

    func sceneWillResignActive(_ scene: UIScene) {
        print("[Echolet-APP] SCENE_WILL_RESIGN_ACTIVE")
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        print("[Echolet-APP] SCENE_DID_ENTER_BACKGROUND")
        // Background transition: retain polling only if active recording is underway
        if AudioCaptureController.shared.status != .recording {
            WarmIPCService.shared.stopPolling()
        }
    }
}
