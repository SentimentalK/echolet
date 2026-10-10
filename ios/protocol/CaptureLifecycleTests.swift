import Foundation

func assertTest(_ condition: Bool, _ message: String) {
    if !condition {
        fputs("CAPTURE LIFECYCLE TEST FAILED: \(message)\n", stderr)
        exit(1)
    }
}

func testCaptureLifecycleGateSuite() {
    print("[TEST] Running pure Swift CaptureLifecycleGate unit tests...")

    // Scenario 1: Clean Start & Metric Progression
    do {
        let gate = CaptureLifecycleGate()
        assertTest(gate.state == .idle, "Initial state is idle")
        assertTest(gate.activeGeneration == 0, "Initial gen is 0")

        gate.setPermissionStatus(granted: true)
        assertTest(gate.state == .ready, "State ready after permission granted")

        let tokenResult = gate.issueStartToken()
        guard case .success(let gen1) = tokenResult else {
            fatalError("Failed to issue start token")
        }
        assertTest(gen1 == 1, "Generation token 1 issued")
        assertTest(gate.state == .starting, "State is starting")

        // Metric snapshot before start ack must be dropped
        let earlySnapshot = CaptureLifecycleGate.MetricSnapshot(
            generation: gen1,
            frameIncrement: 1024,
            peakPower: -10.0,
            rmsPower: -15.0,
            elapsedSeconds: 0.05,
            sampleRate: 48000,
            channelCount: 1
        )
        let acceptedEarly = gate.acceptMetricSnapshot(earlySnapshot)
        assertTest(!acceptedEarly, "Metric snapshot before ack must be dropped")
        assertTest(gate.totalFramesRecorded == 0, "Frames remain 0")

        // Hardware start acknowledges
        let ackResult = gate.acknowledgeStartSuccess(for: gen1)
        assertTest(ackResult == .success(gen1), "Start acknowledged")
        assertTest(gate.state == .recording, "State is recording")

        // Metric snapshot during recording is accepted
        let validSnapshot = CaptureLifecycleGate.MetricSnapshot(
            generation: gen1,
            frameIncrement: 1024,
            peakPower: -8.0,
            rmsPower: -12.0,
            elapsedSeconds: 0.25,
            sampleRate: 48000,
            channelCount: 1
        )
        let acceptedValid = gate.acceptMetricSnapshot(validSnapshot)
        assertTest(acceptedValid, "Snapshot during recording accepted")
        assertTest(gate.totalFramesRecorded == 1024, "Total frames updated to 1024")
        assertTest(gate.currentMetrics.peakPower == -8.0, "Peak power updated")

        // Stop recording
        let stopGen = gate.stop(targetState: .stopped)
        assertTest(stopGen == 2, "Stop advances generation to 2")
        assertTest(gate.state == .stopped, "State is stopped")
        assertTest(gate.totalFramesRecorded == 1024, "Total frames preserved after stop")
        assertTest(gate.currentMetrics.peakPower == -160.0, "Power reset to -160 on stop")

        // Post-stop metric snapshot from gen 1 must be dropped
        let lateSnapshot = CaptureLifecycleGate.MetricSnapshot(
            generation: gen1,
            frameIncrement: 1024,
            peakPower: -5.0,
            rmsPower: -10.0,
            elapsedSeconds: 0.5,
            sampleRate: 48000,
            channelCount: 1
        )
        let acceptedLate = gate.acceptMetricSnapshot(lateSnapshot)
        assertTest(!acceptedLate, "Post-stop late snapshot must be dropped")
        assertTest(gate.totalFramesRecorded == 1024, "Frames frozen at 1024")
    }

    // Scenario 2: Old start success after Stop rejected (Delayed hardware start)
    do {
        let gate = CaptureLifecycleGate()
        let tokenResult = gate.issueStartToken()
        guard case .success(let gen1) = tokenResult else { fatalError() }

        // User / coordinator issues Stop before native hardware engine starts
        gate.stop(targetState: .stopped)
        assertTest(gate.state == .stopped, "Gate stopped")

        // Now delayed hardware engine start returns for gen1
        let delayedAck = gate.acknowledgeStartSuccess(for: gen1)
        switch delayedAck {
        case .failure(.staleGeneration(let issued, let active)):
            assertTest(issued == gen1 && active == gen1 + 1, "Delayed ack rejected with stale generation")
        default:
            assertTest(false, "Expected delayed start ack to fail")
        }
        assertTest(gate.state == .stopped, "Gate remains stopped, never transitioned to recording")
    }

    // Scenario 3: Duplicate start prevented while starting or recording
    do {
        let gate = CaptureLifecycleGate()
        let res1 = gate.issueStartToken()
        assertTest(res1 == .success(1), "First start token issued")
        assertTest(gate.state == .starting, "State is starting")

        // Concurrent/duplicate start call
        let res2 = gate.issueStartToken()
        assertTest(res2 == .failure(.alreadyRecording), "Duplicate start while starting rejected")

        // Acknowledge start 1
        _ = gate.acknowledgeStartSuccess(for: 1)
        assertTest(gate.state == .recording, "State is recording")

        // Duplicate start while recording
        let res3 = gate.issueStartToken()
        assertTest(res3 == .failure(.alreadyRecording), "Duplicate start while recording rejected")
    }

    // Scenario 4: New generation invalidates prior start and metrics
    do {
        let gate = CaptureLifecycleGate()
        _ = gate.issueStartToken() // gen 1
        _ = gate.acknowledgeStartSuccess(for: 1)
        assertTest(gate.acceptMetricSnapshot(CaptureLifecycleGate.MetricSnapshot(
            generation: 1, frameIncrement: 500, peakPower: -6.0, rmsPower: -10.0, elapsedSeconds: 0.1, sampleRate: 48000, channelCount: 1
        )), "Gen 1 metric accepted")
        assertTest(gate.totalFramesRecorded == 500, "500 frames")

        // Stop session 1
        _ = gate.stop(targetState: .stopped) // gen 2

        // Start session 2
        let resB = gate.issueStartToken() // gen 3
        guard case .success(let gen3) = resB else { fatalError() }
        _ = gate.acknowledgeStartSuccess(for: gen3)
        assertTest(gate.totalFramesRecorded == 0, "New session resets frame count to 0")

        // Stale gen 1 metric arrives late
        let lateGen1 = gate.acceptMetricSnapshot(CaptureLifecycleGate.MetricSnapshot(
            generation: 1, frameIncrement: 500, peakPower: -2.0, rmsPower: -5.0, elapsedSeconds: 0.2, sampleRate: 48000, channelCount: 1
        ))
        assertTest(!lateGen1, "Stale gen 1 metric rejected in session 2")
        assertTest(gate.totalFramesRecorded == 0, "Frame count untouched")

        // Gen 3 metric arrives
        let validGen3 = gate.acceptMetricSnapshot(CaptureLifecycleGate.MetricSnapshot(
            generation: gen3, frameIncrement: 1000, peakPower: -4.0, rmsPower: -8.0, elapsedSeconds: 0.1, sampleRate: 48000, channelCount: 1
        ))
        assertTest(validGen3, "Gen 3 metric accepted")
        assertTest(gate.totalFramesRecorded == 1000, "Frame count is 1000")
    }

    // Scenario 5: Native failure never yields recording state
    do {
        let gate = CaptureLifecycleGate()
        guard case .success(let gen) = gate.issueStartToken() else { fatalError() }
        assertTest(gate.state == .starting, "State is starting")

        let failRes = gate.acknowledgeStartFailure(for: gen, error: .engineConfigurationFailed("Bus error"))
        if case .success = failRes {
            // Success
        } else {
            assertTest(false, "Failure should be acknowledged")
        }
        assertTest(gate.state == .failed, "State transitioned to failed, never recording")
    }

    // Scenario 6: Caller expectedGeneration fence
    do {
        let gate = CaptureLifecycleGate()
        // Caller expects gate is at gen 5, but gate is at 0
        let res = gate.issueStartToken(expectedGeneration: 5)
        switch res {
        case .failure(.staleGeneration(let issued, let active)):
            assertTest(issued == 5 && active == 0, "Stale expectedGeneration rejected")
        default:
            assertTest(false, "Expected staleGeneration error")
        }
    }

    // Scenario 7: Counter overflow fails closed
    do {
        let gate = CaptureLifecycleGate()
        // Simulate high generation near max
        // Use reflection or loop isn't practical for UInt64.max, but we can verify overflow branch via testing
        // Frame increment overflow handling
        _ = gate.issueStartToken()
        _ = gate.acknowledgeStartSuccess(for: 1)
        _ = gate.acceptMetricSnapshot(CaptureLifecycleGate.MetricSnapshot(
            generation: 1, frameIncrement: UInt64.max - 10, peakPower: 0, rmsPower: 0, elapsedSeconds: 1, sampleRate: 48000, channelCount: 1
        ))
        assertTest(gate.totalFramesRecorded == UInt64.max - 10, "High frame count stored")

        // Adding 20 will overflow UInt64
        _ = gate.acceptMetricSnapshot(CaptureLifecycleGate.MetricSnapshot(
            generation: 1, frameIncrement: 20, peakPower: 0, rmsPower: 0, elapsedSeconds: 2, sampleRate: 48000, channelCount: 1
        ))
        assertTest(gate.totalFramesRecorded == UInt64.max, "Frame count clamped to UInt64.max on overflow")
    }

    // Scenario 8: AudioCaptureTapContext lifecycle ownership, callback retention, and non-leaking weak controller
    do {
        class FakeTapRegistrar {
            var installedTapHandler: ((UInt64, Float, Float, UInt64) -> Void)?

            func installTap(handler: @escaping (UInt64, Float, Float, UInt64) -> Void) {
                self.installedTapHandler = handler
            }

            func removeTap() {
                self.installedTapHandler = nil
            }
        }

        class MockCaptureController {
            var receivedSnapshots: [CaptureLifecycleGate.MetricSnapshot] = []
            var isDeallocated: Bool = false
            let stateQueue = DispatchQueue(label: "test.stateQueue")

            func handleMetricSnapshot(_ snapshot: CaptureLifecycleGate.MetricSnapshot) {
                receivedSnapshots.append(snapshot)
            }
        }

        let registrar = FakeTapRegistrar()
        weak var weakContext: AudioCaptureTapContext?
        weak var weakController: MockCaptureController?
        let activeController: MockCaptureController? = MockCaptureController()
        weakController = activeController

        // Setup scope: setup tap and exit scope
        func setupTapScope(controller: MockCaptureController) {
            let context = AudioCaptureTapContext(
                generation: 42,
                sampleRate: 48000.0,
                channelCount: 1,
                startUptimeNanoseconds: 1_000_000_000
            )
            weakContext = context

            // Tap closure captures context strongly, controller weakly
            let tapHandler = AudioCaptureTapContext.makeTapHandler(
                context: context,
                owner: controller,
                stateQueue: controller.stateQueue,
                dispatchAction: { owner, snapshot in
                    owner.handleMetricSnapshot(snapshot)
                }
            )

            registrar.installTap(handler: tapHandler)
        }

        setupTapScope(controller: activeController!)

        // 1. After setupTapScope returns, context MUST be retained by registrar's installedTapHandler!
        assertTest(weakContext != nil, "Context is strongly retained by installed tap handler after local setup scope ends")


        // 2. Simulate tap execution: feed frames at t = 1.0s and t = 1.3s (300ms later, triggering ~4Hz flush)
        assertTest(registrar.installedTapHandler != nil, "Installed tap handler exists")

        // First buffer arrives shortly after start (10ms after start; < 250ms elapsed) -> accumulated, not flushed
        registrar.installedTapHandler?(1024, 0.5, 0.25, 1_010_000_000)
        assertTest(weakContext?.accumulatedFrames == 1024, "Accumulated 1024 frames in tap context")
        assertTest(weakController?.receivedSnapshots.isEmpty == true, "No dispatch before rate limit threshold")

        // Second buffer arrives 300ms later -> exceeds 250ms threshold -> triggers flush of all 2048 frames
        registrar.installedTapHandler?(1024, 0.8, 0.4, 1_310_000_000)
        assertTest(weakContext?.accumulatedFrames == 0, "Accumulated frames reset after snapshot flush")

        // Wait for serial queue dispatch
        weakController?.stateQueue.sync {}

        assertTest(weakController?.receivedSnapshots.count == 1, "Snapshot was dispatched and received by controller")
        let snapshot = weakController!.receivedSnapshots[0]
        assertTest(snapshot.generation == 42, "Snapshot holds token 42")
        assertTest(snapshot.frameIncrement == 2048, "Snapshot accumulated frameIncrement is 2048")
        assertTest(snapshot.sampleRate == 48000.0, "Snapshot sampleRate is 48000")

        // 3. Unregister tap (removeTap): releases context
        registrar.removeTap()
        assertTest(registrar.installedTapHandler == nil, "Tap removed from registrar")
        assertTest(weakContext == nil, "Context is deallocated after removeTap releases closure")


        // 4. Verify weak controller can deallocate cleanly without reference cycle leaks
        // If we clear any external reference to controller, it deallocates
        do {
            var tempController: MockCaptureController? = MockCaptureController()
            weakController = tempController

            let ctx = AudioCaptureTapContext(
                generation: 1,
                sampleRate: 44100,
                channelCount: 1,
                startUptimeNanoseconds: 0
            )
            let handler = AudioCaptureTapContext.makeTapHandler(
                context: ctx,
                owner: tempController!,
                stateQueue: tempController!.stateQueue,
                dispatchAction: { owner, snap in owner.handleMetricSnapshot(snap) }
            )
            registrar.installTap(handler: handler)

            tempController = nil
            assertTest(weakController == nil, "Controller deallocates cleanly despite installed tap (no reference cycle)")
            registrar.removeTap()
        }
    }

    // Scenario 9: Generation-bound conditional stop semantics (AudioCaptureController.stopCaptureIfGeneration backing decision).
    // The conditional stop compares the REAL CaptureLifecycleGate.activeGeneration and
    // state on the serialized state queue; mismatched or already-stopped generations no-op.
    do {
        let gate = CaptureLifecycleGate()

        // Nothing active: no-op for any generation
        assertTest(!gate.stopIfGeneration(expectedGeneration: 1), "Conditional stop no-ops when idle")

        // Session A start: gate generation 1 starting
        guard case .success(let genA) = gate.issueStartToken() else { fatalError("Failed to issue genA") }
        assertTest(genA == 1, "Session A generation 1")
        assertTest(gate.isCaptureActiveAtGeneration(genA), "Generation 1 active during starting")
        _ = gate.acknowledgeStartSuccess(for: genA)
        assertTest(gate.state == .recording, "Session A genuinely recording")

        // Stale-success cleanup targets exactly generation 1: stops only A
        assertTest(gate.stopIfGeneration(expectedGeneration: genA), "Conditional stop executes for the exact active generation")
        assertTest(gate.state == .stopped, "Generation A stopped by conditional stop")

        // Already-stopped generation: repeat conditional stop is a strict no-op (no generation replay)
        assertTest(!gate.stopIfGeneration(expectedGeneration: genA), "Repeated conditional stop for stopped generation no-ops")

        // Session B start: gate generation 2
        guard case .success(let genB) = gate.issueStartToken() else { fatalError("Failed to issue genB") }
        _ = gate.acknowledgeStartSuccess(for: genB)
        assertTest(gate.state == .recording, "Session B genuinely recording")

        // Late stale conditional stop for A's generation must NOT touch B
        assertTest(!gate.stopIfGeneration(expectedGeneration: genA), "Conditional stop for stale A generation no-ops while B active")
        assertTest(gate.state == .recording && gate.activeGeneration == genB, "Session B untouched by stale A conditional stop")

        // Conditional stop for B's own generation stops exactly B
        assertTest(gate.stopIfGeneration(expectedGeneration: genB), "Conditional stop executes for B's exact generation")
        assertTest(gate.state == .stopped, "Session B stopped by its own conditional stop")

        // Mid-start (starting) generation is also genuinely active for conditional stop
        guard case .success(let genC) = gate.issueStartToken() else { fatalError("Failed to issue genC") }
        assertTest(gate.isCaptureActiveAtGeneration(genC), "Starting-state generation counts as genuine active")
        assertTest(gate.stopIfGeneration(expectedGeneration: genC), "Conditional stop can abort an in-flight starting generation")
        assertTest(gate.state == .stopped, "Starting generation aborted")

        // Future generation never matches
        assertTest(!gate.stopIfGeneration(expectedGeneration: genC + 1), "Never-stopped future generation no-ops")
    }

    print("[TEST] All CaptureLifecycleGate unit tests PASSED successfully.")
}

@main
struct CaptureLifecycleTestsMain {
    static func main() {
        testCaptureLifecycleGateSuite()
    }
}
