import Foundation

/// CLI tests for the J5 launch handshake records and deep-link URL helpers.
/// Compiled against EcholetLaunch.swift only (pure Foundation, macOS).
enum EcholetLaunchTests {

    static var failures: [String] = []

    static func recordFailure(_ name: String, _ detail: String) {
        failures.append("\(name): \(detail)")
        print("FAIL[\(name)]: \(detail)")
    }

    static func expectTrue(_ cond: Bool, _ name: String, _ detail: String = "") {
        if !cond { recordFailure(name, detail.isEmpty ? "expected true" : detail) }
    }

    static func expectNil<T>(_ value: T?, _ name: String, _ detail: String = "") {
        if let value = value { recordFailure(name, detail.isEmpty ? "expected nil, got \(value)" : detail) }
    }

    static func expectEqual<T>(_ a: T, _ b: T, _ name: String, _ detail: String = "") {
        if String(describing: a) != String(describing: b) { recordFailure(name, detail.isEmpty ? "\(a) != \(b)" : detail) }
    }

    // Unique UserDefaults suite per run
    static func freshDefaults() -> UserDefaults {
        let suiteName = "test.echolet.launch.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        return defaults
    }

    static func run() {
        testURLRoundtrip()
        testURLRejectsForeignSchemes()
        testDemandWriteReadConsume()
        testDemandFreshness()
        testPhaseFencing()
        testWarmLiveAuthority()
        testWarmLiveInactiveAndStaleEpochFails()
        testHostReturnContextScoping()
        testHostReturnContextFreshnessExpiry()
        testNormalizeHostBundleId()
        testReturnableHostBundleId()
        testOrderedReturnHosts()
        testKnownReturnHosts()
        testReturnSteps()
        testLegacyHostBundleIdSync()
        testPendingStartIntentHandoff()

        if failures.isEmpty {
            print("EcholetLaunchTests: ALL PASS")
        } else {
            print("EcholetLaunchTests: \(failures.count) FAILURES")
        }
    }

    // MARK: - URL construction / parsing

    static func testURLRoundtrip() {
        let nonce = "12345678-1234-1234-1234-123456789abc"
        guard let url = try? EcholetLaunch.makeDictationPrepareURL(nonce: nonce, tsMs: 1728670000000) else {
            recordFailure("testURLRoundtrip", "url build failed")
            return
        }
        expectTrue(url.absoluteString.hasPrefix("echolet://dictation/prepare?"), "testURLRoundtrip", url.absoluteString)
        guard let parsed = EcholetLaunch.parseDictationPrepareURL(url) else {
            recordFailure("testURLRoundtrip", "parse failed")
            return
        }
        expectEqual(parsed.nonce, nonce, "testURLRoundtrip")
        expectEqual(parsed.tsMs, UInt64(1728670000000), "testURLRoundtrip")
    }

    static func testURLRejectsForeignSchemes() {
        let foreign1 = URL(string: "echodict://dictation/prepare?nonce=abcdefgh")!
        expectNil(EcholetLaunch.parseDictationPrepareURL(foreign1), "testURLRejectsForeignSchemes.1")
        let foreign2 = URL(string: "echolet://keyboard/finish?nonce=abcdefgh")!
        expectNil(EcholetLaunch.parseDictationPrepareURL(foreign2), "testURLRejectsForeignSchemes.2")
        let foreign3 = URL(string: "echolet://dictation/other?nonce=abcdefgh")!
        expectNil(EcholetLaunch.parseDictationPrepareURL(foreign3), "testURLRejectsForeignSchemes.3")
        let foreign4 = URL(string: "echolet://dictation/prepare")! // missing nonce
        expectNil(EcholetLaunch.parseDictationPrepareURL(foreign4), "testURLRejectsForeignSchemes.4")
    }

    // MARK: - Demand record lifecycle

    static func testDemandWriteReadConsume() {
        let defaults = freshDefaults()
        let demand = EcholetLaunch.DemandRecord(nonce: "nonce-alpha-0001", createdAtMs: 1000, ttlMs: 5000)
        expectTrue(EcholetLaunch.writeDemand(demand, defaults: defaults), "testDemandWriteReadConsume(1)")
        let read = EcholetLaunch.readDemand(defaults: defaults)
        expectEqual(read, demand, "testDemandWriteReadConsume(2)")
        let consumed = EcholetLaunch.consumeDemand(defaults: defaults)
        expectEqual(consumed, demand, "testDemandWriteReadConsume(3)")
        expectNil(EcholetLaunch.consumeDemand(defaults: defaults), "testDemandWriteReadConsume(4)")
        expectNil(EcholetLaunch.readDemand(defaults: defaults), "testDemandWriteReadConsume(5)")
    }

    static func testDemandFreshness() {
        let defaults = freshDefaults()
        let demand = EcholetLaunch.DemandRecord(nonce: "nonce-bravo-0002", createdAtMs: 1000, ttlMs: EcholetLaunch.defaultDemandTtlMs)
        EcholetLaunch.writeDemand(demand, defaults: defaults)
        expectTrue(EcholetLaunch.isDemandFresh(demand, nowMs: 1000), "testDemandFreshness(1)")
        expectTrue(EcholetLaunch.isDemandFresh(demand, nowMs: 1000 + EcholetLaunch.defaultDemandTtlMs), "testDemandFreshness(2)")
        let expired = EcholetLaunch.DemandRecord(nonce: "nonce-bravo-0002", createdAtMs: 1000, ttlMs: EcholetLaunch.defaultDemandTtlMs)
        expectTrue(!EcholetLaunch.isDemandFresh(expired, nowMs: 1000 + EcholetLaunch.defaultDemandTtlMs + 1), "testDemandFreshness(3)")
        let shortTtl = EcholetLaunch.DemandRecord(nonce: "nonce-bravo-0002", createdAtMs: 1000, ttlMs: 10)
        EcholetLaunch.writeDemand(shortTtl, defaults: defaults)
        expectTrue(!EcholetLaunch.isDemandFresh(shortTtl, nowMs: 1011), "testDemandFreshness(4)")
    }

    // MARK: - Phase record fencing

    static func testPhaseFencing() {
        let defaults = freshDefaults()
        let epoch = "app-epoch-aaaa"
        let now: UInt64 = 9000
        expectTrue(EcholetLaunch.writePhase(nonce: "nonce-charlie-003", appEpoch: epoch, phase: .warmReady, nowMs: now, defaults: defaults), "testPhaseFencing(1)")

        // Matching nonce + epoch + freshness
        let match = EcholetLaunch.freshPhaseMatching(nonce: "nonce-charlie-003", appEpoch: epoch, nowMs: now, defaults: defaults)
        expectEqual(match?.phase, EcholetLaunch.Phase.warmReady, "testPhaseFencing(2)")

        // Wrong nonce rejected
        expectNil(EcholetLaunch.freshPhaseMatching(nonce: "nonce-delta-004", appEpoch: epoch, nowMs: now, defaults: defaults), "testPhaseFencing(3)")
        // Wrong epoch rejected (dead/old app process observation)
        expectNil(EcholetLaunch.freshPhaseMatching(nonce: "nonce-charlie-003", appEpoch: "other-epoch", nowMs: now, defaults: defaults), "testPhaseFencing(4)")
        // Expired rejected
        expectNil(EcholetLaunch.freshPhaseMatching(nonce: "nonce-charlie-003", appEpoch: epoch, nowMs: now + EcholetLaunch.defaultPhaseTtlMs + 1, defaults: defaults), "testPhaseFencing(5)")
        // Older observation under later-overwrite cannot regress (latest wins)
        EcholetLaunch.writePhase(nonce: "nonce-charlie-003", appEpoch: epoch, phase: .modelLoaded, nowMs: now + 100, defaults: defaults)
        let latest = EcholetLaunch.freshPhaseMatching(nonce: "nonce-charlie-003", appEpoch: epoch, nowMs: now + 100, defaults: defaults)
        expectEqual(latest?.phase, EcholetLaunch.Phase.modelLoaded, "testPhaseFencing(6)")
    }

    // MARK: - Warm live authority

    static func testWarmLiveAuthority() {
        let defaults = freshDefaults()
        let epoch = "app-epoch-warm"
        let now: UInt64 = 50_000
        EcholetLaunch.writeWarmLive(appEpoch: epoch, recorderActive: true, nowMs: now, defaults: defaults)
        let live = EcholetLaunch.freshWarmLive(nowMs: now, currentAppEpoch: epoch, defaults: defaults)
        expectTrue(live?.recorderActive == true, "testWarmLiveAuthority(1)")
        // Just-inside freshness window (10s)
        let edge = EcholetLaunch.freshWarmLive(nowMs: now + EcholetLaunch.defaultWarmLiveFreshnessMs, currentAppEpoch: epoch, defaults: defaults)
        expectTrue(edge != nil, "testWarmLiveAuthority(2)")
    }

    static func testWarmLiveInactiveAndStaleEpochFails() {
        let defaults = freshDefaults()
        let now: UInt64 = 50_000
        // Inactive recorder never counts as warm.
        EcholetLaunch.writeWarmLive(appEpoch: "epoch-a", recorderActive: false, nowMs: now, defaults: defaults)
        expectNil(EcholetLaunch.freshWarmLive(nowMs: now, currentAppEpoch: "epoch-a", defaults: defaults), "testWarmLiveInactive(1)")

        // Stale cold app: epoch mismatch == dead app process.
        EcholetLaunch.writeWarmLive(appEpoch: "epoch-old", recorderActive: true, nowMs: now, defaults: defaults)
        expectNil(EcholetLaunch.freshWarmLive(nowMs: now, currentAppEpoch: "epoch-new", defaults: defaults), "testWarmLiveStaleEpoch(2)")

        // Aged record beyond freshness window.
        EcholetLaunch.writeWarmLive(appEpoch: "epoch-a", recorderActive: true, nowMs: now, defaults: defaults)
        expectNil(EcholetLaunch.freshWarmLive(nowMs: now + EcholetLaunch.defaultWarmLiveFreshnessMs + 1, currentAppEpoch: "epoch-a", defaults: defaults), "testWarmLiveAgeFence(3)")

        // No app epoch published at all.
        expectNil(EcholetLaunch.freshWarmLive(nowMs: now, currentAppEpoch: nil, defaults: defaults), "testWarmLiveNoEpoch(4)")
    }

    // MARK: - Nonce-bound HostReturnContext & PendingStartIntent tests

    static func testHostReturnContextScoping() {
        let defaults = freshDefaults()
        let now: UInt64 = 100_000
        let nonceA = "nonce-alpha-1234"
        let nonceB = "nonce-bravo-5678"

        let ctxA = EcholetLaunch.HostReturnContext(launchNonce: nonceA, hostBundleId: "com.google.chrome.ios", capturedAtMs: now)
        expectTrue(EcholetLaunch.writeHostReturnContext(ctxA, defaults: defaults), "testHostReturnContextScoping(1)")

        // Matching nonce recovers the correct bundle ID
        let readA = EcholetLaunch.freshHostReturnContext(forNonce: nonceA, nowMs: now, defaults: defaults)
        expectEqual(readA?.hostBundleId, "com.google.chrome.ios", "testHostReturnContextScoping(2)")

        // Unmatched nonce fails closed (prevent jumping back to Chrome from another app!)
        let readB = EcholetLaunch.freshHostReturnContext(forNonce: nonceB, nowMs: now, defaults: defaults)
        expectNil(readB, "testHostReturnContextScoping(3)")

        // Clear removes context
        EcholetLaunch.clearHostReturnContext(defaults: defaults)
        expectNil(EcholetLaunch.readHostReturnContext(defaults: defaults), "testHostReturnContextScoping(4)")
    }

    static func testHostReturnContextFreshnessExpiry() {
        let defaults = freshDefaults()
        let now: UInt64 = 200_000
        let nonce = "nonce-expiry-test"

        let ctx = EcholetLaunch.HostReturnContext(launchNonce: nonce, hostBundleId: "com.google.chrome.ios", capturedAtMs: now)
        expectTrue(EcholetLaunch.writeHostReturnContext(ctx, defaults: defaults), "testHostReturnContextExpiry(1)")

        // Within TTL (e.g. at 59 seconds) is fresh
        let fresh = EcholetLaunch.freshHostReturnContext(forNonce: nonce, nowMs: now + 59_000, defaults: defaults)
        expectEqual(fresh?.hostBundleId, "com.google.chrome.ios", "testHostReturnContextExpiry(2)")

        // Past default TTL (60_000 ms) fails closed
        let expired = EcholetLaunch.freshHostReturnContext(forNonce: nonce, nowMs: now + EcholetLaunch.defaultHostReturnTtlMs + 1, defaults: defaults)
        expectNil(expired, "testHostReturnContextExpiry(3)")
    }

    static func testNormalizeHostBundleId() {
        expectNil(EcholetLaunch.normalizeHostBundleId(nil), "testNormalizeHostBundleId(nil)")
        expectNil(EcholetLaunch.normalizeHostBundleId(""), "testNormalizeHostBundleId(empty)")
        expectNil(EcholetLaunch.normalizeHostBundleId("   \n\t  "), "testNormalizeHostBundleId(whitespace)")
        expectNil(EcholetLaunch.normalizeHostBundleId("<null>"), "testNormalizeHostBundleId(nullTag1)")
        expectNil(EcholetLaunch.normalizeHostBundleId("(null)"), "testNormalizeHostBundleId(nullTag2)")
        expectEqual(EcholetLaunch.normalizeHostBundleId(" com.google.chrome.ios \n"), "com.google.chrome.ios", "testNormalizeHostBundleId(trimmed)")
    }

    static func testReturnableHostBundleId() {
        expectTrue(EcholetLaunch.isReturnableHostBundleId("com.google.chrome.ios"), "testReturnableHostBundleId(chrome)")
        expectTrue(EcholetLaunch.isReturnableHostBundleId("com.apple.Maps"), "testReturnableHostBundleId(maps)")
        expectTrue(EcholetLaunch.isReturnableHostBundleId(" com.apple.mobilesafari "), "testReturnableHostBundleId(trimmed)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId(nil), "testReturnableHostBundleId(nil)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId(""), "testReturnableHostBundleId(empty)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId("com.apple.springboard"), "testReturnableHostBundleId(springboard)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId("com.apple.WebKit.WebContent"), "testReturnableHostBundleId(webkit)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId("com.mainstayx.echolet.dev.keyboard"), "testReturnableHostBundleId(self)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId("8F3A1C2E-9B44-4D1A-A1C0-0E5B6A7C8D9E"), "testReturnableHostBundleId(uuid)")
        expectTrue(!EcholetLaunch.isReturnableHostBundleId("chrome"), "testReturnableHostBundleId(bare)")
    }

    static func testOrderedReturnHosts() {
        let both = EcholetLaunch.orderedReturnHosts(
            breadcrumb: "com.google.chrome.ios",
            storedHost: "com.apple.Maps",
            sourceApp: nil
        )
        expectEqual(both, ["com.apple.Maps", "com.google.chrome.ios"], "testOrderedReturnHosts(storedFirst)")

        let storedOnly = EcholetLaunch.orderedReturnHosts(breadcrumb: nil, storedHost: "com.apple.Maps", sourceApp: nil)
        expectEqual(storedOnly, ["com.apple.Maps"], "testOrderedReturnHosts(stored)")

        let filtered = EcholetLaunch.orderedReturnHosts(
            breadcrumb: "com.apple.springboard",
            storedHost: "com.google.chrome.ios",
            sourceApp: "com.mainstayx.echolet.dev"
        )
        expectEqual(filtered, ["com.google.chrome.ios"], "testOrderedReturnHosts(filtered)")

        let deduped = EcholetLaunch.orderedReturnHosts(
            breadcrumb: "com.google.chrome.ios",
            storedHost: "com.google.chrome.ios",
            sourceApp: "com.google.chrome.ios"
        )
        expectEqual(deduped, ["com.google.chrome.ios"], "testOrderedReturnHosts(deduped)")
    }

    static func testKnownReturnHosts() {
        expectEqual(EcholetLaunch.knownReturnHost(bundleId: "com.apple.DocumentsApp")?.name, "Apple Files", "testKnownReturnHosts(files)")
        expectEqual(EcholetLaunch.returnURL(forHostBundleId: "com.google.chrome.ios")?.absoluteString, "googlechrome://", "testKnownReturnHosts(chromeURL)")
        expectEqual(EcholetLaunch.returnURL(forHostBundleId: "com.apple.DocumentsApp")?.absoluteString, "shareddocuments://", "testKnownReturnHosts(filesURL)")
        expectEqual(EcholetLaunch.returnURL(forHostBundleId: "com.openai.chat")?.absoluteString, "chatgpt://", "testKnownReturnHosts(chatgptURL)")
        expectEqual(EcholetLaunch.returnURL(forHostBundleId: "com.apple.Maps")?.absoluteString, "maps://", "testKnownReturnHosts(mapsURL)")
        expectNil(EcholetLaunch.returnURL(forHostBundleId: "com.example.unknown.app"), "testKnownReturnHosts(unknown)")
        expectTrue(EcholetLaunch.isReturnableHostBundleId("com.apple.DocumentsApp"), "testKnownReturnHosts(filesReturnable)")
    }

    static func testReturnSteps() {
        expectEqual(EcholetLaunch.initialReturnStep(hostBundleId: nil), .stay("no_returnable_host"), "testReturnSteps(nil)")
        expectEqual(EcholetLaunch.initialReturnStep(hostBundleId: "com.apple.springboard"), .stay("no_returnable_host"), "testReturnSteps(springboard)")
        expectEqual(
            EcholetLaunch.initialReturnStep(hostBundleId: "com.google.chrome.ios"),
            .openBundle("com.google.chrome.ios"),
            "testReturnSteps(chromeBundle)"
        )
        expectEqual(
            EcholetLaunch.afterBundleOpenRejected(hostBundleId: "com.google.chrome.ios"),
            .openURL(URL(string: "googlechrome://")!, bundleId: "com.google.chrome.ios"),
            "testReturnSteps(chromeURLFallback)"
        )
        expectEqual(
            EcholetLaunch.afterBundleOpenRejected(hostBundleId: "com.example.unknown.app"),
            .stay("bundle_open_rejected"),
            "testReturnSteps(unknownRejected)"
        )
    }

    static func testLegacyHostBundleIdSync() {
        let defaults = freshDefaults()
        let now: UInt64 = 300_000
        let nonce = "nonce-legacy-sync"

        let ctx = EcholetLaunch.HostReturnContext(launchNonce: nonce, hostBundleId: "com.google.chrome.ios", capturedAtMs: now)
        _ = EcholetLaunch.writeHostReturnContext(ctx, defaults: defaults)

        // Legacy key is synchronized
        let legacy = defaults.string(forKey: EcholetLaunch.hostBundleIdKey)
        expectEqual(legacy, "com.google.chrome.ios", "testLegacyHostBundleIdSync(1)")

        // Clear removes both
        EcholetLaunch.clearHostReturnContext(defaults: defaults)
        expectNil(defaults.string(forKey: EcholetLaunch.hostBundleIdKey), "testLegacyHostBundleIdSync(2)")
        expectNil(EcholetLaunch.readHostReturnContext(defaults: defaults), "testLegacyHostBundleIdSync(3)")
    }

    static func testPendingStartIntentHandoff() {
        let defaults = freshDefaults()
        let now: UInt64 = 100_000
        let nonce = "nonce-intent-4321"

        let intent = EcholetLaunch.PendingStartIntent(launchNonce: nonce, initiatedAtMs: now, ttlMs: 45_000)
        expectTrue(EcholetLaunch.writePendingStartIntent(intent, defaults: defaults), "testPendingStartIntentHandoff(1)")

        // Fresh intent is readable
        let fresh = EcholetLaunch.freshPendingStartIntent(nowMs: now + 500, defaults: defaults)
        expectEqual(fresh?.launchNonce, nonce, "testPendingStartIntentHandoff(2)")

        // Expired intent fails closed
        let expired = EcholetLaunch.freshPendingStartIntent(nowMs: now + 45_001, defaults: defaults)
        expectNil(expired, "testPendingStartIntentHandoff(3)")

        // Clear removes intent
        EcholetLaunch.clearPendingStartIntent(defaults: defaults)
        expectNil(EcholetLaunch.readPendingStartIntent(defaults: defaults), "testPendingStartIntentHandoff(4)")
    }
}

@main
struct EcholetLaunchTestsMain {
    static func main() {
        EcholetLaunchTests.run()
        exit(EcholetLaunchTests.failures.isEmpty ? 0 : 1)
    }
}
