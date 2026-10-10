import Foundation

/// Small CLI smoke test runner verifying Swift Foundation-only EcholetIPC implementation.
///
/// Tests:
/// 1. Decoding Golden JSON fixtures (start, stop, partial, final).
/// 2. Encoding round-trip equality.
/// 3. Invariant validation (reject empty IDs, version mismatch, sequence=0).
/// 4. Rejection of malformed / unknown payload structures.
///
/// Returns exit code 0 on success, non-zero on failure.

@main
struct IPCCodecSmoke {
    static func main() {
        print("[IPCCodecSmoke] Starting Swift Foundation IPC Codec Smoke Suite...")

        let arguments = CommandLine.arguments
        guard arguments.count >= 2 else {
            fputs("Usage: \(arguments[0]) <fixtures_directory_path>\n", stderr)
            exit(1)
        }

        let fixturesDir = URL(fileURLWithPath: arguments[1])
        let decoder = EcholetIPC.makeDecoder()
        let encoder = EcholetIPC.makeEncoder()

        // 1. Verify golden_request_start.json
        do {
            let startUrl = fixturesDir.appendingPathComponent("golden_request_start.json")
            let data = try Data(contentsOf: startUrl)
            let req = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: data)
            try req.validate()

            assert(req.protocolVersion == 1, "protocolVersion must be 1")
            assert(req.sessionId == "session-golden-42", "sessionId mismatch")
            assert(req.sequence == 1, "sequence mismatch")
            assert(req.requestId == "req-golden-001", "requestId mismatch")
            assert(req.command == .start, "command must be start")
            assert(req.clientTimestampMs == 1728570000123, "clientTimestampMs mismatch")

            // Round trip
            let encoded = try encoder.encode(req)
            let roundTripped = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: encoded)
            assert(roundTripped == req, "roundtrip mismatch for start request")
            print("  ✓ golden_request_start.json decoded and round-tripped successfully")
        } catch {
            fputs("FAIL: golden_request_start.json: \(error)\n", stderr)
            exit(2)
        }

        // 2. Verify golden_request_stop.json
        do {
            let stopUrl = fixturesDir.appendingPathComponent("golden_request_stop.json")
            let data = try Data(contentsOf: stopUrl)
            let req = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: data)
            try req.validate()

            assert(req.protocolVersion == 1)
            assert(req.sessionId == "session-golden-42")
            assert(req.sequence == 2)
            assert(req.requestId == "req-golden-002")
            assert(req.command == .stop)
            assert(req.clientTimestampMs == nil)

            let encoded = try encoder.encode(req)
            let roundTripped = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: encoded)
            assert(roundTripped == req, "roundtrip mismatch for stop request")
            print("  ✓ golden_request_stop.json decoded and round-tripped successfully")
        } catch {
            fputs("FAIL: golden_request_stop.json: \(error)\n", stderr)
            exit(3)
        }

        // 3. Verify golden_response_partial.json
        do {
            let partialUrl = fixturesDir.appendingPathComponent("golden_response_partial.json")
            let data = try Data(contentsOf: partialUrl)
            let resp = try decoder.decode(EcholetIPC.AppResponse.self, from: data)
            try resp.validate()

            assert(resp.protocolVersion == 1)
            assert(resp.sessionId == "session-golden-42")
            assert(resp.acknowledgedRequestId == "req-golden-001")
            assert(resp.acknowledgedSequence == 1)
            assert(resp.revision == 3)
            assert(resp.state == .listening)
            assert(resp.recognizedText == "testing speech recognition")
            assert(!resp.isFinal)
            assert(resp.serverTimestampMs == 1728570001500)

            let encoded = try encoder.encode(resp)
            let roundTripped = try decoder.decode(EcholetIPC.AppResponse.self, from: encoded)
            assert(roundTripped == resp, "roundtrip mismatch for partial response")
            print("  ✓ golden_response_partial.json decoded and round-tripped successfully")
        } catch {
            fputs("FAIL: golden_response_partial.json: \(error)\n", stderr)
            exit(4)
        }

        // 4. Verify golden_response_final.json
        do {
            let finalUrl = fixturesDir.appendingPathComponent("golden_response_final.json")
            let data = try Data(contentsOf: finalUrl)
            let resp = try decoder.decode(EcholetIPC.AppResponse.self, from: data)
            try resp.validate()

            assert(resp.protocolVersion == 1)
            assert(resp.sessionId == "session-golden-42")
            assert(resp.acknowledgedRequestId == "req-golden-002")
            assert(resp.acknowledgedSequence == 2)
            assert(resp.revision == 4)
            assert(resp.state == .completed)
            assert(resp.recognizedText == "testing speech recognition.")
            assert(resp.isFinal)

            let encoded = try encoder.encode(resp)
            let roundTripped = try decoder.decode(EcholetIPC.AppResponse.self, from: encoded)
            assert(roundTripped == resp, "roundtrip mismatch for final response")
            print("  ✓ golden_response_final.json decoded and round-tripped successfully")
        } catch {
            fputs("FAIL: golden_response_final.json: \(error)\n", stderr)
            exit(5)
        }

        // 5. Verify validation rejects invalid/malformed models
        do {
            // Empty session
            var threw = false
            do {
                _ = try EcholetIPC.KeyboardRequest(sessionId: "   ", sequence: 1, requestId: "r1", command: .start)
            } catch EcholetIPC.ValidationError.emptySessionId {
                threw = true
            } catch {
                fputs("Unexpected error: \(error)\n", stderr)
            }
            assert(threw, "Must reject empty session ID")

            // Zero sequence
            threw = false
            do {
                _ = try EcholetIPC.KeyboardRequest(sessionId: "s1", sequence: 0, requestId: "r1", command: .start)
            } catch EcholetIPC.ValidationError.invalidSequence(0) {
                threw = true
            } catch {
                fputs("Unexpected error: \(error)\n", stderr)
            }
            assert(threw, "Must reject sequence 0")

            // Empty request ID
            threw = false
            do {
                _ = try EcholetIPC.KeyboardRequest(sessionId: "s1", sequence: 1, requestId: "", command: .start)
            } catch EcholetIPC.ValidationError.emptyRequestId {
                threw = true
            } catch {
                fputs("Unexpected error: \(error)\n", stderr)
            }
            assert(threw, "Must reject empty request ID")

            // Empty response session ID
            threw = false
            do {
                _ = try EcholetIPC.AppResponse(sessionId: "", acknowledgedRequestId: "r1", acknowledgedSequence: 1, revision: 1, state: .listening)
            } catch EcholetIPC.ValidationError.emptySessionId {
                threw = true
            } catch {
                fputs("Unexpected error: \(error)\n", stderr)
            }
            assert(threw, "Must reject empty response session ID")

            // Malformed JSON decode
            let badJson = Data("{\"not_valid_json\":".utf8)
            threw = false
            do {
                _ = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: badJson)
            } catch {
                threw = true
            }
            assert(threw, "Must throw on malformed JSON")

            // Unsupported protocol version
            let badVersionJson = Data("""
            {
                "protocol_version": 99,
                "session_id": "s1",
                "sequence": 1,
                "request_id": "r1",
                "command": "start"
            }
            """.utf8)
            threw = false
            do {
                let parsed = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: badVersionJson)
                try parsed.validate()
            } catch EcholetIPC.ValidationError.unsupportedProtocolVersion(99) {
                threw = true
            } catch {
                fputs("Unexpected error: \(error)\n", stderr)
            }
            assert(threw, "Must reject unsupported protocol version 99")

            print("  ✓ Invalidation & malformed payload rejection checks passed")
        }

        print("[IPCCodecSmoke] ALL TESTS PASSED.")
    }
}
