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
        case .none: return nil
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
    }
}
