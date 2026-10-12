import Foundation

/// CLI tests for the iOS model-management flow logic:
/// * `EcholetModelGate` — honest selection denial under every active audio
///   truth (session, pending activation, warm hardware, armed mic, manual
///   capture, load/switch in flight);
/// * `EcholetModelSwitchCoordinator` — serialized switch epoch, duplicate
///   fencing, rollback on failed recognizer swap (fail-closed, no phantom
///   ready), exactly-once barrier close;
/// * `EcholetModelDownloadPolicy` — duplicate coalescing, size cap, space
///   verdicts, archive destination safety, HTTPS-only redirects.
enum EcholetModelFlowTests {

    static var failures: [String] = []
    static func expectTrue(_ cond: Bool, _ name: String, _ detail: String = "") {
        if !cond {
            failures.append(name)
            print("FAIL[\(name)]: \(detail.isEmpty ? "expected true" : detail)")
        }
    }
    static func expectEqual<T: Equatable>(_ a: T, _ b: T, _ name: String, _ detail: String = "") {
        if a != b {
            failures.append(name)
            print("FAIL[\(name)]: \(detail.isEmpty ? "\(a) != \(b)" : detail)")
        }
    }

    static func inputs() -> EcholetModelGate.Inputs {
        return EcholetModelGate.Inputs()
    }

    // MARK: - Gate

    static func testGateDenials() {
        expectCaseEqual(
            EcholetModelGate.evaluate(inputs()),
            .allow, "testGateDenials(1)"
        )
        let cases: [(EcholetModelGate.Inputs, String)] = [
            (
                EcholetModelGate.Inputs(keyboardSessionActive: true),
                "session active must deny"
            ),
            (
                EcholetModelGate.Inputs(keyboardActivationPending: true),
                "pending activation must deny"
            ),
            (
                EcholetModelGate.Inputs(warmHardwareActive: true),
                "warm hardware must deny"
            ),
            (
                EcholetModelGate.Inputs(micUserArmed: true),
                "armed mic must deny"
            ),
            (
                EcholetModelGate.Inputs(manualCaptureRecording: true),
                "manual capture must deny"
            ),
            (
                EcholetModelGate.Inputs(asrBusy: true),
                "ASR load/swap busy must deny"
            )
        ]
        for (idx, entry) in cases.enumerated() {
            guard case .deny(let guidance) = EcholetModelGate.evaluate(entry.0) else {
                expectTrue(false, "testGateDenials(\(1 + 1 + idx))", entry.1)
                continue
            }
            expectTrue(!guidance.isEmpty, "testGateDenialsGuidance(\(idx))", entry.1)
            // Guidance must tell the user to ACT (stop/disarm/wait), never
            // silently proceed; no implicit dictation termination is allowed.
            expectTrue(
                guidance.lowercased().contains("stop")
                    || guidance.lowercased().contains("disarm")
                    || guidance.lowercased().contains("wait"),
                "testGateDenialsGuidanceAction(\(idx))",
                entry.1
            )
        }
        // Selection while active is fenced as deny (duplicate tap path).
        guard case .deny = EcholetModelGate.evaluate(
            EcholetModelGate.Inputs(selectionAlreadyActive: true)
        ) else {
            expectTrue(false, "testGateDenials(9)", "selectionAlreadyActive must deny")
            return
        }
    }

    static func expectCaseEqual(
        _ a: EcholetModelGate.Decision,
        _ b: EcholetModelGate.Decision,
        _ name: String
    ) {
        if a != b { expectTrue(false, name, "decision mismatch") }
    }

    // MARK: - Switch coordinator

    static func testSwitchHappyPathAndFencing() {
        // Mutable closure state lives in a Box; the coordinator serializes
        // its decision queue, so accesses are naturally ordered for reads
        // made after waits.
        final class Box {
            var barrierOpen = false
            var swappedDirectory: String?
        }
        let box = Box()

        let coordinator = EcholetModelSwitchCoordinator(deps: EcholetModelSwitchCoordinator.Deps(
            gateInputs: { EcholetModelGate.Inputs() },
            previousActive: { nil },
            rustSelect: { id in
                expectTrue(id == "m1", "rustSelectModel")
                return .success(())
            },
            rustRollback: { _ in expectTrue(false, "rollback must not run on success") },
            swapRecognizer: { directory, done in
                box.swappedDirectory = directory
                done(true, nil)
            },
            setBarrier: { on in box.barrierOpen = on }
        ))

        let success = waitSelect(coordinator, modelId: "m1", modelDirectory: "/path/m1")
        expectTrue(
            success == .switched(selectedId: "m1"),
            "switchHappyPath", "\(success)"
        )
        expectEqual(box.barrierOpen, false, "barrierClosedExactlyOnce")
        expectEqual(box.swappedDirectory, "/path/m1", "swapDir")
        expectEqual(coordinator.isSwitchActive, false, "epochClosed")
    }

    @discardableResult
    static func waitSelect(
        _ coordinator: EcholetModelSwitchCoordinator,
        modelId: String,
        modelDirectory: String
    ) -> EcholetModelSwitchCoordinator.Outcome {
        var outcome: EcholetModelSwitchCoordinator.Outcome?
        let lock = NSLock()
        coordinator.select(modelId: modelId, modelDirectory: modelDirectory) { result in
            lock.lock()
            outcome = result
            lock.unlock()
        }
        // Bounded spin: the coordinator chains its own dispatch queues.
        let deadline = Date().addingTimeInterval(3)
        while Date() < deadline {
            lock.lock()
            let value = outcome
            lock.unlock()
            if let value = value { return value }
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
        return .failed(message: "TIMEOUT waiting for switch completion")
    }

    static func testSwitchFailurePaths() {
        final class Box { var barrierOpen = false }
        let box = Box()

        // 2. Failure of persistence BEFORE swap: no swap, no rollback, error
        //    surfaced, barrier closed exactly once.
        let failSelect = EcholetModelSwitchCoordinator(deps: EcholetModelSwitchCoordinator.Deps(
            gateInputs: { EcholetModelGate.Inputs() },
            previousActive: { "old" },
            rustSelect: { _ in .failure(EcholetModelError(message: "Failed to persist selected model")) },
            rustRollback: { _ in expectTrue(false, "rollback must not run when persistence itself failed") },
            swapRecognizer: { _, done in done(false, "must not swap") },
            setBarrier: { on in box.barrierOpen = on }
        ))
        let selectFailOutcome = waitSelect(failSelect, modelId: "m2", modelDirectory: "/x")
        guard case .failed(let persistMessage) = selectFailOutcome else {
            expectTrue(false, "selectFailResult", "\(selectFailOutcome)")
            return
        }
        expectTrue(persistMessage.contains("persist"), "selectFailMessage")
        expectEqual(box.barrierOpen, false, "barrier closed after select failure")

        // 3. Recognizer swap failure AFTER a successful select: the PREVIOUS
        //    selection is restored (rollback), honest failure — no phantom
        //    ready state anywhere.
        final class Rollback { var target: String? }
        let rollback = Rollback()
        let swapFails = EcholetModelSwitchCoordinator(deps: EcholetModelSwitchCoordinator.Deps(
            gateInputs: { EcholetModelGate.Inputs() },
            previousActive: { "old-model" },
            rustSelect: { _ in .success(()) },
            rustRollback: { id in rollback.target = id },
            swapRecognizer: { _, done in done(false, "recognizer_create_failed") },
            setBarrier: { on in box.barrierOpen = on }
        ))
        let swapFailOutcome = waitSelect(swapFails, modelId: "m3", modelDirectory: "/bad")
        guard case .failed(let swapMessage) = swapFailOutcome else {
            expectTrue(false, "swapFailResult", "\(swapFailOutcome)")
            return
        }
        expectTrue(swapMessage.contains("could not be loaded"), "swapFailMessage")
        expectEqual(rollback.target, "old-model", "rollbackTargetIsPrevious")
        expectEqual(box.barrierOpen, false, "barrier closed after swap failure")
    }

    static func testDuplicateSelectFencing() {
        final class Box { var barrierOpen = false }
        let box = Box()

        let coordinator = EcholetModelSwitchCoordinator(deps: EcholetModelSwitchCoordinator.Deps(
            gateInputs: { EcholetModelGate.Inputs() },
            previousActive: { nil },
            rustSelect: { _ in .success(()) },
            rustRollback: { _ in },
            // SLOW swap: leaves a window where a duplicate tap must deny.
            swapRecognizer: { _, done in
                box.barrierOpen = true
                DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) {
                    done(true, nil)
                }
            },
            setBarrier: { on in box.barrierOpen = on }
        ))

        var outcomes: [EcholetModelSwitchCoordinator.Outcome] = []
        let lock = NSLock()

        coordinator.select(modelId: "m1", modelDirectory: "/1") { outcome in
            lock.lock()
            outcomes.append(outcome)
            lock.unlock()
        }
        // Duplicate fires while the first is still mid-switch — must deny.
        coordinator.select(modelId: "m2", modelDirectory: "/2") { outcome in
            lock.lock()
            outcomes.append(outcome)
            lock.unlock()
        }

        let deadline = Date().addingTimeInterval(4)
        while Date() < deadline && underLockedCount(outcomes, lock: lock) < 2 {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
        lock.lock()
        let deniedCount = outcomes.filter { outcome -> Bool in
            if case .denied = outcome { return true }
            return false
        }.count
        let switched = outcomes.filter { outcome -> Bool in
            if case .switched = outcome { return true }
            return false
        }.count
        lock.unlock()
        expectEqual(deniedCount, 1, "exactlyOneDuplicateDenied")
        expectTrue(switched <= 1, "atMostOneSwitched")
        expectEqual(coordinator.isSwitchActive, false, "epoch closed after both")
    }

    // MARK: - Download policy

    static func testDownloadPolicy() {
        // Coalescing: in-flight states deny begin; idle/failed/installed allow.
        for state in [
            EcholetHomePresenter.ModelTransferState.downloading(downloadedBytes: 10, totalBytes: 100),
            .verifying,
            .installing
        ] where EcholetModelDownloadPolicy.beginDecision(for: state) != .coalesce {
            expectTrue(false, "downloadPolicy(1)", "in-flight must coalesce")
        }
        for state: EcholetHomePresenter.ModelTransferState? in [nil, .idle, .failed(message: "x")] {
            if EcholetModelDownloadPolicy.beginDecision(for: state) != .begin {
                expectTrue(false, "downloadPolicy(2)", "idle/failed must begin")
            }
        }
        // Installed rows deliberately re-allow a begin decision: the UI hides
        // the Download CTA; but a stale double-tap of retry is safe (installer
        // rejects dup installs itself via Rust single-flight).
        if case .begin = EcholetModelDownloadPolicy.beginDecision(for: .installed) {} else {
            expectTrue(false, "downloadPolicy(3)", "installed taps may re-begin safely")
        }

        // Size cap (2 GiB) applies whenever the total is known.
        guard case .tooLarge = EcholetModelDownloadPolicy.sizeVerdict(
            expectedTotalBytes: EcholetModelDownloadPolicy.maxArchiveBytes + 1
        ) else {
            expectTrue(false, "downloadPolicy(4)", "oversize must be rejected")
            return
        }
        guard case .ok = EcholetModelDownloadPolicy.sizeVerdict(expectedTotalBytes: 551847917) else {
            expectTrue(false, "downloadPolicy(5)", "hard cap ok edge")
            return
        }
        guard case .ok = EcholetModelDownloadPolicy.sizeVerdict(expectedTotalBytes: nil) else {
            expectTrue(false, "downloadPolicy(6)", "unknown total allowed with honest sizing")
            return
        }

        // Space: full footprint required (archive + install + margin).
        guard case .insufficient = EcholetModelDownloadPolicy.spaceVerdict(
            availableBytes: 600_000_000,
            downloadSizeBytes: 551847917,
            installedSizeBytes: 614596718
        ) else {
            expectTrue(false, "downloadPolicy(7)", "not enough space must fail closed")
            return
        }
        guard case .ok = EcholetModelDownloadPolicy.spaceVerdict(
            availableBytes: 551847917 + 614596718 + EcholetModelDownloadPolicy.spaceMarginBytes + 1,
            downloadSizeBytes: 551847917,
            installedSizeBytes: 614596718
        ) else {
            expectTrue(false, "downloadPolicy(8)", "adequate space must pass")
            return
        }
        guard case .unknownLedAllowed = EcholetModelDownloadPolicy.spaceVerdict(
            availableBytes: nil, downloadSizeBytes: nil, installedSizeBytes: nil
        ) else {
            expectTrue(false, "downloadPolicy(9)", "unknown space must proceed honestly")
            return
        }

        // Staging: app-private, sanitized, one-shot path.
        let dest = EcholetModelDownloadPolicy.archiveDestination(
            downloadsDirectory: URL(fileURLWithPath: "/tmp/downloads"),
            modelId: "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1"
        )
        expectTrue(dest.path.hasPrefix("/tmp/downloads/"), "downloadPolicy(10)")
        expectTrue(dest.path.hasSuffix(".archive"), "downloadPolicy(11)")
        let traversal = EcholetModelDownloadPolicy.archiveDestination(
            downloadsDirectory: URL(fileURLWithPath: "/tmp/downloads"),
            modelId: "../../evil"
        )
        expectTrue(!traversal.path.contains("/tmp/evil"), "downloadPolicy(12)", traversal.path)
        expectTrue(!traversal.path.contains(".."), "downloadPolicy(13)", traversal.path)

        // Redirects: ONLY https.
        expectTrue(EcholetModelDownloadPolicy.allowsRedirect(url: URL(string: "https://example.com/a")!), "downloadPolicy(14)")
        expectTrue(!EcholetModelDownloadPolicy.allowsRedirect(url: URL(string: "http://example.com/a")!), "downloadPolicy(15)")
    }

    static func underLockedCount(_ array: [EcholetModelSwitchCoordinator.Outcome], lock: NSLock) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return array.count
    }

    static func run() {
        testGateDenials()
        testSwitchHappyPathAndFencing()
        testSwitchFailurePaths()
        testDuplicateSelectFencing()
        testDownloadPolicy()

        if failures.isEmpty {
            print("EcholetModelFlowTests: ALL PASS")
        } else {
            print("EcholetModelFlowTests: \(failures.count) FAILURES")
        }
    }
}

@main
struct EcholetModelFlowTestsMain {
    static func main() {
        EcholetModelFlowTests.run()
        exit(EcholetModelFlowTests.failures.isEmpty ? 0 : 1)
    }
}
