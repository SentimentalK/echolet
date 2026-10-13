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
        testModelCatalogRows()
        testModelCatalogTransferStates()
        testExperimentalRowsAreDownloadableAndSelectable()
        testActiveRowStaysSelectedAfterInstallTransfer()
        testSelectShowsLoading()

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
        // NoModel home: prepare disabled until the user downloads + selects.
        let p = EcholetHomePresenter.project(EcholetHomePresenter.Inputs(modelInstalled: true))
        expectEqual(p.headline, "Ready", "testModelStateProjection(1)")
        expectTrue(p.prepareEnabled, "testModelStateProjection(2)")

        let noModel = EcholetHomePresenter.project(EcholetHomePresenter.Inputs(modelInstalled: false))
        expectEqual(noModel.headline, "No Model", "testModelStateProjection(3)")
        expectEqual(noModel.prepareEnabled, false, "testModelStateProjection(4)")
        expectTrue(
            noModel.micDetail.lowercased().contains("download"),
            "testModelStateProjection(5)"
        )
        // Browsing models never auto-starts the mic.
        expectEqual(noModel.micHardwareOn, false, "testModelStateProjection(6)")
    }

    static func testModelSwitchNoticeExists() {
        // Any no-model state must point at a real download, not a promise.
        let noModel = EcholetHomePresenter.project(EcholetHomePresenter.Inputs(modelInstalled: false))
        expectTrue(
            noModel.micDetail.lowercased().contains("offline model"),
            "testModelSwitchNoticeExists(1)"
        )
        expectTrue(
            !noModel.micDetail.lowercased().contains("automatically"),
            "testModelSwitchNoticeExists(2)"
        )
    }

    static func verifiedXasr(
        installed: Bool,
        active: Bool
    ) -> EcholetHomePresenter.ModelEntry {
        return EcholetHomePresenter.ModelEntry(
            id: "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
            displayName: "X-ASR 0.16B",
            version: "2026",
            languages: ["zh", "en"],
            verificationStatus: "Echolet Verified",
            runtimeCompatible: true,
            installed: installed,
            active: active,
            downloadSizeBytes: 551847917,
            installedSizeBytes: 614596718
        )
    }

    static func experimentalKroko(installed: Bool) -> EcholetHomePresenter.ModelEntry {
        return EcholetHomePresenter.ModelEntry(
            id: "echolet-kroko-streaming-en-2025-08-06-r1",
            displayName: "Kroko English Streaming 0.066B",
            version: "2025",
            languages: ["en"],
            verificationStatus: "Experimental",
            runtimeCompatible: false,
            installed: installed,
            active: false,
            downloadSizeBytes: 57267600,
            installedSizeBytes: 71053214
        )
    }

    static func testModelCatalogRows() {
        // Not installed + verified → honest Not Installed + Download CTA.
        let download = EcholetHomePresenter.projectModelRow(verifiedXasr(installed: false, active: false), transfer: .idle)
        expectEqual(download.stateLine, "Not installed", "testModelCatalogRows(1)")
        expectEqual(download.action, .download, "testModelCatalogRows(2)")
        expectTrue(download.stateLine.lowercased().contains("not installed"), "testModelCatalogRows(3)")

        // Installed selected → read-only Selected (no fake re-Select).
        let selected = EcholetHomePresenter.projectModelRow(verifiedXasr(installed: true, active: true), transfer: .idle)
        expectEqual(selected.stateLine, "Selected", "testModelCatalogRows(4)")
        expectEqual(selected.action, .selected, "testModelCatalogRows(5)")

        // Installed unselected → Select action (proven runtime only).
        let selectable = EcholetHomePresenter.projectModelRow(verifiedXasr(installed: true, active: false), transfer: .idle)
        expectEqual(selectable.stateLine, "Installed", "testModelCatalogRows(6)")
        expectEqual(selectable.action, .select, "testModelCatalogRows(7)")
    }

    static func testModelCatalogTransferStates() {
        let entry = verifiedXasr(installed: false, active: false)

        let downloading = EcholetHomePresenter.projectModelRow(
            entry,
            transfer: .downloading(downloadedBytes: 300_000_000, totalBytes: 551_847_917)
        )
        expectTrue(downloading.stateLine.lowercased().contains("downloading"), "testModelCatalogTransferStates(1)")
        expectTrue(downloading.stateLine.contains("%"), "testModelCatalogTransferStates(2)")
        expectEqual(downloading.action, .busy(label: "54%"), "testModelCatalogTransferStates(3)")
        expectEqual(downloading.actionEnabled, false, "testModelCatalogTransferStates(3b)")

        let verifying = EcholetHomePresenter.projectModelRow(entry, transfer: .verifying)
        expectEqual(verifying.stateLine, "Verifying checksum…", "testModelCatalogTransferStates(4)")
        expectEqual(verifying.action, .busy(label: "Verifying"), "testModelCatalogTransferStates(4b)")

        let installing = EcholetHomePresenter.projectModelRow(entry, transfer: .installing)
        expectEqual(installing.stateLine, "Installing…", "testModelCatalogTransferStates(5)")
        expectEqual(installing.action, .busy(label: "Installing"), "testModelCatalogTransferStates(5b)")

        // Installed ONLY after the real Rust read-back, and the button stays Select.
        let installed = EcholetHomePresenter.projectModelRow(
            verifiedXasr(installed: true, active: false),
            transfer: .installed
        )
        expectEqual(installed.stateLine, "Installed", "testModelCatalogTransferStates(6)")
        expectEqual(installed.action, .select, "testModelCatalogTransferStates(7)")
        // A not-yet-installed entry must NEVER claim installed.
        let premature = EcholetHomePresenter.projectModelRow(
            verifiedXasr(installed: false, active: false),
            transfer: .installed
        )
        expectEqual(premature.stateLine, "Not installed", "testModelCatalogTransferStates(8)")
        expectEqual(premature.action, .download, "testModelCatalogTransferStates(8b)")

        let failed = EcholetHomePresenter.projectModelRow(entry, transfer: .failed(message: "HTTP 404"))
        expectTrue(failed.stateLine.lowercased().contains("4"), "testModelCatalogTransferStates(9)")
        expectEqual(failed.action, .retryDownload, "testModelCatalogTransferStates(10)")
    }

    static func testExperimentalRowsAreDownloadableAndSelectable() {
        // Experimental entries stay visible AND actionable, matching desktop:
        // Download when absent, Select when installed.
        let idle = EcholetHomePresenter.projectModelRow(experimentalKroko(installed: false), transfer: .idle)
        expectEqual(idle.action, .download, "testExperimentalRowsAreDownloadableAndSelectable(1)")
        expectEqual(idle.actionTitle, "Download", "testExperimentalRowsAreDownloadableAndSelectable(1b)")
        expectTrue(idle.detail.lowercased().contains("experimental"), "testExperimentalRowsAreDownloadableAndSelectable(2)")
        expectEqual(idle.badge, "Experimental", "testExperimentalRowsAreDownloadableAndSelectable(2b)")

        let evenInstalled = EcholetHomePresenter.projectModelRow(
            experimentalKroko(installed: true),
            transfer: .installed
        )
        expectEqual(evenInstalled.action, .select, "testExperimentalRowsAreDownloadableAndSelectable(3)")
        expectEqual(evenInstalled.stateLine, "Installed", "testExperimentalRowsAreDownloadableAndSelectable(3b)")
        expectTrue(
            evenInstalled.detail.lowercased().contains("experimental"),
            "testExperimentalRowsAreDownloadableAndSelectable(4)"
        )
    }

    static func testActiveRowStaysSelectedAfterInstallTransfer() {
        // A finished install must not keep offering Select once Rust says
        // this row is the active model.
        let selected = EcholetHomePresenter.projectModelRow(
            verifiedXasr(installed: true, active: true),
            transfer: .installed
        )
        expectEqual(selected.action, .selected, "testActiveRowStaysSelectedAfterInstallTransfer(1)")
        expectEqual(selected.stateLine, "Selected", "testActiveRowStaysSelectedAfterInstallTransfer(2)")
        expectEqual(selected.actionTitle, "Selected", "testActiveRowStaysSelectedAfterInstallTransfer(3)")
    }

    static func testSelectShowsLoading() {
        let loading = EcholetHomePresenter.projectModelRow(
            verifiedXasr(installed: true, active: false),
            transfer: .installed,
            selectInProgress: true
        )
        expectEqual(loading.stateLine, "Loading model…", "testSelectShowsLoading(1)")
        expectEqual(loading.action, .busy(label: "Loading…"), "testSelectShowsLoading(2)")
        expectEqual(loading.actionEnabled, false, "testSelectShowsLoading(3)")

        let recognizer = EcholetHomePresenter.projectModelRow(
            verifiedXasr(installed: true, active: true),
            transfer: .idle,
            recognizerLoading: true
        )
        expectEqual(recognizer.action, .busy(label: "Loading…"), "testSelectShowsLoading(4)")
    }
}

@main
struct EcholetHomePresenterTestsMain {
    static func main() {
        EcholetHomePresenterTests.run()
        exit(EcholetHomePresenterTests.failures.isEmpty ? 0 : 1)
    }
}
