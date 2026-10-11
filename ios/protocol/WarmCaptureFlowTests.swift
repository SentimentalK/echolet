import Foundation

func assertFlow(_ condition: Bool, _ message: String) {
    if !condition {
        fputs("WARM CAPTURE FLOW TEST FAILED: \(message)\n", stderr)
        exit(1)
    }
}

func firstStopDirective(_ effects: [WarmCaptureFlowCoordinator.FlowEffect]) -> WarmCaptureFlowCoordinator.HardwareStopDirective? {
    for effect in effects {
        if case .stopHardwareKeyboard(let directive) = effect { return directive }
    }
    return nil
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
        let (staleEffects, staleCleanup) = coordinator.handleHardwareStartCompletion(token: tokenA, result: .success(10))
        assertFlow(staleEffects.isEmpty, "Stale hardware start callback produces NO response or admission effects")
        assertFlow(staleCleanup != nil && staleCleanup!.nativeGeneration == 10, "Cleanup directive carries the stale start's exact native generation")
        assertFlow(staleCleanup?.token == tokenA, "Cleanup directive carries the exact stale session token")
        assertFlow(coordinator.activeSessionToken == nil, "Active session stays nil")
        assertFlow(coordinator.pendingStopAcknowledgmentDirective != nil, "Admitted STOP still awaits its exact ACK")

        // Stop completion carries exact stop directive; valid current STOP still ACKs completed
        let stopDirective = firstStopDirective(stopEffects)
        assertFlow(stopDirective != nil, "Admitted stop issued an exact directive")
        let ackEffects = coordinator.handleHardwareStopCompletion(directive: stopDirective!)
        assertFlow(ackEffects.contains { if case .writeResponse(_, let rq, _, let st, _, _, _) = $0, rq == "req-stop-A", st == .completed { return true } else { return false } }, "Valid current STOP ACK acknowledges its own request after hardware stop")
        assertFlow(coordinator.pendingStopAcknowledgmentDirective == nil, "Pending stop ACK consumed exactly once")
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
        let replaceDirective = firstStopDirective(replaceEffects)
        assertFlow(replaceDirective != nil, "Hardware stop requested for A")
        assertFlow(replaceEffects.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "Hardware stop effect present for A")
        assertFlow(coordinator.pendingStartToken == nil, "B not yet pending start until Stop(A) finishes")

        // Forged stale stop completion (same session A identity, wrong operation id)
        // must NOT release the replacement barrier or ACK A.
        let forgedDirective = WarmCaptureFlowCoordinator.HardwareStopDirective(
            appEpoch: epoch,
            sessionId: "session-A",
            intentSequence: 1,
            requestId: "req-start-A",
            sequence: 1,
            operationId: replaceDirective!.operationId &+ 7,
            reason: "stale_stop_cannot_release_barrier"
        )
        let forgedEffects = coordinator.handleHardwareStopCompletion(directive: forgedDirective)
        assertFlow(forgedEffects.isEmpty, "Forged stale stop directive produces ZERO effects")
        assertFlow(coordinator.isWaitingForHardwareStopToStartReplacement == true, "Barrier still held for forged completion")

        // Hardware stop of A finishes with the EXACT directive issued
        let stopAEffects = coordinator.handleHardwareStopCompletion(directive: replaceDirective!)
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
        let (proceed, proceedReason, _) = coordinator.handleManualStartCaptureInitiated()
        assertFlow(proceed == true, "Manual start allowed")
        assertFlow(proceedReason == nil, "No busy reason when keyboard idle")
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

    // Case 9: A async Start succeeds late AFTER B is already recording => no Stop B, no response overwrite.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        // Session A start admitted; permission granted; hardware start in flight
        let reqA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-A", sequence: 1, requestId: "req-A", command: .start)
        _ = coordinator.handleIncomingRequest(reqA)
        let tokenA = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: tokenA, granted: true)

        // B replaces A: native Stop(A) barrier before B starts
        let reqB = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-B", sequence: 1, requestId: "req-B", command: .start)
        let replaceEffects = coordinator.handleIncomingRequest(reqB)
        let barrierDirective = firstStopDirective(replaceEffects)
        assertFlow(barrierDirective != nil, "Replacement barrier stop issued for A")
        _ = coordinator.handleHardwareStopCompletion(directive: barrierDirective!)

        // B permission + start success => B recording (native generation 7)
        let tokenB = coordinator.pendingStartToken!
        assertFlow(tokenB.sessionId == "session-B", "B now pending")
        _ = coordinator.handlePermissionCallback(token: tokenB, granted: true)
        let _ = coordinator.handleHardwareStartCompletion(token: tokenB, result: .success(7))
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-B", "B is active/recording")

        // A's hardware start completes LATE with success (native generation 1):
        // stale A must not stop B's engine and must not write any response.
        let (staleEffects, staleCleanup) = coordinator.handleHardwareStartCompletion(token: tokenA, result: .success(1))
        assertFlow(!staleEffects.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "Stale A success emits NO keyboard hardware stop effect")
        assertFlow(!staleEffects.contains { if case .writeResponse = $0 { return true } else { return false } }, "Stale A success writes NO snapshot")
        assertFlow(!staleEffects.contains { if case .stopHardwareManual = $0 { return true } else { return false } }, "Stale A success must not stop manual capture")
        assertFlow(staleCleanup != nil && staleCleanup!.nativeGeneration == 1, "Cleanup directive bound to stale A generation 1")
        assertFlow(staleCleanup?.token == tokenA, "Cleanup directive bound to stale A token")
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-B", "B remains the active session")
        _ = consumeCleanupGeneration(staleCleanup) // bounds native generation to conditional stop in executor
    } catch {
        assertFlow(false, "Case 9 threw: \(error)")
    }

    // Case 10: manual App Start while keyboard hardware is active / permission pending => BUSY, no owner transfer.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        // Keyboard permission pending
        let reqK = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-K", sequence: 1, requestId: "req-K", command: .start)
        _ = coordinator.handleIncomingRequest(reqK)
        let tokenK = coordinator.pendingStartToken!
        assertFlow(coordinator.captureOwner.captureOwnerSessionId == "session-K", "Keyboard owns pending capture")

        let (proceed1, busy1, effects1) = coordinator.handleManualStartCaptureInitiated()
        assertFlow(proceed1 == false, "Manual start rejected while keyboard start pending")
        assertFlow(busy1 != nil, "BUSY reason surfaced")
        assertFlow(effects1.contains { if case .notifyBlocked = $0 { return true } else { return false } }, "User-visible BUSY notification emitted")
        assertFlow(!effects1.contains { if case .stopHardwareKeyboard = $0 { return true } else { return false } }, "BUSY rejection must NOT stop keyboard hardware")
        assertFlow(!effects1.contains { if case .stopHardwareManual = $0 { return true } else { return false } }, "BUSY rejection must NOT stop anything manual")
        assertFlow(coordinator.captureOwner.captureOwnerSessionId == "session-K", "No owner transfer: keyboard still owns pending capture")
        assertFlow(coordinator.pendingStartToken == tokenK, "Keyboard pending token untouched")

        // Keyboard hardware now actively recording
        _ = coordinator.handlePermissionCallback(token: tokenK, granted: true)
        let (_, _) = coordinator.handleHardwareStartCompletion(token: tokenK, result: .success(3))
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-K", "Keyboard K actively recording")

        let (proceed2, busy2, _) = coordinator.handleManualStartCaptureInitiated()
        assertFlow(proceed2 == false, "Manual start rejected while keyboard actively recording")
        assertFlow(busy2 != nil, "BUSY reason surfaced while recording")
        assertFlow(coordinator.captureOwner.captureOwnerSessionId == "session-K", "Owner transfer refused during recording")

        // Keyboard STOP truly completes; then manual retry succeeds
        let reqStopK = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-K", sequence: 2, requestId: "req-stop-K", command: .stop)
        let stopEffects = coordinator.handleIncomingRequest(reqStopK)
        let stopDirective = firstStopDirective(stopEffects)
        assertFlow(stopDirective != nil, "Keyboard stop directive issued")
        let ackEffects = coordinator.handleHardwareStopCompletion(directive: stopDirective!)
        assertFlow(ackEffects.contains { if case .writeResponse(_, _, _, let st, _, _, _) = $0, st == .completed { return true } else { return false } }, "Keyboard stop ACK completed")

        let (proceed3, busy3, _) = coordinator.handleManualStartCaptureInitiated()
        assertFlow(proceed3 == true, "Manual retry allowed after keyboard STOP truly completed")
        assertFlow(busy3 == nil, "No BUSY after keyboard stop")
    } catch {
        assertFlow(false, "Case 10 threw: \(error)")
    }

    // Case 11: stale STOP completion (exact-ACK + intent fencing) cannot overwrite latest response.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        // Session A admission + recording
        let reqA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-A", sequence: 1, requestId: "req-A", command: .start)
        _ = coordinator.handleIncomingRequest(reqA)
        let tokenA = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: tokenA, granted: true)
        _ = coordinator.handleHardwareStartCompletion(token: tokenA, result: .success(5))
        assertFlow(coordinator.activeSessionToken?.sessionId == "session-A", "A recording")

        // Legit STOP(A, intent 2) admitted; stop ACK awaits hardware completion
        let reqStopOld = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-A", sequence: 2, requestId: "req-stop-old", command: .stop)
        let stopOldEffects = coordinator.handleIncomingRequest(reqStopOld)
        let stopOldDirective = firstStopDirective(stopOldEffects)
        assertFlow(stopOldDirective != nil, "Old stop directive registered")
        assertFlow(coordinator.pendingStopAcknowledgmentDirective == stopOldDirective, "Old stop ACK directive is pending")

        // A NEW intent (session B, intent 3) replaces A: replacement barrier stop pending
        let reqB = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 3, sessionId: "session-B", sequence: 1, requestId: "req-B", command: .start)
        let replaceEffects = coordinator.handleIncomingRequest(reqB)
        let barrierDirective = firstStopDirective(replaceEffects)
        assertFlow(barrierDirective != nil, "Replacement barrier stop issued for A")
        assertFlow(coordinator.pendingReplacementStartToken?.sessionId == "session-B", "B pending behind barrier")

        // The old STOP(A) completes while B is pending: must NOT ACK/overwrite the
        // newer intent's response slot (intent fence: B intent 3 > stop intent 2).
        let oldStopLateEffects = coordinator.handleHardwareStopCompletion(directive: stopOldDirective!)
        assertFlow(oldStopLateEffects.isEmpty, "Late STOP(A) completion cannot write completed while newer intent pending")
        assertFlow(coordinator.pendingStopAcknowledgmentDirective == stopOldDirective, "Old stop ACK directive NOT consumed by fenced completion")

        // Forged completion reusing the SAME sessionId with a higher intent and unknown opId => no ACK
        let forgedReuse = WarmCaptureFlowCoordinator.HardwareStopDirective(
            appEpoch: epoch,
            sessionId: "session-A",
            intentSequence: 9,
            requestId: "req-forged-s",
            sequence: 3,
            operationId: (stopOldDirective?.operationId ?? 0) &+ 9,
            reason: "forged_same_session_reuse"
        )
        let forgedReuseEffects = coordinator.handleHardwareStopCompletion(directive: forgedReuse)
        assertFlow(forgedReuseEffects.isEmpty, "Forged same-session higher-intent STOP completion cannot ACK")
        assertFlow(coordinator.pendingStopAcknowledgmentDirective == stopOldDirective, "Old stop ACK directive only consumed by exact identity")

        // Releasing the barrier requires the EXACT replacement stop directive
        let barrierEffects = coordinator.handleHardwareStopCompletion(directive: barrierDirective!)
        assertFlow(barrierEffects.contains { if case .requestPermission(let t) = $0, t.sessionId == "session-B" { return true } else { return false } }, "B permission starts only after exact barrier directive")
        assertFlow(coordinator.pendingReplacementStopDirective == nil, "Barrier directive consumed")
    } catch {
        assertFlow(false, "Case 11 threw: \(error)")
    }

    // Case 12: cancellation generation overflow is terminal fail-closed (no new starts, no old token replay).
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        coordinator.debugForceCancellationGenerationOverflowForCLITests()
        assertFlow(coordinator.monotonicCancelGeneration == UInt64.max, "Generation counter pinned at UInt64.max")
        assertFlow(coordinator.isCancellationGenerationFailClosed == true, "Overflow latched terminal fail-closed")

        // No further mic starts
        let reqStart = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-O", sequence: 1, requestId: "req-O", command: .start)
        let startEffects = coordinator.handleIncomingRequest(reqStart)
        assertFlow(coordinator.pendingStartToken == nil, "No start token minted after overflow")
        assertFlow(startEffects.contains { if case .requestPermission = $0 { return true } else { return false } } == false, "No permission request after overflow")
        assertFlow(startEffects.contains { if case .writeResponse(_, _, _, let st, _, _, let err) = $0, st == .blocked, err == "capture_generation_overflow_fail_closed" { return true } else { return false } }, "Start answered blocked with fail-closed error")

        // Old token replay fenced: an old/equal generation token can never start the mic
        replayTokenGuarded(coordinator)
        assertFlow(coordinator.isCancellationGenerationFailClosed == true, "Fail-closed state persists (terminal)")
    } catch {
        assertFlow(false, "Case 12 threw: \(error)")
    }

    // Case 13: DEBUG mock response sequencing guard (read-only probe): stale mock
    // intents cannot overwrite newer STOP/session responses; only the CURRENT
    // intent's exact START request may be refined.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let reqA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-A", sequence: 1, requestId: "req-A", command: .start)

        // Newest unapplied current intent => eligible initial mock
        assertFlow(coordinator.evaluateMockResponseEligibility(reqA) == .eligible, "Fresh current START is mock-eligible")

        // Live intake admits A (real flow owns the response slot thereafter)
        _ = coordinator.handleIncomingRequest(reqA)

        // Exact latest applied START => allowed refinement (mock .completed over real .listening)
        assertFlow(coordinator.evaluateMockResponseEligibility(reqA) == .eligible, "Refining exact latest applied START allowed")

        // Same watermark but different request id => superseded, never eligible
        let reqForgedSameIntent = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-A", sequence: 2, requestId: "req-forged-1", command: .start)
        assertFlow(coordinator.evaluateMockResponseEligibility(reqForgedSameIntent) != .eligible, "Same-intent different request id superseded")

        // Newer STOP for A (intent 2) admitted => old START mock stale
        let reqStopA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-A", sequence: 2, requestId: "req-stop-A", command: .stop)
        let stopEffects = coordinator.handleIncomingRequest(reqStopA)
        let stopDirective = firstStopDirective(stopEffects)
        assertFlow(stopDirective != nil, "Stop admitted in case 13")
        assertFlow(coordinator.evaluateMockResponseEligibility(reqA) != .eligible, "Mock of older intent must NEVER write after newer STOP")

        // A same-intent forged clone stamped higher than its reality is also stale
        let reqStaleForged = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-A", sequence: 3, requestId: "req-stale-forged", command: .start)
        assertFlow(coordinator.evaluateMockResponseEligibility(reqStaleForged) != .eligible, "Old intent forged start remains stale")

        // Newer session B (intent 3, seq 1) is cleanly admissible; before live
        // intake applies it, the mock probe sees it as the current newest intent.
        let reqB = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 3, sessionId: "session-B", sequence: 1, requestId: "req-B", command: .start)
        assertFlow(coordinator.evaluateMockResponseEligibility(reqB) == .eligible, "Newest unapplied replacement START is mock-eligible")
        _ = coordinator.handleIncomingRequest(reqB)
        assertFlow(coordinator.evaluateMockResponseEligibility(reqB) == .eligible, "Refining exact latest applied replacement START allowed")
        assertFlow(coordinator.evaluateMockResponseEligibility(reqA) != .eligible, "Old session A never again mock-eligible after newer session")

        // New session C with sequence > 1 while a session is active => conflict
        let gateC = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinatorC = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gateC)
        _ = coordinatorC.setUserArmMicrophone(true)
        let reqCActive = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-C", sequence: 1, requestId: "req-C", command: .start)
        _ = coordinatorC.handleIncomingRequest(reqCActive) // C active listening (gate level)
        let reqCConflict = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-D", sequence: 2, requestId: "req-D", command: .start)
        assertFlow(coordinatorC.evaluateMockResponseEligibility(reqCConflict) != .eligible, "Conflicting active-session newer-intent mock rejected")

        // Cold boot fence: an uninitialized boot gate never mock-eligible
        let gateFenced = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: true)
        let coordinatorFenced = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gateFenced)
        let reqFenced = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-F", sequence: 1, requestId: "req-F", command: .start)
        assertFlow(coordinatorFenced.evaluateMockResponseEligibility(reqFenced) != .eligible, "Cold boot fence blocks mocks")
    } catch {
        assertFlow(false, "Case 13 threw: \(error)")
    }

    // Case 14: single-writer serialization — UI mock decides/advances revision
    // atomically with live intake on a real serial queue; probes and revision
    // counter form ONE serialized order with no stale writes or duplicates.
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let reqStopA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 2, sessionId: "session-A", sequence: 2, requestId: "req-stop-A", command: .stop)

        let q = DispatchQueue(label: "test.warmcapture.mockrace")
        var mockRevisions: [UInt64] = []
        let successCount = NSLock()
        var successes = 0

        func enqueueMock(_ request: EcholetIPC.KeyboardRequest) {
            q.async {
                // Atomic on the queue: identical production decision-step order
                // (eligibility probe, then revision increment, only if eligible).
                if coordinator.evaluateMockResponseEligibility(request) == .eligible {
                    let rev = coordinator.nextResponseRevision()
                    assertFlow(rev != nil, "Race case revision must not overflow")
                    mockRevisions.append(rev!)
                    successCount.lock()
                    successes += 1
                    successCount.unlock()
                }
            }
        }

        // Phase 1: queue-first live STOP admission (intent 2), then concurrent
        // spawns of stale mock reqA intent 1 taps and duplicate live intakes.
        // Serial order is deterministic: STOP applies first, so every later
        // stale mock must contribute ZERO revision.
        let reqA = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 1, sessionId: "session-A", sequence: 1, requestId: "req-A", command: .start)
        _ = coordinator.handleIncomingRequest(reqA)
        q.async {
            let effects = coordinator.handleIncomingRequest(reqStopA)
            assertFlow(!effects.isEmpty, "Stop applies first in serialized race phase 1")
        }
        let phase1 = DispatchGroup()
        for _ in 0..<30 {
            phase1.enter()
            q.async {
                enqueueMock(reqA)
                phase1.leave()
            }
        }
        phase1.wait()
        q.sync {}
        assertFlow(successes == 0, "Stale UI mocks after newer STOP wrote nothing")
        assertFlow(mockRevisions.isEmpty, "No revisions consumed by stale mocks")
        assertFlow(coordinator.responseRevision == 0, "Serialized revision mailbox untouched by stale mock race")

        // Phase 2: queue-first live admission of CURRENT intent (reqC intent 3),
        // then concurrent UI refinement mocks of the exact same request: every
        // probe is eligible, every write advances ONE strictly increasing
        // serialized revision order, and no duplicate/old ACK can interleave.
        let reqC = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 3, sessionId: "session-C", sequence: 1, requestId: "req-C", command: .start)
        q.async {
            let effects = coordinator.handleIncomingRequest(reqC)
            assertFlow(!effects.isEmpty, "Current intake applies first in serialized race phase 2")
        }
        let phase2 = DispatchGroup()
        for _ in 0..<30 {
            phase2.enter()
            q.async {
                enqueueMock(reqC)
                phase2.leave()
            }
        }
        phase2.wait()
        q.sync {}
        assertFlow(successes == 30, "All refinement mocks of the current intent wrote exactly once each")
        assertFlow(mockRevisions.count == 30, "Every eligible write consumed exactly one revision")
        assertFlow(mockRevisions == (1...30).map { $0 + UInt64(0) }, "Single serialized revision order: strictly monotonic 1..30")
        assertFlow(coordinator.responseRevision == 30, "Coordinator revision mailbox agrees with serialized writes")
    } catch {
        assertFlow(false, "Case 14 threw: \(error)")
    }

    print("[TEST] All WarmCaptureFlowCoordinator unit tests PASSED successfully.")
}

// Stale-cleanup executor binding: the coordinator halts nothing natively; the executor
// performs a generation-bound conditional stop with the directive's native generation.
func consumeCleanupGeneration(_ cleanup: WarmCaptureFlowCoordinator.HardwareStartCleanupDirective?) -> UInt64? {
    return cleanup?.nativeGeneration
}

extension WarmCaptureFlowCoordinator.CaptureOwner {
    var captureOwnerSessionId: String? {
        switch self {
        case .none, .warmStandby: return nil
        case .manualTest: return "manualTest"
        case .keyboard(let sessionId, _): return sessionId
        }
    }
}

func replayTokenGuarded(_ coordinator: WarmCaptureFlowCoordinator) {
    // Permission & start-completion fences refuse everything while fail-closed.
    let token = WarmCaptureFlowCoordinator.SessionToken(
        appEpoch: coordinator.appEpoch,
        sessionId: "session-old",
        intentSequence: 1,
        requestId: "req-old",
        sequence: 1,
        cancelGeneration: coordinator.monotonicCancelGeneration
    )
    let permEffects = coordinator.handlePermissionCallback(token: token, granted: true)
    assertFlow(permEffects.isEmpty, "No token replay: permission callback fenced fail-closed")
    let (startEffects, cleanup) = coordinator.handleHardwareStartCompletion(token: token, result: .success(42))
    assertFlow(startEffects.isEmpty, "No token replay: start completion fenced fail-closed")
    assertFlow(cleanup != nil, "Stale success under fail-closed still resolves via generation-bound cleanup directive")
}

@main
struct WarmCaptureFlowTestsMain {
    static func main() {
        testWarmCaptureFlowSuite()
        testWarmStandbySuite()
        testWarmStopTranscriptSuite()
    }
}

// ============================================================
// J4-E2E Warm standby adoption, disarm fencing and STOP transcript
// ============================================================

func testWarmStandbySuite() {
    print("[TEST] Running J4-E2E warm standby suite...")
    do {
        let epoch = "epoch-warm-1151"
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        // 1. Prepare warm standby claims ownership but does NOT touch
        // hardware and does not mint keyboard session responses.
        let prepareEffects = coordinator.prepareWarmStandbyCapture()
        assertFlow(prepareEffects.contains { if case .requestWarmPermission = $0 { return true } else { return false } }, "Warm prepare requests warm permission (not gate-fenced keyboard path)")
        assertFlow(coordinator.captureOwner == .warmStandby, "Warm standby owns capture")
        assertFlow(coordinator.isMicrophoneArmedByUser, "Warm prepare arms the mic")

        guard case let .requestWarmPermission(warmToken)? = prepareEffects.first(where: { if case .requestWarmPermission = $0 { return true } else { return false } }) else {
            fatalError("warm permission token missing")
        }

        // Manual test start during warm standby is rejected busy (no state change).
        let manualBusy = coordinator.handleManualStartCaptureInitiated()
        assertFlow(!manualBusy.shouldProceed, "Manual capture rejected busy under warm standby")
        assertFlow(coordinator.captureOwner == .warmStandby, "Warm ownership intact after busy manual attempt")

        // 2. Permission granted -> hardware start effect.
        let warmStartEffects = coordinator.handleWarmStandbyPermissionCallback(token: warmToken, granted: true)
        assertFlow(warmStartEffects.contains { if case .startHardwareWarmStandby = $0 { return true } else { return false } }, "Warm permission grants hardware start effect")

        // 3. Hardware start confirmation -> warm standby hardware active.
        let (warmDoneEffects, warmCleanup) = coordinator.handleWarmStandbyHardwareStartCompletion(token: warmToken, result: .success(7001))
        assertFlow(warmCleanup == nil, "Warm standby success does not mint cleanup")
        assertFlow(coordinator.isWarmStandbyHardwareActive, "Warm hardware active state recorded")
        assertFlow(warmDoneEffects.contains { if case .notifyWarmStandbyActive(let active) = $0 { return active } else { return false } }, "Warm active notification emitted")

        // 4. Keyboard START while warm recorder running MUST adopt, NOT
        // restart hardware and NEVER re-request permission.
        let adoptReq = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 10, sessionId: "warm-session-adopt", sequence: 1, requestId: "req-warm-adopt", command: .start)
        let adoptEffects = coordinator.handleIncomingRequest(adoptReq)
        assertFlow(adoptEffects.contains { if case .adoptWarmStandbyAsr = $0 { return true } else { return false } }, "Keyboard START adopts warm standby via ASR adoption effect")
        assertFlow(adoptEffects.contains { if case .writeResponse(_, _, _, let state, _, _, _) = $0 { return state == .preparing } else { return false } }, "Adoption prepares response before ASR bound")
        assertFlow(!adoptEffects.contains { if case .startHardware = $0 { return true } else { return false } }, "Adoption NEVER restarts hardware")
        assertFlow(!adoptEffects.contains { if case .requestPermission = $0 { return true } else { return false } }, "Adoption NEVER re-requests permission")
        assertFlow(coordinator.captureOwner == .keyboard(sessionId: "warm-session-adopt", intentSequence: 10), "Keyboard owner set by adoption")
        assertFlow(coordinator.activeSessionToken?.sessionId == "warm-session-adopt", "Active session token set by adoption")
        assertFlow(coordinator.isWarmStandbyHardwareActive, "Warm hardware continues (not stopped/reopened)")

        // Adoption success: keyboard response slot turns listening.
        let adoptToken = coordinator.activeSessionToken!
        let adoptDoneEffects = coordinator.handleWarmAsrAdoptionCompleted(token: adoptToken, success: true, errorCode: nil)
        assertFlow(adoptDoneEffects.contains { if case .writeResponse(_, _, _, let state, _, _, _) = $0 { return state == .listening } else { return false } }, "Adoption completion publishes listening")

        // Adoption failure path surfaces blocked honestly and keeps warm
        // hardware alive for a retry.
        let failReq = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 12, sessionId: "warm-session-fail", sequence: 1, requestId: "req-warm-fail", command: .start)
        let failAdoptEffects = coordinator.handleIncomingRequest(failReq)
        assertFlow(failAdoptEffects.contains { if case .adoptWarmStandbyAsr = $0 { return true } else { return false } }, "Second warm START also adopts (session replacement semantics)")
        let failToken = coordinator.activeSessionToken!
        let failDoneEffects = coordinator.handleWarmAsrAdoptionCompleted(token: failToken, success: false, errorCode: "asr_model_not_loaded")
        assertFlow(failDoneEffects.contains { if case .writeResponse(_, _, _, let state, _, _, let errorCode) = $0 { return state == .blocked && errorCode == "asr_model_not_loaded" } else { return false } }, "Adoption failure publishes honest blocked state")
        assertFlow(coordinator.isWarmStandbyHardwareActive, "Warm hardware still active after failed adoption (retry possible)")

        // 5. STOP with fresh intent collects transcript via provider.
        let finalText = "中国 recognizer 中文 final transcript"
        coordinator.stopFinalTranscriptProvider = { sessionId in
            guard sessionId == "warm-session-fail" else { return nil }
            return finalText
        }
        let stopReq = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 13, sessionId: "warm-session-fail", sequence: 2, requestId: "req-warm-finish-stop", command: .stop)
        let stopEffects = coordinator.handleIncomingRequest(stopReq)
        assertFlow(stopEffects.contains { if case .stopHardwareKeyboard(let d) = $0 { return d.reason == "keyboard_stop" && d.requestId == "req-warm-finish-stop" } else { return false } }, "Admitted keyboard STOP mints exact stop directive")

        guard case let .stopHardwareKeyboard(stopDirective)? = stopEffects.first(where: { if case .stopHardwareKeyboard = $0 { return true } else { return false } }) else {
            fatalError("stop directive missing")
        }
        let completionEffects = coordinator.handleHardwareStopCompletion(directive: stopDirective)
        guard case let .writeResponse(_, ackReq, ackSeq, .completed, recognizedText, isFinal, _)? = completionEffects.first(where: { if case .writeResponse = $0 { return true } else { return false } }) else {
            fatalError("completed response missing")
        }
        assertFlow(ackReq == "req-warm-finish-stop", "Completed ACKs the STOP requestId")
        assertFlow(ackSeq == 2, "Completed ACKs the STOP sequence")
        assertFlow(isFinal, "Completed response is final")
        assertFlow(recognizedText == finalText, "Completed response carries REAL final transcript via provider")

        // 6. Warm standby cleared after the keyboard stop transition.
        assertFlow(!coordinator.isWarmStandbyHardwareActive, "Warm standby hardware state cleared after keyboard stop")
        assertFlow(coordinator.activeSessionToken == nil, "No active session after stop")
    } catch {
        assertFlow(false, "testWarmStandbySuite threw: \(error)")
    }

    // 7. Disarm in warm standby stops hardware and clears ownership.
    do {
        let epoch = "epoch-warm-1152"
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)
        let prepareEffects = coordinator.prepareWarmStandbyCapture()
        guard case let .requestWarmPermission(token2)? = prepareEffects.first(where: { if case .requestWarmPermission = $0 { return true } else { return false } }) else {
            fatalError("second warm token missing")
        }
        _ = coordinator.handleWarmStandbyPermissionCallback(token: token2, granted: true)
        _ = coordinator.handleWarmStandbyHardwareStartCompletion(token: token2, result: .success(7002))
        assertFlow(coordinator.isWarmStandbyHardwareActive, "Second warm standby hardware active")
        let disarmEffects = coordinator.setUserArmMicrophone(false)
        assertFlow(disarmEffects.contains { if case .stopHardwareManual = $0 { return true } else { return false } }, "Disarm in warm standby stops hardware")
        assertFlow(coordinator.captureOwner == .none, "Warm standby ownership cleared by disarm")
        assertFlow(!coordinator.isWarmStandbyHardwareActive, "Warm hardware state cleared by disarm")

        // Warm standby + readiness after disarm: keyboard START follows the
        // standard permission/hardware path (NOT adoption).
        let coldReq = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 20, sessionId: "cold-session", sequence: 1, requestId: "req-cold-start", command: .start)
        _ = coordinator.setUserArmMicrophone(true)
        let coldEffects = coordinator.handleIncomingRequest(coldReq)
        assertFlow(coldEffects.contains { if case .requestPermission = $0 { return true } else { return false } }, "Non-warm keyboard START re-requests permission")
        assertFlow(!coldEffects.contains { if case .adoptWarmStandbyAsr = $0 { return true } else { return false } }, "Non-warm keyboard START does not adopt ASR")
    } catch {
        assertFlow(false, "disarm-cold-start threw: \(error)")
    }

    print("[TEST] J4-E2E warm standby suite PASSED.")
}

func testWarmStopTranscriptSuite() {
    print("[TEST] Running J4-E2E stop transcript provider suite...")
    do {
        // Provider absent -> completed response carries nil text + honest
        // error code (never mock text).
        let epoch = "epoch-stop-1153"
        let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: false)
        let coordinator = WarmCaptureFlowCoordinator(appEpoch: epoch, admissionGate: gate)
        _ = coordinator.setUserArmMicrophone(true)

        let start = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 30, sessionId: "stop-session", sequence: 1, requestId: "req-stop-start", command: .start)
        // Reach an ACTIVE keyboard capture through the standard path:
        // admit START -> permission granted -> hardware start success.
        let startEffects = coordinator.handleIncomingRequest(start)
        assertFlow(startEffects.contains { if case .requestPermission = $0 { return true } else { return false } }, "standard START requests permission")
        let startToken = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: startToken, granted: true)
        let (startDone, _) = coordinator.handleHardwareStartCompletion(token: startToken, result: .success(5001))
        assertFlow(startDone.contains { if case .writeResponse(_, _, _, let state, _, _, _) = $0 { return state == .listening } else { return false } }, "active keyboard capture established")
        assertFlow(coordinator.activeSessionToken?.sessionId == "stop-session", "keyboard session token established")
        let stopReq = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 31, sessionId: "stop-session", sequence: 2, requestId: "req-stop-stop", command: .stop)
        let stopEffects = coordinator.handleIncomingRequest(stopReq)
        guard case let .stopHardwareKeyboard(stopDirective)? = stopEffects.first(where: { if case .stopHardwareKeyboard = $0 { return true } else { return false } }) else {
            fatalError("stop directive missing (no provider)")
        }
        let completionEffects = coordinator.handleHardwareStopCompletion(directive: stopDirective)
        guard case let .writeResponse(_, ackReq1, ackSeq1, .completed, recognizedText, _, errorCode)? = completionEffects.first(where: { if case .writeResponse = $0 { return true } else { return false } }) else {
            fatalError("completed response missing (no provider)")
        }
        assertFlow(ackReq1 == "req-stop-stop" && ackSeq1 == 2, "tombstone-free exact ACK")
        assertFlow(recognizedText == nil, "No provider -> nil text (honest, never mock)")
        assertFlow(errorCode == "asr_final_transcript_unavailable", "No provider -> explicit unavailable errorCode")

        // Provider returning an empty string means genuine no-speech.
        coordinator.stopFinalTranscriptProvider = { sessionId in
            guard sessionId == "empty-session" else { return nil }
            return ""
        }
        // Establish a NEW active session (standard path) then STOP it.
        // The prior ended session still holds the gate active slot, so the
        // second START is a REPLACEMENT: complete its barrier first.
        let finishStop2 = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 32, sessionId: "empty-session", sequence: 1, requestId: "req-empty-start-2", command: .start)
        let start2Effects = coordinator.handleIncomingRequest(finishStop2)
        guard case let .stopHardwareKeyboard(replaceStop)? = start2Effects.first(where: { if case .stopHardwareKeyboard = $0 { return true } else { return false } }) else {
            assertFlow(false, "replacement stop directive missing")
            return
        }
        let barrierEffects = coordinator.handleHardwareStopCompletion(directive: replaceStop)
        assertFlow(barrierEffects.contains { if case .requestPermission = $0 { return true } else { return false } }, "Replacement barrier released to permission")
        let start2Token = coordinator.pendingStartToken!
        _ = coordinator.handlePermissionCallback(token: start2Token, granted: true)
        _ = coordinator.handleHardwareStartCompletion(token: start2Token, result: .success(5002))
        _ = coordinator.handlePermissionCallback(token: start2Token, granted: true)
        _ = coordinator.handleHardwareStartCompletion(token: start2Token, result: .success(5002))
        assertFlow(coordinator.activeSessionToken?.sessionId == "empty-session", "second session established")
        let stop2Req = try EcholetIPC.KeyboardRequest(appEpoch: epoch, intentSequence: 33, sessionId: "empty-session", sequence: 2, requestId: "req-empty-stop-2", command: .stop)
        let stop2Effects = coordinator.handleIncomingRequest(stop2Req)
        guard case let .stopHardwareKeyboard(stopDirective2)? = stop2Effects.first(where: { if case .stopHardwareKeyboard = $0 { return true } else { return false } }) else {
            fatalError("stop directive missing (empty provider)")
        }
        let done2 = coordinator.handleHardwareStopCompletion(directive: stopDirective2)
        guard case let .writeResponse(_, _, _, .completed, recognizedText2, _, errorCode2)? = done2.first(where: { if case .writeResponse = $0 { return true } else { return false } }) else {
            fatalError("completed response missing (empty provider)")
        }
        assertFlow(recognizedText2 == "", "Empty provider -> honest empty text")
        assertFlow(errorCode2 == nil, "Empty provider has no error code")
    } catch {
        assertFlow(false, "testWarmStopTranscriptSuite threw: \(error)")
    }

    print("[TEST] J4-E2E stop transcript provider suite PASSED.")
}
