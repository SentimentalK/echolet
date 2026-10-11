import Foundation
import CoreFoundation

/// Keyboard-owned single-button dictation session controller (pure Foundation).
///
/// This is the testable core of the J5 single-button keyboard UI. The
/// `UIInputViewController` wires it to the real `textDocumentProxy`, the App
/// Group `UserDefaults` suite, `NSExtensionContext.open` and Darwin hints;
/// all decisions (state transitions, request publication, response
/// correlation, insert-once gating, focus fail-closed, auto/tapped launch
/// policy) live here and are exercised by CLI tests without UIKit.
///
/// State machine: ready -> starting -> listening -> finalizing -> ready,
/// with honest terminal `blocked(reason)` observations surfaced in status.
///
/// Launch policy:
/// * If a positively FRESH warm recorder record exists for the current app
///   epoch, START is issued directly — the app is never relaunched.
/// * Otherwise, at most ONE automatic containing-app open attempt per fresh
///   keyboard appearance is made through the injected `openHost` seam (the
///   ONLY public, extension-point-safe API surface; no private selectors).
///   User taps may re-attempt. `AUTO_LAUNCH_UNSUPPORTED` is surfaced
///   honestly when the platform cannot perform the open.
/// * Launch demand consumes a unique nonce; nonce-matched fresh phase
///   observations (uriReceived/modelLoaded/warmReady) drive auto-START
///   exactly once while the warm recorder stays untouched by the keyboard.
/// * After a warmReady observation, automatic reopen attempts are
///   suppressed (reentry-loop fence); the user tap path stays available.
/// * Red "Stop Listening" is reachable ONLY after a real `.listening` ACK,
///   and final insertion happens exactly once per session via the injected
///   insert seam after the correlated STOP ACK.
public final class EcholetKeyboardSessionController {

    // MARK: - Constants

    public static let intentSequenceStorageKey = "echolet.keyboard.intent_sequence_counter"

    /// Waiting for the START to surface a real mic `.listening` ACK.
    public static let startAckTimeoutMs: UInt64 = 15_000
    /// Waiting for launch evidence (phase record, warm-live truth, or open
    /// callback) before declaring the automatic launch honestly failed.
    public static let openEvidenceTimeoutMs: UInt64 = 6_000
    /// Waiting for the FINAL transcript after an exact STOP.
    public static let finalizeAckTimeoutMs: UInt64 = 20_000

    public enum OpenAttemptResult: Equatable {
        case initiated
        case failed(reason: String)
        case unsupported(reason: String)
    }

    public struct Environment {
        public let defaults: UserDefaults
        public let nowMs: () -> UInt64
        public let currentAppEpoch: () -> String?
        /// Public-API bounded containing-app open attempt. `.initiated` means
        /// the open call was handed to the OS (an async completion may follow
        /// through `handleOpenCompletion`); `.unsupported` means the
        /// platform has no API surface for the open; `.failed` means the
        /// attempt was synchronously rejected.
        public let openHost: (URL) -> OpenAttemptResult
        public let insertText: (String) -> Bool
        /// Editor identity probe; nil == nil compares as "same (unknown)".
        /// The controller records it at session start and at the real
        /// `.listening` ACK, and fails closed at insert time on mismatch.
        public let docFingerprint: () -> (before: String?, after: String?, selected: String?)
        public let log: ((String) -> Void)?

        public init(
            defaults: UserDefaults,
            nowMs: @escaping () -> UInt64,
            currentAppEpoch: @escaping () -> String?,
            openHost: @escaping (URL) -> OpenAttemptResult,
            insertText: @escaping (String) -> Bool,
            docFingerprint: @escaping () -> (before: String?, after: String?, selected: String?),
            log: ((String) -> Void)? = nil
        ) {
            self.defaults = defaults
            self.nowMs = nowMs
            self.currentAppEpoch = currentAppEpoch
            self.openHost = openHost
            self.insertText = insertText
            self.docFingerprint = docFingerprint
            self.log = log
        }
    }

    // MARK: - Observable state

    public enum Phase: Equatable {
        case ready
        case opening
        case starting
        case listening
        case finalizing
        case blocked(reason: String)
    }

    public private(set) var phase: Phase = .ready
    /// Honest platform-capability record observed at runtime.
    public private(set) var autoLaunchUnsupportedReason: String?
    public private(set) var lastInsertedText: String?
    public private(set) var insertCount: Int = 0

    private struct ActiveSession {
        let epoch: String
        let sessionId: String
        let intentSequence: UInt64
        var requestId: String
        var sequence: UInt64
        var lastAcceptedRevision: UInt64
        /// Editor identity recorded at the real `.listening` ACK (fallback:
        /// session start for warm-adopted sessions). Insert-time identity
        /// must match or the write fails closed.
        var documentFingerprint: DocumentFingerprint?
        var finalTextInserted: Bool
    }

    private struct LaunchContext {
        let nonce: String
        let url: URL
        let startedAtMs: UInt64
        let autoStartIssued: Bool
    }

    private struct DocumentFingerprint {
        let before: String?
        let after: String?
        let selected: String?

        init(probe: () -> (before: String?, after: String?, selected: String?)) {
            let value = probe()
            self.before = value.before
            self.after = value.after
            self.selected = value.selected
        }

        func equals(_ other: DocumentFingerprint) -> Bool {
            return before == other.before && after == other.after && selected == other.selected
        }
    }

    private let env: Environment
    private var activeSession: ActiveSession?
    private var launchContext: LaunchContext?
    private var autoAttemptDoneForCurrentAppearance = false
    private var lastStartAttemptMs: UInt64 = 0
    private var lastRevisionArrivalMs: UInt64 = 0

    public init(environment: Environment) {
        self.env = environment
    }

    // MARK: - Derived UI helpers

    public var shouldShowRedStopButton: Bool {
        return phase == .listening
    }

    public var primaryButtonLabel: String {
        switch phase {
        case .ready: return "Start Listening"
        case .opening, .starting: return "Starting…"
        case .listening: return "Stop Listening"
        case .finalizing: return "Finalizing…"
        case .blocked: return "Start Listening"
        }
    }

    public var isPrimaryButtonInteractive: Bool {
        switch phase {
        case .ready, .blocked: return true
        default: return false
        }
    }

    // MARK: - Logging

    private func log(_ event: String) {
        env.log?(event)
    }

    // MARK: - Appearance lifecycle

    /// Recognition of REAL warm readiness happens here: if a positively
    /// fresh warm recorder record exists for the current epoch, START is
    /// issued directly and the containing app is NOT relaunched.
    public func handleAppeared() {
        autoAttemptDoneForCurrentAppearance = false
        log("KBD_APPEAR")
        _ = consumePendingResponse() // surface state written while hidden
        // Resume an in-flight launch instead of re-attempting: the demand is
        // already written and the app may be mid-prepare (or the user may
        // have just opened it manually — the foreground hand path).
        if let ctx = launchContext, env.nowMs() - ctx.startedAtMs <= EcholetLaunch.defaultDemandTtlMs {
            if phase == .ready {
                setPhase(.opening)
                log("OPEN_RESUMED(inflight_launch)")
            }
        } else if phase == .ready {
            if freshWarmLiveExists() {
                log("OPEN_SKIPPED(warm_live_fresh)")
                beginStart(reason: "auto_adopt_warm")
            } else {
                attemptLaunch(auto: true)
            }
        }
        // Expired launch context evaporates truthfully.
        if let ctx = launchContext, env.nowMs() - ctx.startedAtMs > EcholetLaunch.defaultDemandTtlMs {
            launchContext = nil
            log("BLOCKED(launch_nonce_expired)")
            if phase == .opening {
                setPhase(.ready)
            }
        }
    }

    /// Keyboard hidden / about to hide: truthful cleanup. A session that is
    /// still live (listening or finalizing) is closed with a best-effort
    /// CANCEL command always carrying fresh intent correlation.
    public func handleDisappeared() {
        log("HIDE_CANCEL")
        cancelActiveSession(reason: "keyboard_hidden")
        if let ctx = launchContext, env.nowMs() - ctx.startedAtMs > EcholetKeyboardSessionController.openEvidenceTimeoutMs {
            launchContext = nil
        }
        setPhase(.ready)
    }

    // MARK: - Warm readiness authority

    public func freshWarmLiveExists() -> Bool {
        return EcholetLaunch.freshWarmLive(
            nowMs: env.nowMs(),
            currentAppEpoch: env.currentAppEpoch(),
            defaults: env.defaults
        ) != nil
    }

    // MARK: - Launch attempts

    /// Auto attempt: at most once per fresh appearance, never relaunching
    /// over an existing fresh warm recorder, never during a recent
    /// warmReady window (reentry-loop fence).
    private func attemptLaunch(auto: Bool) {
        if freshWarmLiveExists() {
            log("OPEN_SKIPPED(warm_live_fresh)")
            beginStart(reason: auto ? "auto_adopt_warm" : "tap_adopt_warm")
            return
        }
        if auto {
            if autoAttemptDoneForCurrentAppearance {
                return
            }
            if recentWarmReadyLaunchExists() {
                log("OPEN_SKIPPED(recent_warm_ready_no_reopen)")
                autoLaunchUnsupportedReason = "recent_warm_ready_no_reopen"
                return
            }
            autoAttemptDoneForCurrentAppearance = true
        }

        let nonce = UUID().uuidString
        let now = env.nowMs()
        guard let url = try? EcholetLaunch.makeDictationPrepareURL(nonce: nonce, tsMs: now) else {
            return
        }
        let demand = EcholetLaunch.DemandRecord(
            nonce: nonce,
            createdAtMs: now,
            ttlMs: EcholetLaunch.defaultDemandTtlMs
        )
        EcholetLaunch.writeDemand(demand, defaults: env.defaults)
        launchContext = LaunchContext(nonce: nonce, url: url, startedAtMs: now, autoStartIssued: false)
        setPhase(.opening)
        log("OPEN_ATTEMPT(auto=\(auto))")

        switch env.openHost(url) {
        case .initiated:
            log("OPEN_HANDOFF_INITIATED")
        case .failed(let reason):
            log("OPEN_RESULT(failed:\(reason))")
            failOpen(reason: "open_failed_\(reason)")
        case .unsupported(let reason):
            log("OPEN_RESULT(unsupported:\(reason))")
            failOpen(reason: "AUTO_LAUNCH_UNSUPPORTED_\(reason)")
        }
    }

    private func failOpen(reason: String) {
        autoLaunchUnsupportedReason = reason
        launchContext = nil
        if phase == .opening {
            setPhase(.ready)
        }
        log("BLOCKED(\(reason))")
    }

    /// True when an app-side warmReady phase for the CURRENT epoch was
    /// observed recently (inside TTL). Doubles as the reopen-loop fence.
    public func recentWarmReadyLaunchExists() -> Bool {
        guard let epoch = env.currentAppEpoch() else { return false }
        guard let record = EcholetLaunch.readPhase(defaults: env.defaults),
              record.appEpoch == epoch,
              record.phase == .warmReady,
              env.nowMs() >= record.updatedAtMs,
              env.nowMs() - record.updatedAtMs <= EcholetLaunch.defaultPhaseTtlMs else {
            return false
        }
        return true
    }

    /// Async completion from the extension-context open callback.
    public func handleOpenCompletion(success: Bool, reason: String?) {
        if success {
            log("OPEN_RESULT(callback_completed)")
        } else {
            // The callback alone is not always trustworthy on this Apple
            // extension point; positive phase evidence resolves the truth.
            log("OPEN_RESULT(callback_failed_\(reason ?? "unknown"))")
            autoLaunchUnsupportedReason = reason
        }
    }

    // MARK: - User taps

    public func handlePrimaryButtonTap() {
        switch phase {
        case .ready, .blocked:
            // A positively fresh active warm recorder is adopted directly;
            // otherwise the user gesture drives a best-supported launch.
            if freshWarmLiveExists() {
                beginStart(reason: "tap_adopt_warm")
            } else {
                attemptLaunch(auto: false)
            }
        case .listening:
            handleStopTap()
        case .opening, .starting, .finalizing:
            log("TAP_IGNORED(state=non_terminal)")
        }
    }

    public func handleStopTap() {
        guard let active = activeSession, phase == .listening else {
            log("STOP_TAP_IGNORED(no_listening_session)")
            return
        }
        guard let nextIntentSeq = reserveNextIntentSequence() else {
            log("BLOCKED(intent_sequence_overflow)")
            return
        }
        let stopRequestId = UUID().uuidString
        let stopSequence = active.sequence + 1
        let command = try? EcholetIPC.KeyboardRequest(
            appEpoch: active.epoch,
            intentSequence: nextIntentSeq,
            sessionId: active.sessionId,
            sequence: stopSequence,
            requestId: stopRequestId,
            command: .stop,
            clientTimestampMs: env.nowMs()
        )
        guard let command = command, writeRequest(command) else {
            log("BLOCKED(stop_write_failed)")
            return
        }
        // Retain the session; expected ACK identity becomes the exact STOP.
        activeSession?.requestId = stopRequestId
        activeSession?.sequence = stopSequence
        setPhase(.finalizing)
        log("STOP_SENT(seq=\(stopSequence))")
    }

    // MARK: - Session start / cancel

    public func beginStart(reason: String) {
        guard phaseIsStartable else { return }
        guard let epoch = env.currentAppEpoch(), !epoch.isEmpty else {
            log("BLOCKED(app_epoch_missing)")
            setPhase(.blocked(reason: "Open Echolet app once to publish its epoch."))
            return
        }
        // Rapid-tap guard: minimum real spacing between request writes.
        let now = env.nowMs()
        if now - lastStartAttemptMs < 800 && lastStartAttemptMs > 0 {
            log("START_RAPID_TAP_IGNORED")
            return
        }
        lastStartAttemptMs = now

        let newSessionId = UUID().uuidString
        let newRequestId = UUID().uuidString
        guard let nextIntentSeq = reserveNextIntentSequence() else {
            log("BLOCKED(intent_sequence_overflow)")
            return
        }

        let request = try? EcholetIPC.KeyboardRequest(
            appEpoch: epoch,
            intentSequence: nextIntentSeq,
            sessionId: newSessionId,
            sequence: 1,
            requestId: newRequestId,
            command: .start,
            clientTimestampMs: now
        )
        guard let request = request, writeRequest(request) else {
            log("BLOCKED(start_write_failed)")
            return
        }

        activeSession = ActiveSession(
            epoch: epoch,
            sessionId: newSessionId,
            intentSequence: nextIntentSeq,
            requestId: newRequestId,
            sequence: 1,
            lastAcceptedRevision: 0,
            documentFingerprint: DocumentFingerprint(probe: env.docFingerprint),
            finalTextInserted: false
        )
        setPhase(.starting)
        log("START_SENT(reason=\(reason),session=\(newSessionId.prefix(8)))")
    }

    private var phaseIsStartable: Bool {
        switch phase {
        case .ready, .blocked: return true
        default: return false
        }
    }

    public func cancelActiveSession(reason: String) {
        guard let active = activeSession else { return }
        activeSession = nil
        setPhase(.ready)
        log("SESSION_CLOSED(\(reason))")
        guard let nextIntentSeq = reserveNextIntentSequence() else { return }
        let cancelRequestId = UUID().uuidString
        let cancelReq = try? EcholetIPC.KeyboardRequest(
            appEpoch: active.epoch,
            intentSequence: nextIntentSeq,
            sessionId: active.sessionId,
            sequence: active.sequence + 1,
            requestId: cancelRequestId,
            command: .cancel,
            clientTimestampMs: env.nowMs()
        )
        if let req = cancelReq, writeRequest(req) {
            log("CANCEL_SENT(seq=\(req.sequence))")
        }
    }

    // MARK: - Focus / editor change handling

    /// Fail closed on REAL editor changes while a session exists; a change
    /// that equals the fingerprint recorded at the listening ack is our own
    /// insertion churn and must NOT cancel a valid session.
    public func noteTextWillChange() {
        guard let active = activeSession else { return }
        let fp = DocumentFingerprint(probe: env.docFingerprint)
        if let base = active.documentFingerprint, fp.equals(base) {
            log("FOCUS_IGNORED(same_doc)")
            return
        }
        log("FOCUS_CHANGED(editor_identity_mismatch)")
        cancelActiveSession(reason: "focus_changed")
        setPhase(.blocked(reason: "Editor focus changed; session cancelled."))
    }

    // MARK: - Timer tick (bounded polling while visible)

    /// Called at ~0.3s cadence while the keyboard is visible. Consumes
    /// responses, resolves launch evidence, and applies bounded timeouts.
    public func handleTimerTick() {
        _ = consumePendingResponse()
        resolveLaunchEvidence()
        applyTimeouts()
    }

    private func resolveLaunchEvidence() {
        guard let ctx = launchContext else { return }
        let now = env.nowMs()

        // Positive app acknowledgement through the nonce-fenced phase record.
        if let epoch = env.currentAppEpoch(),
           let record = EcholetLaunch.freshPhaseMatching(nonce: ctx.nonce, appEpoch: epoch, nowMs: now, defaults: env.defaults) {
            switch record.phase {
            case .uriReceived:
                log("APP_URI_RECEIVED")
            case .modelLoaded:
                log("MODEL_LOAD(observed)")
            case .micNativeReady:
                log("MIC_NATIVE_READY(observed)")
            case .warmReady:
                log("OPEN_RESULT(warm_ready_evidence)")
                issueStartForWarmReady(context: ctx)
            case .requested:
                break
            case .failed:
                log("OPEN_RESULT(app_failed_\(record.errorCode ?? "unknown"))")
                failOpen(reason: "app_activation_failed_\(record.errorCode ?? "unknown")")
            }
            return
        }

        // Alternative genuine evidence: warm recorder now live.
        if phase == .opening, freshWarmLiveExists() {
            log("OPEN_RESULT(evidence_warm_live)")
            launchContext = nil
            setPhase(.ready)
            beginStart(reason: "warm_live_evidence")
            return
        }

        if phase == .opening, now - ctx.startedAtMs > EcholetKeyboardSessionController.openEvidenceTimeoutMs {
            log("OPEN_RESULT(timeout_no_ack)")
            failOpen(reason: "AUTO_LAUNCH_UNSUPPORTED_open_timeout_no_ack")
        }
    }

    private func issueStartForWarmReady(context: LaunchContext) {
        guard !context.autoStartIssued else { return }
        launchContext = LaunchContext(
            nonce: context.nonce,
            url: context.url,
            startedAtMs: context.startedAtMs,
            autoStartIssued: true
        )
        guard activeSession == nil else { return }
        // warmReady evidence authorizes the START regardless of the current
        // opening presentation state; evidence is consumed exactly once.
        launchContext = nil
        setPhase(.ready)
        beginStart(reason: "warm_ready_evidence")
    }

    private func applyTimeouts() {
        guard activeSession != nil else { return }
        let now = env.nowMs()
        switch phase {
        case .starting:
            if now >= lastStartAttemptMs,
               now - lastStartAttemptMs >= EcholetKeyboardSessionController.startAckTimeoutMs {
                log("BLOCKED(start_ack_timeout)")
                activeSession = nil
                setPhase(.blocked(reason: "Mic start not acknowledged in time. Try again."))
            }
        case .finalizing:
            let base = lastRevisionArrivalMs > 0 ? lastRevisionArrivalMs : lastStartAttemptMs
            if now >= base,
               now - base >= EcholetKeyboardSessionController.finalizeAckTimeoutMs {
                log("BLOCKED(finalize_ack_timeout)")
                activeSession = nil
                setPhase(.blocked(reason: "Final transcript not acknowledged in time."))
            }
        default:
            break
        }
    }

    // MARK: - Response consumption (correlation / monotonic / insert-once)

    /// Reads the latest App Group response and applies strict gates: epoch,
    /// session ID, exact ACK identity, monotonic revision and editor
    /// identity. Inserts the genuine final transcript EXACTLY ONCE per
    /// session when the final ACK correlates.
    @discardableResult
    public func consumePendingResponse() -> Bool {
        guard let active = activeSession else { return false }
        guard let responseData = env.defaults.data(forKey: EcholetIPC.appResponseKey) else { return false }
        guard let response = try? EcholetIPC.makeDecoder().decode(EcholetIPC.AppResponse.self, from: responseData) else {
            log("RESPONSE_DECODE_FAILED")
            return false
        }

        // Gate 1: current process epoch (cold dead app snapshots fail).
        guard response.appEpoch == active.epoch else {
            log("RESPONSE_IGNORED(stale_epoch)")
            return false
        }
        // Gate 2: exact session.
        guard response.sessionId == active.sessionId else {
            log("RESPONSE_IGNORED(other_session)")
            return false
        }
        // Gate 3: exact correlation of the recognized ACK identity.
        guard response.acknowledgedRequestId == active.requestId,
              response.acknowledgedSequence == active.sequence else {
            log("RESPONSE_IGNORED(ack_mismatch)")
            return false
        }
        // Gate 4: strictly monotonic revision.
        guard response.revision > active.lastAcceptedRevision else {
            log("RESPONSE_IGNORED(stale_revision)")
            return false
        }

        activeSession?.lastAcceptedRevision = response.revision
        lastRevisionArrivalMs = env.nowMs()

        if response.state == .listening {
            log("START_ACK")
            activeSession?.documentFingerprint = DocumentFingerprint(probe: env.docFingerprint)
            setPhase(.listening)
            return false
        }
        if !response.isFinal {
            // Non-final intermediate revision: watermark recorded only.
            return false
        }

        // Terminal final responses:
        if response.state == .blocked {
            activeSession = nil
            setPhase(.blocked(reason: "Mic blocked (code: \(response.errorCode ?? "unknown"))."))
            log("BLOCKED(app_\(response.errorCode ?? "unknown"))")
            return false
        }

        if response.state == .completed {
            let finalText = response.recognizedText ?? ""
            if finalText.isEmpty {
                activeSession = nil
                setPhase(.ready)
                log("STOP_ACK(empty_no_speech)")
                return false
            }
            if activeSession?.finalTextInserted == true {
                activeSession = nil
                setPhase(.ready)
                log("STOP_ACK(duplicate_suppressed)")
                return false
            }
            // Target-document identity gate (fail closed on real change).
            if let recorded = activeSession?.documentFingerprint {
                let currentFp = DocumentFingerprint(probe: env.docFingerprint)
                if !currentFp.equals(recorded) {
                    activeSession = nil
                    setPhase(.blocked(reason: "Editor changed during dictation; final text NOT inserted."))
                    log("INSERT_SUPPRESSED(editor_mismatch)")
                    return false
                }
            }
            let inserted = env.insertText(finalText)
            if inserted {
                activeSession?.finalTextInserted = true
                lastInsertedText = finalText
                insertCount += 1
                log("INSERT_FINAL_ONCE(chars=\(finalText.count))")
            } else {
                log("INSERT_SUPPRESSED(proxy_refused)")
            }
            activeSession = nil
            setPhase(.ready)
            return inserted
        }

        return false
    }

    // MARK: - Request writer + Darwin hint

    private func writeRequest(_ request: EcholetIPC.KeyboardRequest) -> Bool {
        guard let data = try? EcholetIPC.makeEncoder().encode(request) else { return false }
        env.defaults.set(data, forKey: EcholetIPC.keyboardRequestKey)
        env.defaults.synchronize()
        postRequestHint()
        return true
    }

    private func postRequestHint() {
        let center = CFNotificationCenterGetDarwinNotifyCenter()
        let notificationName = CFNotificationName(EcholetIPC.darwinNotificationRequest as CFString)
        CFNotificationCenterPostNotification(center, notificationName, nil, nil, true)
    }

    // MARK: - Intent sequence allocation

    func reserveNextIntentSequence() -> UInt64? {
        let current = env.defaults.object(forKey: Self.intentSequenceStorageKey) as? UInt64 ?? 0
        guard current < UInt64.max else { return nil }
        let next = current + 1
        env.defaults.set(next, forKey: Self.intentSequenceStorageKey)
        env.defaults.synchronize()
        return next
    }

    // MARK: - Helpers

    private func setPhase(_ newPhase: Phase) {
        phase = newPhase
    }
}
