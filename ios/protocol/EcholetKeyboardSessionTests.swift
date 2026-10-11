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
        var openResultOverride: EcholetKeyboardSessionController.OpenAttemptResult?
        var insertedTexts: [String] = []
        var fingerprints: [(before: String?, after: String?, selected: String?)] = [(nil, nil, nil)]
        var logs: [String] = []
        let defaults: UserDefaults
        var nextOpenResult: EcholetKeyboardSessionController.OpenAttemptResult = .initiated

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
        // Fresh appearance is a new window: exactly one more attempt.
        c.handleDisappeared()
        c.handleAppeared()
        expectEqual(h.openCallsRaw.count, 2, "testAppearWithNoWarmAutoOpenOnce(3)")
        // Demand record written for the launch attempt.
        let demand = EcholetLaunch.readDemand(defaults: h.defaults)
        expectTrue(demand != nil, "testAppearWithNoWarmAutoOpenOnce(4)")
        // Nonce in URL matches the demand record.
        if let demand = demand, let url = h.openCallsRaw.last?.url,
           let parsed = EcholetLaunch.parseDictationPrepareURL(url) {
            expectEqual(parsed.nonce, demand.nonce, "testAppearWithNoWarmAutoOpenOnce(5)")
        } else {
            recordFailure("testAppearWithNoWarmAutoOpenOnce(5)", "missing demand/url")
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
}

@main
struct EcholetKeyboardSessionTestsMain {
    static func main() {
        EcholetKeyboardSessionTests.run()
        exit(EcholetKeyboardSessionTests.failures.isEmpty ? 0 : 1)
    }
}
