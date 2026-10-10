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
        return true
    }

    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        return UISceneConfiguration(name: "Default Configuration", sessionRole: connectingSceneSession.role)
    }
}
