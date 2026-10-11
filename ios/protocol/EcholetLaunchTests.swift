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
}

@main
struct EcholetLaunchTestsMain {
    static func main() {
        EcholetLaunchTests.run()
        exit(EcholetLaunchTests.failures.isEmpty ? 0 : 1)
    }
}
