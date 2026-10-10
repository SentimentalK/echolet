import UIKit

@main
class AppDelegate: UIResponder, UIApplicationDelegate {

    /// Per-process containing app epoch minted once upon process launch.
    /// Fences against stale IPC requests across app process lifetimes.
    static let sharedEpoch: String = UUID().uuidString
    static let appGroupId = "group.com.mainstayx.echolet.dev"

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        // Publish unique process epoch once per containing app process launch
        if let defaults = UserDefaults(suiteName: Self.appGroupId) {
            defaults.set(Self.sharedEpoch, forKey: EcholetIPC.appEpochKey)
            defaults.synchronize()
        }

        // Initialize and start process-owned WarmIPCService early so command intake is live
        WarmIPCService.shared.initializeGate()
        WarmIPCService.shared.startPolling()

        return true
    }

    func applicationWillEnterForeground(_ application: UIApplication) {
        // Re-read latest App Group state and resume polling upon foreground return
        WarmIPCService.shared.startPolling()
    }

    func applicationDidEnterBackground(_ application: UIApplication) {
        // While active recording is ongoing under UIBackgroundModes audio,
        // keep polling active as long as iOS genuinely schedules the app process.
        // If microphone is not actively recording, stop polling to avoid background CPU drains.
        if AudioCaptureController.shared.status != .recording {
            WarmIPCService.shared.stopPolling()
        }
    }

    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        return UISceneConfiguration(name: "Default Configuration", sessionRole: connectingSceneSession.role)
    }
}
