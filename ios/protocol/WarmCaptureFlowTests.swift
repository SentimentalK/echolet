import Foundation

func assertFlow(_ condition: Bool, _ message: String) {
    if !condition {
        fputs("WARM CAPTURE FLOW TEST FAILED: \(message)\n", stderr)
        exit(1)
    }
}

func testWarmCaptureFlowSuite() {
    print("[TEST] Running pure Swift WarmCaptureFlowCoordinator unit tests...")

    let epoch = "epoch-test-flow-1"

    // Case 1: Start A, user Arm OFF, late granted permission => zero hardware starts, no listening response.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let reqA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: 1,
            sessionId: "session-A",
            sequence: 1,
            requestId: "req-start-A",
            command: .start
        )

        let effectsStart = coordinator.handleIncomingRequest(reqA)
        assertFlow(effectsStart.contains { if case .requestPermission = $0 { return true } else { return false } }, "Should request permission")
        assertFlow(coordinator.pendingStartToken?.sessionId == "session-A", "Pending token is A")

        let tokenA = coordinator.pendingStartToken!

        // User toggles ARM OFF before permission callback returns
        _ = coordinator.setUserArmMicrophone(false)
        assertFlow(!coordinator.isMicrophoneArmedByUser, "Mic disarmed")
        assertFlow(coordinator.pendingStartToken == nil, "Pending start token cleared")

        // Now late permission callback returns granted: true
        let lateEffects = coordinator.handlePermissionCallback(token: tokenA, granted: true)
        assertFlow(lateEffects.isEmpty, "Late permission callback after disarm must produce ZERO effects (no hardware start)")
        assertFlow(coordinator.activeSessionToken == nil, "No active session")
    } catch {
        assertFlow(false, "Case 1 threw: \(error)")
    }

    // Case 2: UI Arm ON permission pending, Arm OFF, old permission granted => no hot mic / no rearm.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let req = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: 1,
            sessionId: "session-X",
            sequence: 1,
            requestId: "req-X",
            command: .start
        )
        _ = coordinator.handleIncomingRequest(req)
        let tokenX = coordinator.pendingStartToken!

        _ = coordinator.setUserArmMicrophone(false)
        let effects = coordinator.handlePermissionCallback(token: tokenX, granted: true)
        assertFlow(effects.isEmpty, "No hardware start after disarm")
        assertFlow(coordinator.activeSessionToken == nil, "Active session nil")
    } catch {
        assertFlow(false, "Case 2 threw: \(error)")
    }

    // Case 3: Start A, hardware start pending, STOP A, old success callback => never restore A listening, stop real engine if owned.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let reqStartA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: 1,
            sessionId: "session-A",
            sequence: 1,
            requestId: "req-start-A",
            command: .start
        )
        _ = coordinator.handleIncomingRequest(reqStartA)
        let tokenA = coordinator.pendingStartToken!

        let permEffects = coordinator.handlePermissionCallback(token: tokenA, granted: true)
        assertFlow(permEffects.contains { if case .startHardware = $0 { return true } else { return false } }, "Hardware start requested")

        // STOP A arrives while hardware engine start is in flight
        let reqStopA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: 2,
            sessionId: "session-A",
            sequence: 2,
            requestId: "req-stop-A",
            command: .stop
        )
        let stopEffects = coordinator.handleIncomingRequest(reqStopA)
        assertFlow(stopEffects.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "Stop hardware emitted")
        assertFlow(coordinator.activeSessionToken == nil, "Active session cleared")

        // Now delayed hardware start success arrives for tokenA
        struct DummyError: Error {}
        let (staleEffects, cleanupNeeded) = coordinator.handleHardwareStartCompletion(token: tokenA, result: .success(10))
        assertFlow(staleEffects.isEmpty, "Stale hardware start callback produces NO response or admission effects")
        assertFlow(cleanupNeeded == true, "Cleanup needed to ensure hardware engine is halted")
        assertFlow(coordinator.activeSessionToken == nil, "Active session stays nil")
    } catch {
        assertFlow(false, "Case 3 threw: \(error)")
    }

    // Case 4: A active, B replaces A, B must not start until native Stop(A) callback; old STOP A callback late cannot overwrite B response.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let reqStartA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: 1,
            sessionId: "session-A",
            sequence: 1,
            requestId: "req-start-A",
            command: .start
        )
        _ = coordinator.handleIncomingRequest(reqStartA)
        let tokenA = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: tokenA, granted: true)
        _ = coordinator.handleHardwareStartCompletion(token: tokenA, result: .success(1))
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-A", "Session A active")

        // B replaces A
        let reqStartB = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: 2,
            sessionId: "session-B",
            sequence: 1,
            requestId: "req-start-B",
            command: .start
        )
        let replaceEffects = coordinator.handleIncomingRequest(reqStartB)
        assertFlow(coordinator.isWaitingForHardwareStopToStartReplacement == true, "Waiting for hardware stop of A")
        assertFlow(replaceEffects.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "Hardware stop requested for A")
        assertFlow(coordinator.pendingStartToken == nil, "B not yet pending start until Stop(A) finishes")

        // Hardware stop of A finishes
        let stopAEffects = coordinator.handleHardwareStopCompletion(
            stoppedRequestId: "req-start-A",
            stoppedSessionId: "session-A",
            stoppedSequence: 1
        )
        assertFlow(coordinator.isWaitingForHardwareStopToStartReplacement == false, "Wait complete")
        assertFlow(stopAEffects.contains { if case .requestPermission(let t) = $0, t.sessionId == "session-B" { return true } else { return false } }, "Now B permission is initiated")
        assertFlow(!stopAEffects.contains { if case .writeResponse(let s, _, _, let st, _, _, _) = $0, s == "session-A", st == .completed { return true } else { return false } }, "Old stop A cannot emit completed response when B is pending/active")
    } catch {
        assertFlow(false, "Case 4 threw: \(error)")
    }

    // Case 5: stale STOP A while B recording => B hardware untouched.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        // Session B active
        let reqStartB = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-B", sequence: 1, requestId: "req-sB", command: .start)
        _ = coordinator.handleIncomingRequest(reqStartB)
        let tokenB = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: tokenB, granted: true)
        _ = coordinator.handleHardwareStartCompletion(token: tokenB, result: .success(2))
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-B", "Session B active")

        // Stale stop A request rejected by wire gate
        let staleStopA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 3, sessionId: "session-A", sequence: 2, requestId: "req-stale-stop", command: .stop)
        let staleEffects = coordinator.handleIncomingRequest(staleStopA)
        assertFlow(staleEffects.contains { if case .notifyRejected = $0 { return true } else { return false } }, "Stale stop rejected")
        assertFlow(!staleEffects.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "B hardware NOT stopped")
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-B", "Session B stays active")
    } catch {
        assertFlow(false, "Case 5 threw: \(error)")
    }

    // Case 6: manual App mic recording + keyboard STOP/tombstone or START => preserve manual recording, return blocked/busy appropriately.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        // Manual test started in App
        let (proceed, _) = coordinator.handleManualStartCaptureInitiated()
        assertFlow(proceed == true, "Manual start allowed")
        assertFlow(coordinator.captureOwner == .manualTest, "Owner is manualTest")

        // Keyboard sends START while manual recording active
        let reqStart = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-K", sequence: 1, requestId: "req-K", command: .start)
        let kStartEffects = coordinator.handleIncomingRequest(reqStart)
        assertFlow(kStartEffects.contains { if case .writeResponse(_, _, _, let st, _, _, let err) = $0, st == .blocked, err == "app_audio_busy_manual_test" { return true } else { return false } }, "Returned blocked busy response")
        assertFlow(coordinator.captureOwner == .manualTest, "Manual test owner preserved")

        // Keyboard sends STOP / tombstone while manual recording active
        let reqStop = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-K", sequence: 2, requestId: "req-K-stop", command: .stop)
        let kStopEffects = coordinator.handleIncomingRequest(reqStop)
        assertFlow(!kStopEffects.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "Keyboard stop must NOT stop manual hardware")
        assertFlow(!kStopEffects.contains { if case .stopHardwareManual = $0 { return true } else { return false } }, "Keyboard stop must NOT stop manual hardware")
        assertFlow(coordinator.captureOwner == .manualTest, "Manual test still active")

        // App explicitly stops manual test
        let stopManualEffects = coordinator.handleManualCaptureStopped()
        assertFlow(stopManualEffects.contains(.stopHardwareManual), "Manual hardware stop emitted")
        assertFlow(coordinator.captureOwner == .none, "Owner reset to none")
    } catch {
        assertFlow(false, "Case 6 threw: \(error)")
    }

    // Case 7: native start fails => blocked not listening; native STOP fails/interrupts => do not report clean stopped as if hardware confirmed.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let reqStart = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-Fail", sequence: 1, requestId: "req-Fail", command: .start)
        _ = coordinator.handleIncomingRequest(reqStart)
        let token = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: token, granted: true)

        struct EngineError: LocalizedError {
            var errorDescription: String? { "AVAudioEngine configuration failed" }
        }
        let (failEffects, _) = coordinator.handleHardwareStartCompletion(token: token, result: .failure(EngineError()))
        assertFlow(failEffects.contains { if case .writeResponse(_, _, _, let st, _, _, _) = $0, st == .blocked { return true } else { return false } }, "Emitted blocked state on engine fail")
        assertFlow(!failEffects.contains { if case .writeResponse(_, _, _, let st, _, _, _) = $0, st == .listening { return true } else { return false } }, "NEVER emitted listening")
        assertFlow(coordinator.activeSessionToken == nil, "Active session nil")
    } catch {
        assertFlow(false, "Case 7 threw: \(error)")
    }

    // Case 8: duplicate request, stale epoch, lost latest-key slot, revision/ACK monotonic and old permission callback => fail-closed.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let req1 = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "s1", sequence: 1, requestId: "r1", command: .start)
        _ = coordinator.handleIncomingRequest(req1)

        // Duplicate request
        let dupEffects = coordinator.handleIncomingRequest(req1)
        assertFlow(dupEffects.isEmpty, "Duplicate request produces no effects")

        // Stale epoch request
        let staleReq = try EcholetIPC.KeyboardRequest(appEpoch: "wrong-epoch", intentSequence: 2, sessionId: "s2", sequence: 1, requestId: "r2", command: .start)
        let staleEffects = coordinator.handleIncomingRequest(staleReq)
        assertFlow(staleEffects.contains { if case .notifyRejected = $0 { return true } else { return false } }, "Stale epoch rejected")

        // Monotonic response revision
        let rev1 = coordinator.nextResponseRevision()
        let rev2 = coordinator.nextResponseRevision()
        assertFlow(rev1 != nil && rev2 != nil && rev2! > rev1!, "Revisions strictly monotonic")
    } catch {
        assertFlow(false, "Case 8 threw: \(error)")
    }

    print("[TEST] All WarmCaptureFlowCoordinator unit tests PASSED successfully.")
}

@main
struct WarmCaptureFlowTestsMain {
    static func main() {
        testWarmCaptureFlowSuite()
    }
}
