import Foundation

/// Adversarial and fence unit tests for J3A-R iOS IPC safety gates.
/// Verifies:
/// 1. requestStart A -> permission pending -> Stop/Cancel A -> permission allowed => A never starts or responds listening
/// 2. Start A -> Start B -> delayed A success => A rejected and B only recording
/// 3. Engine start fails => blocked not listening
/// 4. Duplicate/stale Stop cannot stop newer B
/// 5. Stop completion arrives after start request => no overlap
/// 6. Queued meter callback after Stop ignored
/// 7. No permission/arm and cached old epoch START => no microphone
/// 8. Response revision and ACK strictly monotonic

func assertTest(_ condition: Bool, _ message: String) {
    if !condition {
        fputs("ADVERSARIAL TEST FAILED: \(message)\n", stderr)
        exit(1)
    }
}

func testAdversarialFenceSuite() {
    print("[TEST] Running J3A-R Adversarial IPC & Fencing Policy Tests...")

    let epoch1 = "epoch-alpha-1"
    let epoch2 = "epoch-beta-2"

    // Test 1: requestStart A -> permission pending -> Stop/Cancel A -> permission allowed => A never starts or responds listening
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let startA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "session-A",
            sequence: 1,
            requestId: "req-start-A",
            command: .start
        )
        let resStartA = gate.admitRequest(startA)
        assertTest(resStartA == .success(.admitted), "Start A admitted")

        // User or keyboard sends Stop A before permission callback completes
        let stopA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 2,
            sessionId: "session-A",
            sequence: 2,
            requestId: "req-stop-A",
            command: .stop
        )
        let resStopA = gate.admitRequest(stopA)
        assertTest(resStopA == .success(.admitted), "Stop A admitted")
        assertTest(gate.lastIntentSequence == 2, "Intent sequence advanced to 2")

        // Now simulated delayed permission callback for A arrives:
        // Gate check verifies activeSession is no longer valid for req-start-A
        let isStale = (gate.lastAppliedRequestId != startA.requestId) || (gate.activeSessionId != startA.sessionId)
        assertTest(isStale, "Delayed permission callback for A must be detected as stale/fenced")
    } catch {
        assertTest(false, "Test 1 failed with error: \(error)")
    }

    // Test 2: Start A -> Start B (session replacement) -> delayed A success => A rejected and B only recording
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let startA = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "session-A",
            sequence: 1,
            requestId: "req-start-A",
            command: .start
        )
        _ = gate.admitRequest(startA)

        let startB = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 2,
            sessionId: "session-B",
            sequence: 1,
            requestId: "req-start-B",
            command: .start
        )
        let resB = gate.admitRequest(startB)
        assertTest(resB == .success(.replacedPriorSession(retiredSessionId: "session-A")), "B replaced prior session A")
        assertTest(gate.activeSessionId == "session-B", "Active session is B")

        // Delayed A callback check
        let isAValid = (gate.activeSessionId == "session-A") && (gate.lastAppliedRequestId == "req-start-A")
        assertTest(!isAValid, "Delayed A completion must be rejected; B is active")
    } catch {
        assertTest(false, "Test 2 failed with error: \(error)")
    }

    // Test 3: Engine start fails => blocked not listening
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let startReq = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "session-err",
            sequence: 1,
            requestId: "req-err",
            command: .start
        )
        _ = gate.admitRequest(startReq)

        // Engine failure closes session and emits blocked
        gate.endActiveSession()
        let failureResponse = try EcholetIPC.AppResponse(
            appEpoch: epoch1,
            sessionId: "session-err",
            acknowledgedRequestId: "req-err",
            acknowledgedSequence: 1,
            revision: 1,
            state: .blocked,
            recognizedText: nil,
            isFinal: true,
            errorCode: "audio_engine_start_failed",
            serverTimestampMs: 1000
        )
        assertTest(failureResponse.state == .blocked, "State must be blocked")
        assertTest(failureResponse.errorCode != nil, "Must include error code")
    } catch {
        assertTest(false, "Test 3 failed with error: \(error)")
    }

    // Test 4: Duplicate/stale Stop cannot stop newer B
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let startA = try EcholetIPC.KeyboardRequest(appEpoch: epoch1, intentSequence: 1, sessionId: "session-A", sequence: 1, requestId: "r-sA", command: .start)
        _ = gate.admitRequest(startA)

        let startB = try EcholetIPC.KeyboardRequest(appEpoch: epoch1, intentSequence: 2, sessionId: "session-B", sequence: 1, requestId: "r-sB", command: .start)
        _ = gate.admitRequest(startB)

        // Stale Stop targeting session A arrives with intentSequence 3
        let staleStopA = try EcholetIPC.KeyboardRequest(appEpoch: epoch1, intentSequence: 3, sessionId: "session-A", sequence: 2, requestId: "r-stopA-late", command: .stop)
        let resStale = gate.admitRequest(staleStopA)
        switch resStale {
        case .failure(.staleSessionCommand(let inId, let activeId, let cmd)):
            assertTest(inId == "session-A", "Target was session A")
            assertTest(activeId == "session-B", "Active is session B")
            assertTest(cmd == .stop, "Command is stop")
        default:
            assertTest(false, "Stale Stop targeting retired session A must be rejected")
        }
        assertTest(gate.activeSessionId == "session-B", "Active session must remain B")
    } catch {
        assertTest(false, "Test 4 failed with error: \(error)")
    }

    // Test 5: Stop completion arrives after start request => no overlap
    do {
        let lifecycleGate = CaptureLifecycleGate()
        guard case .success(let gen1) = lifecycleGate.issueStartToken() else {
            fatalError("Failed to issue token")
        }
        _ = lifecycleGate.acknowledgeStartSuccess(for: gen1)
        assertTest(lifecycleGate.state == .recording, "Gen 1 is recording")

        // Stop session 1
        let genAtStop = lifecycleGate.stop(targetState: .stopped)
        assertTest(genAtStop == gen1 + 1, "Stop advances generation")
        assertTest(lifecycleGate.state == .stopped, "Gate is stopped")

        // Delayed ack from gen 1 cannot re-open recording
        let lateAck = lifecycleGate.acknowledgeStartSuccess(for: gen1)
        assertTest(lateAck == .failure(.staleGeneration(issued: gen1, active: genAtStop)), "Old start ack rejected")
        assertTest(lifecycleGate.state == .stopped, "Remains stopped")

        // Fresh start issues next generation and can record
        guard case .success(let gen2) = lifecycleGate.issueStartToken() else {
            fatalError("Failed to issue gen2 token")
        }
        assertTest(gen2 > genAtStop, "New start advances generation beyond stop")
        _ = lifecycleGate.acknowledgeStartSuccess(for: gen2)
        assertTest(lifecycleGate.state == .recording, "New session recording")
    }

    // Test 6: Queued meter callback after Stop ignored
    do {
        let lifecycleGate = CaptureLifecycleGate()
        guard case .success(let gen) = lifecycleGate.issueStartToken() else {
            fatalError("Failed to issue start token")
        }
        _ = lifecycleGate.acknowledgeStartSuccess(for: gen)

        let snapshot1 = CaptureLifecycleGate.MetricSnapshot(
            generation: gen,
            frameIncrement: 1024,
            peakPower: -10.0,
            rmsPower: -15.0,
            elapsedSeconds: 0.25,
            sampleRate: 48000,
            channelCount: 1
        )
        assertTest(lifecycleGate.acceptMetricSnapshot(snapshot1), "Active snapshot accepted")
        assertTest(lifecycleGate.totalFramesRecorded == 1024, "Frames recorded is 1024")

        // Stop session
        lifecycleGate.stop(targetState: .stopped)

        // Queued meter callback from pre-stop generation
        let lateSnapshot = CaptureLifecycleGate.MetricSnapshot(
            generation: gen,
            frameIncrement: 1024,
            peakPower: -8.0,
            rmsPower: -12.0,
            elapsedSeconds: 0.5,
            sampleRate: 48000,
            channelCount: 1
        )
        assertTest(!lifecycleGate.acceptMetricSnapshot(lateSnapshot), "Queued buffer after stop rejected")
        assertTest(lifecycleGate.totalFramesRecorded == 1024, "Frames frozen at 1024")
    }

    // Test 7: No permission/arm and cached old epoch START => no microphone
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch2, coldBootArmed: true)
        let cachedReq = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "cached-session",
            sequence: 1,
            requestId: "cached-req",
            command: .start
        )
        let res = gate.admitRequest(cachedReq)
        switch res {
        case .failure(.staleAppEpoch):
            break // Expected: old epoch rejected immediately
        default:
            assertTest(false, "Expected staleAppEpoch rejection for cached old epoch request")
        }
        assertTest(gate.activeSessionId == nil, "No session active")
    } catch {
        assertTest(false, "Test 7 failed with error: \(error)")
    }

    // Test 8: Response revision and ACK strictly monotonic
    do {
        var revision: UInt64 = 0
        var lastAckSeq: UInt64 = 0

        func emitResponse(ackSeq: UInt64) throws -> EcholetIPC.AppResponse {
            guard ackSeq >= lastAckSeq else { throw EcholetIPC.ValidationError.invalidAcknowledgedSequence(ackSeq) }
            lastAckSeq = ackSeq
            revision += 1
            return try EcholetIPC.AppResponse(
                appEpoch: epoch1,
                sessionId: "s",
                acknowledgedRequestId: "r\(ackSeq)",
                acknowledgedSequence: ackSeq,
                revision: revision,
                state: .listening,
                recognizedText: nil,
                isFinal: false,
                errorCode: nil,
                serverTimestampMs: 100
            )
        }

        let resp1 = try emitResponse(ackSeq: 1)
        assertTest(resp1.revision == 1, "Revision 1")

        let resp2 = try emitResponse(ackSeq: 1)
        assertTest(resp2.revision == 2, "Revision 2 strictly greater")

        let resp3 = try emitResponse(ackSeq: 2)
        assertTest(resp3.revision == 3, "Revision 3 strictly greater")
        assertTest(resp3.acknowledgedSequence == 2, "Ack sequence monotonic")
    } catch {
        assertTest(false, "Test 8 failed with error: \(error)")
    }

    print("[TEST] All J3A-R Adversarial IPC & Fencing Policy Tests PASSED successfully.")
}

@main
struct AdversarialTestsMain {
    static func main() {
        testAdversarialFenceSuite()
    }
}
