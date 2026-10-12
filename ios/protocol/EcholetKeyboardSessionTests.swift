import Foundation

/// CLI tests for the J5 keyboard single-button session controller
/// (`EcholetKeyboardSessionController`). Exercises the production pure
/// Foundation logic with fakes: fake navigation/response proxies, fake
/// open-host seam, fake clock, fake editor fingerprints. Compiled against
/// EcholetIPC.swift + EcholetAdmission.swift + EcholetLaunch.swift +
/// EcholetKeyboardSessionController.swift (no UIKit).
enum EcholetKeyboardSessionTests {

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

    // MARK: - Fake harness

    final class FakeHarness {
        var now: UInt64 = 100_000
        var appEpoch: String = "epoch-live-1"
        var openCalls: [(url: URL, auto: Bool)] {
            openCallsRaw
        }
        var openCallsRaw: [(url: URL, auto: Bool)] = []
        var nextOpenResult: EcholetKeyboardSessionController.OpenAttemptResult = .initiated
        var insertedTexts: [String] = []
        var deleteBackwardCounts: [Int] = []
        var fingerprints: [(before: String?, after: String?, selected: String?)] = [(nil, nil, nil)]
        var logs: [String] = []
        let defaults: UserDefaults

        init(appGroup: String = "test.echolet.session.\(UUID().uuidString)") {
            defaults = UserDefaults(suiteName: appGroup)!
        }

        var fingerprint: (before: String?, after: String?, selected: String?) {
            return fingerprints.last ?? (nil, nil, nil)
        }

        func makeFingerprint() -> (before: String?, after: String?, selected: String?) {
            let marker = "fp-\(fingerprints.count)-\(now)"
            fingerprints.append((marker, nil, nil))
            return fingerprints.last!
        }

        func controller() -> EcholetKeyboardSessionController {
            let env = EcholetKeyboardSessionController.Environment(
                defaults: defaults,
                nowMs: { [weak self] in self?.now ?? 0 },
                currentAppEpoch: { [weak self] in self?.appEpoch },
                openHost: { [weak self] url in
                    guard let self = self else { return .failed(reason: "gone") }
                    self.openCallsRaw.append((url, false))
                    return self.nextOpenResult
                },
                insertText: { [weak self] text in
                    guard let self = self, !text.isEmpty else { return false }
                    self.insertedTexts.append(text)
                    return true
                },
                deleteBackward: { [weak self] count in
                    guard let self = self else { return }
                    self.deleteBackwardCounts.append(count)
                },
                docFingerprint: { [weak self] in self?.fingerprint ?? (nil, nil, nil) },
                log: { [weak self] event in self?.logs.append(event) }
            )
            return EcholetKeyboardSessionController(environment: env)
        }

        @discardableResult
        func writeResponse(
            epoch: String? = nil,
            sessionId: String,
            ackReq: String,
            ackSeq: UInt64,
            rev: UInt64,
            state: EcholetIPC.AppState,
            text: String? = nil,
            isFinal: Bool = false,
            errorCode: String? = nil
        ) -> Bool {
            let response = try? EcholetIPC.AppResponse(
                appEpoch: epoch ?? appEpoch,
                sessionId: sessionId,
                acknowledgedRequestId: ackReq,
                acknowledgedSequence: ackSeq,
                revision: rev,
                state: state,
                recognizedText: text,
                isFinal: isFinal,
                errorCode: errorCode
            )
            guard let response = response, let data = try? EcholetIPC.makeEncoder().encode(response) else { return false }
            defaults.set(data, forKey: EcholetIPC.appResponseKey)
            defaults.synchronize()
            return true
        }

        func lastRequest() -> EcholetIPC.KeyboardRequest? {
            guard let data = defaults.data(forKey: EcholetIPC.keyboardRequestKey) else { return nil }
            return try? EcholetIPC.makeDecoder().decode(EcholetIPC.KeyboardRequest.self, from: data)
        }

        func sentRequestCount() -> Int {
            return Int((defaults.object(forKey: EcholetKeyboardSessionController.intentSequenceStorageKey) as? UInt64) ?? 0)
        }

        func writeWarmLive(epoch: String, active: Bool, tsMs: UInt64) {
            _ = EcholetLaunch.writeWarmLive(appEpoch: epoch, recorderActive: active, nowMs: tsMs, defaults: defaults)
        }

        func writePhase(nonce: String, phase: EcholetLaunch.Phase, tsMs: UInt64) {
            _ = EcholetLaunch.writePhase(
                nonce: nonce,
                appEpoch: appEpoch,
                phase: phase,
                nowMs: tsMs,
                defaults: defaults
            )
        }

        func warmReadyPhaseNonce() -> String? {
            let last = openCallsRaw.last?.url
            guard let url = last, let parsed = EcholetLaunch.parseDictationPrepareURL(url) else { return nil }
            return parsed.nonce
        }
    }

    // MARK: - Tests

    static func run() {
        testAppearWithNoWarmAutoOpenOnce()
        testFreshWarmLiveSkipsOpenAndStartsDirectly()
        testTappedStartAfterUnsupportedOpen()
        testRecentWarmReadyBlocksAutoReopen()
        testColdDeadAppStaleWarmLiveOpensApp()
        testStartAckGatesRedStop()
        testStopExactAckThenInsertOnce()
        testInsertOnceUnicode()
        testEditorMismatchFailsClosed()
        testOtherSessionAndStaleResponsesIgnored()
        testFocusChangeCancelsSession()
        testInsertChurnDoesNotCancelSession()
        testHideCancelsSession()
        testRapidTapsDeduplicated()
        testWarmReadyEvidenceAutoStartsOnce()
        testPermissionFailureBlockedResponse()
        testOpenTimeoutUnsupported()
        testStartAckTimeout()
        testLivePartialStreaming()
        testFinalReconcilesStreamedPartial()
        testNoSpeechFinalRemovesPartial()
        testPreparedDemandURLMatchesDemandRecord()
        testGuardedPartialDeletionOnCursorMismatch()
        testColdProcessRecoveryNewEpoch()
        testPreparedDemandURLRotatesNearExpiry()

        if failures.isEmpty {
            print("EcholetKeyboardSessionTests: ALL PASS")
        } else {
            print("EcholetKeyboardSessionTests: \(failures.count) FAILURES")
        }
    }

    static func testAppearWithNoWarmAutoOpenOnce() {
        let h = FakeHarness()
        let c = h.controller()
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testAppearWithNoWarmAutoOpenOnce(1)")
        // Second appearance event in the SAME appearance window dedupes.
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testAppearWithNoWarmAutoOpenOnce(2)")
        // An in-flight launch context resumes (OPEN_RESUMED), never re-opens.
        c.handleDisappeared()
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testAppearWithNoWarmAutoOpenOnce(3)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.opening, "testAppearWithNoWarmAutoOpenOnce(4)")
        // Once the demand epoch expires (>45s), a fresh appearance may
        // measure exactly one new attempt.
        c.handleDisappeared()
        h.now += EcholetLaunch.defaultDemandTtlMs + 1
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 2, "testAppearWithNoWarmAutoOpenOnce(5)")
        // Demand record written for the launch attempt.
        let demand = EcholetLaunch.readDemand(defaults: h.defaults)
        expectTrue(demand != nil, "testAppearWithNoWarmAutoOpenOnce(6)")
        // Nonce in URL matches the demand record.
        if let demand = demand, let url = h.openCallsRaw.last?.url,
           let parsed = EcholetLaunch.parseDictationPrepareURL(url) {
            expectEqual(parsed.nonce, demand.nonce, "testAppearWithNoWarmAutoOpenOnce(7)")
        } else {
            recordFailure("testAppearWithNoWarmAutoOpenOnce(7)", "missing demand/url")
        }
    }

    static func testFreshWarmLiveSkipsOpenAndStartsDirectly() {
        let h = FakeHarness()
        let c = h.controller()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 0, "testFreshWarmLiveSkipsOpenAndStartsDirectly(1)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.starting, "testFreshWarmLiveSkipsOpenAndStartsDirectly(2)")
        let req = h.lastRequest()
        expectEqual(req?.command, EcholetIPC.Command.start, "testFreshWarmLiveSkipsOpenAndStartsDirectly(3)")
        expectEqual(req?.sequence, UInt64(1), "testFreshWarmLiveSkipsOpenAndStartsDirectly(4)")
    }

    static func testTappedStartAfterUnsupportedOpen() {
        let h = FakeHarness()
        h.nextOpenResult = .unsupported(reason: "no_public_open_api")
        let c = h.controller()
        c.handleAppeared()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.ready, "testTappedStartAfterUnsupportedOpen(1)")
        expectEqual(h.openCallsRaw.count, 1, "testTappedStartAfterUnsupportedOpen(2)")
        expectTrue(c.autoLaunchUnsupportedReason?.contains("AUTO_LAUNCH_UNSUPPORTED") == true || c.autoLaunchUnsupportedReason != nil, "testTappedStartAfterUnsupportedOpen(3)")
        // User tap re-attempts the open (tapped attempts allowed); a
        // synchronously unsupported open resolves back to ready honestly.
        c.handlePrimaryButtonTap()
        expectEqual(h.openCallsRaw.count, 2, "testTappedStartAfterUnsupportedOpen(4)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.ready, "testTappedStartAfterUnsupportedOpen(5)")

        // While the open is pending (initiated), the state is opening.
        h.nextOpenResult = .initiated
        c.handlePrimaryButtonTap()
        expectEqual(h.openCallsRaw.count, 3, "testTappedStartAfterUnsupportedOpen(6)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.opening, "testTappedStartAfterUnsupportedOpen(7)")
    }

    static func testRecentWarmReadyBlocksAutoReopen() {
        let h = FakeHarness()
        let c = h.controller()
        h.writePhase(nonce: "nonce-previous-1", phase: .warmReady, tsMs: h.now - 1000)
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 0, "testRecentWarmReadyBlocksAutoReopen(1)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.ready, "testRecentWarmReadyBlocksAutoReopen(2)")
        // Tapped attempt still permitted.
        c.handlePrimaryButtonTap()
        expectEqual(h.openCallsRaw.count, 1, "testRecentWarmReadyBlocksAutoReopen(3)")
    }

    static func testColdDeadAppStaleWarmLiveOpensApp() {
        let h = FakeHarness()
        let c = h.controller()
        // Old process epoch + aged recorder record: NOT a fresh warm mic.
        h.writeWarmLive(epoch: "epoch-dead", active: true, tsMs: h.now - 60_000)
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testColdDeadAppStaleWarmLiveOpensApp(1)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.opening, "testColdDeadAppStaleWarmLiveOpensApp(2)")
    }

    static func testStartAckGatesRedStop() {
        let h = FakeHarness()
        let c = h.controller()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        c.handleAppeared() // direct START
        expectEqual(c.shouldShowRedStopButton, false, "testStartAckGatesRedStop(1)")
        expectTrue(c.phase == .starting, "testStartAckGatesRedStop(2)")
        guard let req = h.lastRequest() else {
            recordFailure("testStartAckGatesRedStop", "no start request")
            return
        }
        // Response with .preparing must NOT flip to listening.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .preparing)
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.starting, "testStartAckGatesRedStop(3)")
        // Wrong-epoch listening ack ignored (never show red).
        h.writeResponse(epoch: "epoch-dead", sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 2, state: .listening)
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.starting, "testStartAckGatesRedStop(4)")
        // Real listening ack flips state.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 3, state: .listening)
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.listening, "testStartAckGatesRedStop(5)")
        expectEqual(c.shouldShowRedStopButton, true, "testStartAckGatesRedStop(6)")
    }

    static func testStopExactAckThenInsertOnce() {
        let h = FakeHarness()
        h.now = 1_000_000
        let c = h.controller()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        c.handleAppeared()
        guard let startReq = h.lastRequest() else {
            recordFailure("testStopExactAckThenInsertOnce", "no start request")
            return
        }
        h.writeResponse(sessionId: startReq.sessionId, ackReq: startReq.requestId, ackSeq: startReq.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.listening, "testStopExactAckThenInsertOnce(1)")

        // Tap stop BEFORE any listening ack is impossible (state gate); here it is listening.
        c.handleStopTap()
        guard let stopReq = h.lastRequest() else {
            recordFailure("testStopExactAckThenInsertOnce(2)", "no stop request")
            return
        }
        expectEqual(stopReq.command, EcholetIPC.Command.stop, "testStopExactAckThenInsertOnce(3)")
        expectEqual(stopReq.sessionId, startReq.sessionId, "testStopExactAckThenInsertOnce(4)")
        expectEqual(stopReq.sequence, startReq.sequence + 1, "testStopExactAckThenInsertOnce(5)")
        expectTrue(stopReq.requestId != startReq.requestId, "testStopExactAckThenInsertOnce(6)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.finalizing, "testStopExactAckThenInsertOnce(7)")

        // Final transcript with WRONG ack identity must be ignored.
        h.writeResponse(sessionId: startReq.sessionId, ackReq: startReq.requestId, ackSeq: startReq.sequence, rev: 2, state: .completed, text: "WRONG", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts.count, 0, "testStopExactAckThenInsertOnce(8)")

        // Correct correlated final transcript inserts exactly once.
        // Note: h.makeFingerprint() churns identity; fingerprint stays stable instead.
        h.writeResponse(sessionId: stopReq.sessionId, ackReq: stopReq.requestId, ackSeq: stopReq.sequence, rev: 3, state: .completed, text: "hello recap test", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts, ["hello recap test"], "testStopExactAckThenInsertOnce(9)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.ready, "testStopExactAckThenInsertOnce(10)")
        expectEqual(c.insertCount, 1, "testStopExactAckThenInsertOnce(11)")
        // Replaying the SAME response (stale revision) can never re-insert.
        c.consumePendingResponse()
        expectEqual(c.insertCount, 1, "testStopExactAckThenInsertOnce(12)")
        // Re-applying same ACCEPTED-identity higher revision completes without new text unless app re-acknowledges.
        h.writeResponse(sessionId: stopReq.sessionId, ackReq: stopReq.requestId, ackSeq: stopReq.sequence, rev: 4, state: .completed, text: "hello recap test", isFinal: true)
        c.handleTimerTick()
        expectEqual(c.insertCount, 1, "testStopExactAckThenInsertOnce(13)")
    }

    static func testInsertOnceUnicode() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testInsertOnceUnicode", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        c.handleStopTap()
        c.consumePendingResponse() // no response yet; finalizing stands
        let unicode = "你好世界 mixed 中英 hello — 世界！ Привет"
        guard let stopReq = h.lastRequest() else {
            recordFailure("testInsertOnceUnicode(2)", "no stop request")
            return
        }
        h.writeResponse(sessionId: stopReq.sessionId, ackReq: stopReq.requestId, ackSeq: stopReq.sequence, rev: 2, state: .completed, text: unicode, isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts, [unicode], "testInsertOnceUnicode(3)")
        expectEqual(h.insertedTexts.first, "你好世界 mixed 中英 hello — 世界！ Привет", "testInsertOnceUnicode(4)")
    }

    static func testEditorMismatchFailsClosed() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testEditorMismatchFailsClosed", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        // Editor identity changes between the listening ack and the insert.
        _ = h.makeFingerprint()
        guard let stopReq = h.lastRequest() else {
            recordFailure("testEditorMismatchFailsClosed(2)", "no stop request")
            return
        }
        h.writeResponse(sessionId: stopReq.sessionId, ackReq: stopReq.requestId, ackSeq: stopReq.sequence, rev: 2, state: .completed, text: "never inserted", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts.count, 0, "testEditorMismatchFailsClosed(3)")
        if case .blocked = c.phase {} else {
            recordFailure("testEditorMismatchFailsClosed(4)", "expected blocked phase, got \(c.phase)")
        }
    }

    static func testOtherSessionAndStaleResponsesIgnored() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testOtherSessionAndStaleResponsesIgnored", "no start request")
            return
        }
        // Other-session response ignored.
        h.writeResponse(sessionId: "other-session", ackReq: "other-req", ackSeq: 1, rev: 99, state: .listening)
        c.handleTimerTick()
        expectTrue(c.phase == .starting, "testOtherSessionAndStaleResponsesIgnored(1)")
        // Ack identity mismatch ignored.
        h.writeResponse(sessionId: req.sessionId, ackReq: "unknown-req", ackSeq: 42, rev: 100, state: .completed, text: "bad", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts.count, 0, "testOtherSessionAndStaleResponsesIgnored(2)")
        // Never observed revision replay ignored.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 0, state: .listening)
        c.handleTimerTick()
        expectTrue(c.phase == .starting, "testOtherSessionAndStaleResponsesIgnored(3)")
        // Real listening ack still works.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 5, state: .listening)
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.listening, "testOtherSessionAndStaleResponsesIgnored(4)")
    }

    static func testFocusChangeCancelsSession() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testFocusChangeCancelsSession", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.listening, "testFocusChangeCancelsSession(1)")
        // Real editor change: cancel + CANCEL command written.
        _ = h.makeFingerprint()
        c.noteTextWillChange()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.blocked(reason: "Editor focus changed; session cancelled."), "testFocusChangeCancelsSession(2)")
        guard let cancelReq = h.lastRequest() else {
            recordFailure("testFocusChangeCancelsSession(3)", "no cancel request")
            return
        }
        expectEqual(cancelReq.command, EcholetIPC.Command.cancel, "testFocusChangeCancelsSession(4)")
    }

    static func testInsertChurnDoesNotCancelSession() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testInsertChurnDoesNotCancelSession", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        // Same document identity textWillChange (our own insertion churn) is ignored.
        c.noteTextWillChange()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.listening, "testInsertChurnDoesNotCancelSession(1)")
        expectTrue(h.sentRequestCount() >= 1, "testInsertChurnDoesNotCancelSession(2)")
        // Complete the session with an insert; no cancel was sent.
        c.handleStopTap()
        guard let stopReq = h.lastRequest() else {
            recordFailure("testInsertChurnDoesNotCancelSession(3)", "no stop request")
            return
        }
        h.writeResponse(sessionId: stopReq.sessionId, ackReq: stopReq.requestId, ackSeq: stopReq.sequence, rev: 2, state: .completed, text: "kept session", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts, ["kept session"], "testInsertChurnDoesNotCancelSession(4)")
    }

    static func testHideCancelsSession() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testHideCancelsSession", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        c.handleDisappeared()
        guard let cancelReq = h.lastRequest() else {
            recordFailure("testHideCancelsSession(2)", "no cancel request")
            return
        }
        expectEqual(cancelReq.command, EcholetIPC.Command.cancel, "testHideCancelsSession(3)")
        expectEqual(cancelReq.sessionId, req.sessionId, "testHideCancelsSession(4)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.ready, "testHideCancelsSession(5)")
        // A final response for the cancelled session must NOT insert.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 2, state: .completed, text: "after hide", isFinal: true)
        c.consumePendingResponse()
        expectEqual(h.insertedTexts.count, 0, "testHideCancelsSession(6)")
    }

    static func testRapidTapsDeduplicated() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        expectTrue(h.sentRequestCount() >= 1, "testRapidTapsDeduplicated(0)")
        // While starting, extra START taps are state-gated.
        c.handlePrimaryButtonTap()
        expectTrue(c.phase == .starting, "testRapidTapsDeduplicated(1)")
        // Close (this writes a CANCEL with intent+1), then re-tap within the
        // 800ms guard: the rapid tap mints NO new request.
        h.now += 100
        c.cancelActiveSession(reason: "test")
        let preTapIntent = h.sentRequestCount()
        guard let cancelReq = h.lastRequest(), cancelReq.command == .cancel else {
            recordFailure("testRapidTapsDeduplicated(1b)", "expected cancel request")
            return
        }
        c.handlePrimaryButtonTap()
        expectEqual(h.sentRequestCount(), preTapIntent, "testRapidTapsDeduplicated(2)")
        expectEqual(h.lastRequest()!.command, EcholetIPC.Command.cancel, "testRapidTapsDeduplicated(3)")
        // After the guard window a tap produces a fresh START session.
        h.now += 900
        c.handlePrimaryButtonTap()
        let req2 = h.lastRequest()!
        expectEqual(req2.command, EcholetIPC.Command.start, "testRapidTapsDeduplicated(4)")
        expectTrue(req2.intentSequence > cancelReq.intentSequence, "testRapidTapsDeduplicated(5)")
    }

    static func testWarmReadyEvidenceAutoStartsOnce() {
        let h = FakeHarness()
        let c = h.controller()
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testWarmReadyEvidenceAutoStartsOnce(1)")
        let nonce = h.warmReadyPhaseNonce()
        expectTrue(nonce != nil, "testWarmReadyEvidenceAutoStartsOnce(2)")
        // Progress phases do not start (only warmReady evidence does).
        h.now += 100
        h.writePhase(nonce: nonce!, phase: .uriReceived, tsMs: h.now)
        c.handleTimerTick()
        expectTrue(h.lastRequest() == nil, "testWarmReadyEvidenceAutoStartsOnce(3)")
        h.now += 100
        h.writePhase(nonce: nonce!, phase: .modelLoaded, tsMs: h.now)
        c.handleTimerTick()
        expectTrue(h.lastRequest() == nil, "testWarmReadyEvidenceAutoStartsOnce(4)")
        // warmReady evidence: exactly one START for the open-loop session.
        h.now += 100
        h.writePhase(nonce: nonce!, phase: .warmReady, tsMs: h.now)
        c.handleTimerTick()
        let req = h.lastRequest()
        expectEqual(req?.command, EcholetIPC.Command.start, "testWarmReadyEvidenceAutoStartsOnce(5)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.starting, "testWarmReadyEvidenceAutoStartsOnce(6)")
        // Additional warmReady observation cannot mint a second session.
        h.now += 100
        h.writePhase(nonce: nonce!, phase: .warmReady, tsMs: h.now)
        c.handleTimerTick()
        expectEqual(h.sentRequestCount(), Int(req!.intentSequence), "testWarmReadyEvidenceAutoStartsOnce(7)")
    }

    static func testPermissionFailureBlockedResponse() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testPermissionFailureBlockedResponse", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .blocked, isFinal: true, errorCode: "microphone_permission_denied")
        c.handleTimerTick()
        if case .blocked(let reason) = c.phase {
            expectTrue(reason.contains("microphone_permission_denied"), "testPermissionFailureBlockedResponse(1)")
        } else {
            recordFailure("testPermissionFailureBlockedResponse(2)", "expected blocked phase, got \(c.phase)")
        }
    }

    static func testLivePartialStreaming() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testLivePartialStreaming", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        expectTrue(c.phase == .listening, "testLivePartialStreaming(1)")

        // Cumulative partials stream in real time, rewriting only tails.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 2, state: .processing, text: "你")
        c.handleTimerTick()
        expectEqual(h.insertedTexts, ["你"], "testLivePartialStreaming(2)")
        expectEqual(h.deleteBackwardCounts, [] as [Int], "testLivePartialStreaming(3)")

        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 3, state: .processing, text: "你好")
        c.handleTimerTick()
        expectEqual(h.insertedTexts, ["你", "好"], "testLivePartialStreaming(4)")

        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 4, state: .processing, text: "你好世")
        c.handleTimerTick()
        expectEqual(h.deleteBackwardCounts, [] as [Int], "testLivePartialStreaming(5)")

        // ASR may diverge: "你好啊" replaces "你好世" (1 deletion + 1 insertion).
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 5, state: .processing, text: "你好啊")
        c.handleTimerTick()
        expectEqual(h.deleteBackwardCounts.last, 1, "testLivePartialStreaming(6)")
        expectEqual(h.insertedTexts.last, "啊", "testLivePartialStreaming(7)")

        // Partial state stays listening (red Stop remains).
        expectTrue(c.phase == .listening, "testLivePartialStreaming(8)")

        // No speech final with only partial: partial removed.
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 6, state: .completed, text: "", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.deleteBackwardCounts.last, 3, "testLivePartialStreaming(9)")
        expectTrue(c.phase == .ready, "testLivePartialStreaming(10)")
    }

    static func testFinalReconcilesStreamedPartial() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        guard let req = h.lastRequest() else {
            recordFailure("testFinalReconcilesStreamedPartial", "no start request")
            return
        }
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 2, state: .processing, text: "hello wor")
        c.handleTimerTick()
        c.handleStopTap()
        guard let stopReq = h.lastRequest() else {
            recordFailure("testFinalReconcilesStreamedPartial(2)", "no stop request")
            return
        }
        // Final extends the streamed partial: only the tail "ld!" is added.
        h.writeResponse(sessionId: stopReq.sessionId, ackReq: stopReq.requestId, ackSeq: stopReq.sequence, rev: 3, state: .completed, text: "hello world!", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.insertedTexts, ["hello wor", "ld!"], "testFinalReconcilesStreamedPartial(3)")
        expectEqual(h.deleteBackwardCounts, [] as [Int], "testFinalReconcilesStreamedPartial(4)")
        expectTrue(c.phase == .ready, "testFinalReconcilesStreamedPartial(5)")
        // Replay cannot duplicate.
        c.consumePendingResponse()
        expectEqual(h.insertedTexts.count, 2, "testFinalReconcilesStreamedPartial(6)")
        // Full streamed-and-identical final: nothing appended.
        let h2 = FakeHarness()
        h2.writeWarmLive(epoch: h2.appEpoch, active: true, tsMs: h2.now)
        let c2 = h2.controller()
        c2.handleAppeared()
        let req2 = h2.lastRequest()!
        h2.writeResponse(sessionId: req2.sessionId, ackReq: req2.requestId, ackSeq: req2.sequence, rev: 1, state: .listening)
        c2.handleTimerTick()
        h2.writeResponse(sessionId: req2.sessionId, ackReq: req2.requestId, ackSeq: req2.sequence, rev: 2, state: .processing, text: "你好世界")
        c2.handleTimerTick()
        c2.handleStopTap()
        let stopReq2 = h2.lastRequest()!
        h2.writeResponse(sessionId: stopReq2.sessionId, ackReq: stopReq2.requestId, ackSeq: stopReq2.sequence, rev: 3, state: .completed, text: "你好世界", isFinal: true)
        c2.handleTimerTick()
        expectEqual(h2.insertedTexts, ["你好世界"], "testFinalReconcilesStreamedPartial(7)")
        expectEqual(h2.deleteBackwardCounts, [] as [Int], "testFinalReconcilesStreamedPartial(8)")
    }

    static func testNoSpeechFinalRemovesPartial() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        let req = h.lastRequest()!
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 2, state: .processing, text: "abc")
        c.handleTimerTick()
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 3, state: .completed, text: "", isFinal: true)
        c.handleTimerTick()
        expectEqual(h.deleteBackwardCounts.last, 3, "testNoSpeechFinalRemovesPartial(1)")
        expectTrue(c.phase == .ready, "testNoSpeechFinalRemovesPartial(2)")
    }

    static func testOpenTimeoutUnsupported() {
        let h = FakeHarness()
        let c = h.controller()
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testOpenTimeoutUnsupported(1)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.opening, "testOpenTimeoutUnsupported(2)")
        h.now += EcholetKeyboardSessionController.openEvidenceTimeoutMs + 1
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.ready, "testOpenTimeoutUnsupported(3)")
        expectTrue(c.autoLaunchUnsupportedReason?.contains("AUTO_LAUNCH_UNSUPPORTED") == true, "testOpenTimeoutUnsupported(4)")
    }

    static func testStartAckTimeout() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        expectTrue(c.phase == .starting, "testStartAckTimeout(1)")
        h.now += EcholetKeyboardSessionController.startAckTimeoutMs + 1
        c.handleTimerTick()
        if case .blocked(let reason) = c.phase {
            expectTrue(reason.contains("not acknowledged"), "testStartAckTimeout(2)")
        } else {
            recordFailure("testStartAckTimeout(3)", "expected blocked phase, got \(c.phase)")
        }
        // Session was closed truthfully (fail closed) and is startable again.
        expectEqual(c.isPrimaryButtonInteractive, true, "testStartAckTimeout(4)")
    }

    static func testPreparedDemandURLMatchesDemandRecord() {
        let h = FakeHarness()
        let c = h.controller()
        // Ensure prepared demand URL is minted synchronously
        let url = c.ensurePreparedDemandURL()
        expectTrue(url != nil, "testPreparedDemandURLMatchesDemandRecord(1)")
        expectEqual(c.preparedDemandURL, url, "testPreparedDemandURLMatchesDemandRecord(2)")

        let demand = EcholetLaunch.readDemand(defaults: h.defaults)
        expectTrue(demand != nil, "testPreparedDemandURLMatchesDemandRecord(3)")
        if let demand = demand, let url = url, let parsed = EcholetLaunch.parseDictationPrepareURL(url) {
            expectEqual(parsed.nonce, demand.nonce, "testPreparedDemandURLMatchesDemandRecord(4)")
        } else {
            recordFailure("testPreparedDemandURLMatchesDemandRecord(4)", "missing demand/url parsed")
        }

        // When warm live mic exists, appear clears preparedDemandURL and adopts directly
        let h2 = FakeHarness()
        h2.writeWarmLive(epoch: h2.appEpoch, active: true, tsMs: h2.now)
        let c2 = h2.controller()
        c2.handleAppeared()
        expectTrue(c2.preparedDemandURL == nil, "testPreparedDemandURLMatchesDemandRecord(5)")
        expectEqual(c2.phase, EcholetKeyboardSessionController.Phase.starting, "testPreparedDemandURLMatchesDemandRecord(6)")
    }

    static func testGuardedPartialDeletionOnCursorMismatch() {
        let h = FakeHarness()
        h.writeWarmLive(epoch: h.appEpoch, active: true, tsMs: h.now)
        let c = h.controller()
        c.handleAppeared()
        let req = h.lastRequest()!
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 1, state: .listening)
        c.handleTimerTick()

        // Provide docContext before input matching initial partial
        h.fingerprints.append(("foo", nil, nil))
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 2, state: .processing, text: "foo")
        c.handleTimerTick()
        expectEqual(h.insertedTexts, ["foo"], "testGuardedPartialDeletionOnCursorMismatch(1)")

        // Simulate divergent partial: previous "foo", next "bar" -> expects to delete 3 ("foo")
        // But simulate cursor moved elsewhere: docContext before is now "unrelated text" (does NOT end with "foo")
        h.fingerprints.append(("unrelated text", nil, nil))
        h.writeResponse(sessionId: req.sessionId, ackReq: req.requestId, ackSeq: req.sequence, rev: 3, state: .processing, text: "bar")
        c.handleTimerTick()

        // deleteBackward should be GUARDED and NOT called!
        expectEqual(h.deleteBackwardCounts, [] as [Int], "testGuardedPartialDeletionOnCursorMismatch(2)")
        expectTrue(h.logs.contains("STREAM_DELETE_GUARDED(tail_mismatch_or_cursor_moved)"), "testGuardedPartialDeletionOnCursorMismatch(3)")
    }

    static func testColdProcessRecoveryNewEpoch() {
        let h = FakeHarness()
        h.appEpoch = "epoch-1-old"
        // Simulate app was recording before death
        h.writeWarmLive(epoch: "epoch-1-old", active: true, tsMs: h.now)

        // Process killed! New process epoch published:
        h.appEpoch = "epoch-2-new"
        // Old warm-live record still in defaults with epoch-1-old:
        let c = h.controller()
        // Controller must recognize old warm record is stale due to epoch mismatch
        expectTrue(!c.freshWarmLiveExists(), "testColdProcessRecoveryNewEpoch(1)")

        // Appearance should trigger fresh open attempt
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 1, "testColdProcessRecoveryNewEpoch(2)")
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.opening, "testColdProcessRecoveryNewEpoch(3)")

        // Newly launched app publishes warm-ready phase for the demand nonce under epoch-2-new
        guard let url = h.openCallsRaw.last?.url,
              let parsed = EcholetLaunch.parseDictationPrepareURL(url) else {
            recordFailure("testColdProcessRecoveryNewEpoch(4)", "missing parsed URL")
            return
        }
        _ = EcholetLaunch.writePhase(
            nonce: parsed.nonce,
            appEpoch: "epoch-2-new",
            phase: .warmReady,
            nowMs: h.now + 500,
            defaults: h.defaults
        )
        // Also updates fresh warm live
        _ = EcholetLaunch.writeWarmLive(
            appEpoch: "epoch-2-new",
            recorderActive: true,
            nowMs: h.now + 500,
            defaults: h.defaults
        )

        // Advance clock so controller sees record is not from future
        h.now += 500

        // Tick resolves warmReady evidence and issues v2 START with new app epoch
        c.handleTimerTick()
        expectEqual(c.phase, EcholetKeyboardSessionController.Phase.starting, "testColdProcessRecoveryNewEpoch(5)")
        let req = h.lastRequest()!
        expectEqual(req.appEpoch, "epoch-2-new", "testColdProcessRecoveryNewEpoch(6)")
    }

    static func testPreparedDemandURLRotatesNearExpiry() {
        let h = FakeHarness()
        let c = h.controller()
        let url1 = c.ensurePreparedDemandURL()!
        let demand1 = EcholetLaunch.readDemand(defaults: h.defaults)!

        // Advance time to near expiry (within 10s of TTL)
        h.now += (EcholetLaunch.defaultDemandTtlMs - 5_000)
        let url2 = c.ensurePreparedDemandURL()!
        let demand2 = EcholetLaunch.readDemand(defaults: h.defaults)!

        // Demand and URL must have rotated with fresh nonce!
        expectTrue(url1 != url2, "testPreparedDemandURLRotatesNearExpiry(1)")
        expectTrue(demand1.nonce != demand2.nonce, "testPreparedDemandURLRotatesNearExpiry(2)")
        let parsed = EcholetLaunch.parseDictationPrepareURL(url2)!
        expectEqual(parsed.nonce, demand2.nonce, "testPreparedDemandURLRotatesNearExpiry(3)")
    }
}

@main
struct EcholetKeyboardSessionTestsMain {
    static func main() {
        EcholetKeyboardSessionTests.run()
        exit(EcholetKeyboardSessionTests.failures.isEmpty ? 0 : 1)
    }
}
