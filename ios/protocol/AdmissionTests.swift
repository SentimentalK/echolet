import Foundation

func assertCondition(_ condition: Bool, _ message: String) {
    if !condition {
        fputs("TEST FAILED: \(message)\n", stderr)
        exit(1)
    }
}

func testAdmissionGateSuite() {
    print("[TEST] Running pure Swift EcholetAdmission Gate unit tests...")

    let epoch1 = "epoch-1111"
    let epoch2 = "epoch-2222"

    // Test 1: Cold boot fence rejects cached request before authorization
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: true)
        let req = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r1",
            command: .start
        )
        let res = gate.admitRequest(req)
        switch res {
        case .failure(.coldBootFenceReject):
            break // Expected
        default:
            assertCondition(false, "Expected cold boot fence rejection")
        }
    } catch {
        assertCondition(false, "Unexpected error in test 1: \(error)")
    }

    // Test 2: Authorizing boot admits fresh START
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: true)
        gate.authorizeBoot()
        let req = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r1",
            command: .start
        )
        let res = gate.admitRequest(req)
        assertCondition(res == .success(.admitted), "Expected fresh START admitted")
        assertCondition(gate.activeSessionId == "s1", "Active session should be s1")
        assertCondition(gate.lastIntentSequence == 1, "Watermark should be 1")
    } catch {
        assertCondition(false, "Unexpected error in test 2: \(error)")
    }

    // Test 3: Epoch mismatch fails closed
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let req = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch2, // Different epoch!
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r1",
            command: .start
        )
        let res = gate.admitRequest(req)
        switch res {
        case .failure(.staleAppEpoch):
            break // Expected
        default:
            assertCondition(false, "Expected staleAppEpoch rejection")
        }
    } catch {
        assertCondition(false, "Unexpected error in test 3: \(error)")
    }

    // Test 4: Non-monotonic intent sequence rejected
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let req1 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 5,
            sessionId: "s1",
            sequence: 1,
            requestId: "r1",
            command: .start
        )
        _ = gate.admitRequest(req1)

        // Lower intent sequence
        let reqLower = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 4,
            sessionId: "s2",
            sequence: 1,
            requestId: "r2",
            command: .start
        )
        let resLower = gate.admitRequest(reqLower)
        switch resLower {
        case .failure(.nonMonotonicIntentSequence(let inSeq, let lastSeq)):
            assertCondition(inSeq == 4 && lastSeq == 5, "Intent sequence check")
        default:
            assertCondition(false, "Expected nonMonotonicIntentSequence rejection")
        }

        // Same intent sequence (duplicate replayed intent)
        let reqSame = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 5,
            sessionId: "s2",
            sequence: 1,
            requestId: "r3",
            command: .start
        )
        let resSame = gate.admitRequest(reqSame)
        switch resSame {
        case .failure(.nonMonotonicIntentSequence):
            break // Expected
        default:
            assertCondition(false, "Expected nonMonotonicIntentSequence rejection for same intent")
        }
    } catch {
        assertCondition(false, "Unexpected error in test 4: \(error)")
    }

    // Test 5: Duplicate request_id rejected
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let req1 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r_dup",
            command: .start
        )
        _ = gate.admitRequest(req1)

        let reqDup = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 2,
            sessionId: "s1",
            sequence: 2,
            requestId: "r_dup", // duplicate ID
            command: .stop
        )
        let resDup = gate.admitRequest(reqDup)
        switch resDup {
        case .failure(.commandOrderViolation(_, _, let reason)):
            assertCondition(reason.contains("duplicate request_id"), "Duplicate request check")
        default:
            assertCondition(false, "Expected duplicate request rejection")
        }
    } catch {
        assertCondition(false, "Unexpected error in test 5: \(error)")
    }

    // Test 6: Clean session replacement by higher-intent fresh START (sequence == 1)
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let req1 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r1",
            command: .start
        )
        _ = gate.admitRequest(req1)

        let req2 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 2,
            sessionId: "s2",
            sequence: 1,
            requestId: "r2",
            command: .start
        )
        let res2 = gate.admitRequest(req2)
        assertCondition(res2 == .success(.replacedPriorSession(retiredSessionId: "s1")), "Expected replacedPriorSession(s1)")
        assertCondition(gate.activeSessionId == "s2", "Active session should now be s2")
    } catch {
        assertCondition(false, "Unexpected error in test 6: \(error)")
    }

    // Test 7: Stale STOP for old session cannot stop new session
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let req1 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r1",
            command: .start
        )
        _ = gate.admitRequest(req1)

        let req2 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 2,
            sessionId: "s2",
            sequence: 1,
            requestId: "r2",
            command: .start
        )
        _ = gate.admitRequest(req2)

        // Delayed STOP arrives with higher intent_sequence (e.g. 3) but old session_id "s1"
        let staleStop = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 3,
            sessionId: "s1",
            sequence: 2,
            requestId: "r_stale_stop",
            command: .stop
        )
        let resStale = gate.admitRequest(staleStop)
        switch resStale {
        case .failure(.staleSessionCommand(let inId, let activeId, let cmd)):
            assertCondition(inId == "s1" && activeId == "s2" && cmd == .stop, "Stale STOP rejection")
        default:
            assertCondition(false, "Expected staleSessionCommand rejection for old STOP")
        }
        assertCondition(gate.activeSessionId == "s2", "s2 must remain active")
    } catch {
        assertCondition(false, "Unexpected error in test 7: \(error)")
    }

    // Test 8: Tombstone STOP/CANCEL when no active session
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let tombstone = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s_old",
            sequence: 2,
            requestId: "r_tomb",
            command: .stop
        )
        let res = gate.admitRequest(tombstone)
        assertCondition(res == .success(.tombstoneIgnored(command: .stop)), "Expected tombstoneIgnored")
        assertCondition(gate.lastIntentSequence == 1, "Watermark should be updated")
        assertCondition(gate.activeSessionId == nil, "No active session")
    } catch {
        assertCondition(false, "Unexpected error in test 8: \(error)")
    }

    // Test 9: Valid STOP on active session ends session
    do {
        let gate = try EcholetAdmission.Gate(appEpoch: epoch1, coldBootArmed: false)
        let reqStart = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 1,
            sessionId: "s1",
            sequence: 1,
            requestId: "r_start",
            command: .start
        )
        _ = gate.admitRequest(reqStart)

        let reqStop = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 2,
            sessionId: "s1",
            sequence: 2,
            requestId: "r_stop",
            command: .stop
        )
        let resStop = gate.admitRequest(reqStop)
        assertCondition(resStop == .success(.admitted), "Expected STOP admitted")

        // Second stop on ended session rejected
        let reqStop2 = try EcholetIPC.KeyboardRequest(
            appEpoch: epoch1,
            intentSequence: 3,
            sessionId: "s1",
            sequence: 3,
            requestId: "r_stop2",
            command: .stop
        )
        let resStop2 = gate.admitRequest(reqStop2)
        switch resStop2 {
        case .failure(.commandOrderViolation):
            break // Expected
        default:
            assertCondition(false, "Expected commandOrderViolation for stopped session")
        }
    } catch {
        assertCondition(false, "Unexpected error in test 9: \(error)")
    }

    print("[TEST] All EcholetAdmission Gate unit tests PASSED successfully.")
}

@main
struct AdmissionTestsMain {
    static func main() {
        testAdmissionGateSuite()
    }
}
