import Foundation

/// CLI tests for the Echolet Home panel projection
/// (`EcholetHomePresenter`). Production-driven truth rules: warm hardware
/// vs dictation-session display distinction, honest pending/mic-off
/// states, real bundled model state, and the no-model-switching notice.
enum EcholetHomePresenterTests {

    static var failures: [String] = []
    static func recordFailure(_ name: String, _ detail: String) {
        failures.append("\(name): \(detail)")
        print("FAIL[\(name)]: \(detail)")
    }
    static func expectTrue(_ cond: Bool, _ name: String, _ detail: String = "") {
        if !cond { recordFailure(name, detail.isEmpty ? "expected true" : detail) }
    }
    static func expectEqual<T>(_ a: T, _ b: T, _ name: String, _ detail: String = "") {
        if String(describing: a) != String(describing: b) { recordFailure(name, detail.isEmpty ? "\(a) != \(b)" : detail) }
    }

    static func run() {
        testColdIdle()
        testWarmHardwareActiveNeverClaimsMicOff()
        testDictationSessionActiveWinsDisplay()
        testArmedWithoutHardwareIsPending()
        testModelStateProjection()
        testModelSwitchNoticeExists()

        if failures.isEmpty {
            print("EcholetHomePresenterTests: ALL PASS")
        } else {
            print("EcholetHomePresenterTests: \(failures.count) FAILURES")
        }
    }

    static func testColdIdle() {
        let p = EcholetHomePresenter.project(EcholetHomePresenter.Inputs())
        expectEqual(p.headline, "Ready", "testColdIdle(1)")
        expectTrue(p.prepareEnabled, "testColdIdle(2)")
        expectEqual(p.disarmEnabled, false, "testColdIdle(3)")
        expectEqual(p.micHardwareOn, false, "testColdIdle(4)")
        expectTrue(p.micDetail.lowercased().contains("microphone off"), "testColdIdle(5)")
    }

    static func testWarmHardwareActiveNeverClaimsMicOff() {
        // Warm hardware live WITHOUT a dictation session: keyboard stopped or
        // hidden, physical recorder still running (orange indicator truth).
        let p = EcholetHomePresenter.project(
            EcholetHomePresenter.Inputs(warmHardwareActive: true, dictationSessionActive: false)
        )
        expectEqual(p.headline, "Warm mic active", "testWarmHardwareActiveNeverClaimsMicOff(1)")
        expectEqual(p.micHardwareOn, true, "testWarmHardwareActiveNeverClaimsMicOff(2)")
        expectEqual(p.prepareEnabled, false, "testWarmHardwareActiveNeverClaimsMicOff(3)")
        expectEqual(p.disarmEnabled, true, "testWarmHardwareActiveNeverClaimsMicOff(4)")
        // No "microphone off" claim anywhere in the detail.
        expectTrue(!p.micDetail.lowercased().contains("microphone off"), "testWarmHardwareActiveNeverClaimsMicOff(5)")
        expectTrue(p.micDetail.lowercased().contains("orange"), "testWarmHardwareActiveNeverClaimsMicOff(6)")
    }

    static func testDictationSessionActiveWinsDisplay() {
        let p = EcholetHomePresenter.project(
            EcholetHomePresenter.Inputs(warmHardwareActive: true, dictationSessionActive: true)
        )
        expectEqual(p.headline, "Listening", "testDictationSessionActiveWinsDisplay(1)")
        expectEqual(p.micHardwareOn, true, "testDictationSessionActiveWinsDisplay(2)")
        expectEqual(p.prepareEnabled, false, "testDictationSessionActiveWinsDisplay(3)")
        expectEqual(p.disarmEnabled, true, "testDictationSessionActiveWinsDisplay(4)")
    }

    static func testArmedWithoutHardwareIsPending() {
        let p = EcholetHomePresenter.project(
            EcholetHomePresenter.Inputs(warmHardwareActive: false, micUserArmed: true)
        )
        expectEqual(p.headline, "Preparing…", "testArmedWithoutHardwareIsPending(1)")
        expectEqual(p.micHardwareOn, false, "testArmedWithoutHardwareIsPending(2)")
        expectTrue(!p.micDetail.lowercased().contains("microphone off"), "testArmedWithoutHardwareIsPending(3)")
        expectEqual(p.prepareEnabled, false, "testArmedWithoutHardwareIsPending(4)")
        expectEqual(p.disarmEnabled, true, "testArmedWithoutHardwareIsPending(5)")
    }

    static func testModelStateProjection() {
        let notLoaded = EcholetHomePresenter.projectModel(modelState: "notLoaded", modelName: nil)
        expectEqual(notLoaded.title, "X-ASR bilingual (installed offline pack)", "testModelStateProjection(1)")
        expectTrue(notLoaded.loadButtonEnabled, "testModelStateProjection(2)")
        expectTrue(notLoaded.capabilityLine.contains("Chinese") && notLoaded.capabilityLine.contains("English"), "testModelStateProjection(3)")

        let loading = EcholetHomePresenter.projectModel(modelState: "loading", modelName: "Chinese + English (X-ASR / 480ms)")
        expectEqual(loading.title, "Chinese + English (X-ASR / 480ms)", "testModelStateProjection(4)")
        expectEqual(loading.loadButtonEnabled, false, "testModelStateProjection(5)")
        expectTrue(loading.stateLine.lowercased().contains("loading"), "testModelStateProjection(6)")

        let loaded = EcholetHomePresenter.projectModel(modelState: "loaded", modelName: "Chinese + English (X-ASR / 480ms)")
        expectTrue(loaded.stateLine.lowercased().contains("loaded"), "testModelStateProjection(7)")
        expectEqual(loaded.loadButtonEnabled, false, "testModelStateProjection(8)")

        let failed = EcholetHomePresenter.projectModel(modelState: "failed", modelName: nil)
        expectTrue(failed.stateLine.lowercased().contains("fail"), "testModelStateProjection(9)")
        expectTrue(failed.loadButtonEnabled, "testModelStateProjection(10)")

        let unknown = EcholetHomePresenter.projectModel(modelState: "mystery", modelName: nil)
        expectEqual(unknown.stateLine, "mystery", "testModelStateProjection(11)")
        expectEqual(unknown.loadButtonEnabled, false, "testModelStateProjection(12)")
    }

    static func testModelSwitchNoticeExists() {
        expectTrue(EcholetHomePresenter.modelSwitchNotice.contains("not implemented"), "testModelSwitchNoticeExists(1)")
        expectTrue(
            EcholetHomePresenter.modelSwitchNotice.lowercased().contains("installed"),
            "testModelSwitchNoticeExists(2)"
        )
    }
}

@main
struct EcholetHomePresenterTestsMain {
    static func main() {
        EcholetHomePresenterTests.run()
        exit(EcholetHomePresenterTests.failures.isEmpty ? 0 : 1)
    }
}
