import Foundation

/// Pure Foundation orchestrator for the iOS "select installed model" flow
/// (containing app only).
///
/// Serialized, generation-fenced chain — no racy swap:
///
/// 1. Gate on OBSERVED truth (`EcholetModelGate`); a denial finishes with the
///    honest guidance and never touches audio or persisted state.
/// 2. Open an exclusive switch epoch: while it is open, new ASR sessions are
///    rejected upstream ("model_switch_in_progress") through the injected
///    `setBarrier` hook (the real App impl rejects beginSession/load).
/// 3. Persist the selection via the injected `rustSelect` (Rust semantics:
///    registry-validated, save-checked, and Rust itself keeps the previous
///    active id when validation or the config save fails).
/// 4. Swap the live recognizer via the injected `swapRecognizer`: it performs
///    create-then-swap serialized on the ASR worker queue (a stream teardown —
///    none by gate — must happen before the recognizer).
/// 5. On swap failure while persistence succeeded: restore the PREVIOUS
///    persisted selection via `rustRollback` and surface the honest error —
///    no phantom "ready" state and no config/recognizer mismatch.
/// 6. Close the barrier exactly once (success or failure).
///
/// Duplicate selects are fenced: while a switch epoch is open, later requests
/// are denied; the epoch closes exactly once regardless of outcome, and stale
/// async callbacks (older generation) may not mutate state.
public final class EcholetModelSwitchCoordinator {

    /// Immutable dependencies (the real App impl wires queues + Rust FFI).
    public struct Deps {
        /// Reads the current gate inputs at decision time.
        public let gateInputs: () -> EcholetModelGate.Inputs
        /// The currently ACTIVE (persisted) model id before the switch; the
        /// rollback target after a failed recognizer swap.
        public let previousActive: () -> String?
        /// Rust-side selection with persisted semantics. Fails on unknown id,
        /// uninstalled model, or failed config save.
        public let rustSelect: (String) -> Result<Void, EcholetModelError>
        /// Restores persistence to the prior model id after a failed swap.
        public let rustRollback: (String) -> Void
        /// Serialized recognizer swap (create-then-swap on the ASR worker
        /// queue). Called at most once per switch attempt.
        public let swapRecognizer: (String, @escaping (Bool, String?) -> Void) -> Void
        /// Opens/closes the barrier that rejects new ASR sessions while a
        /// switch is mid-flight. Closing happens exactly once.
        public let setBarrier: (Bool) -> Void

        public init(
            gateInputs: @escaping () -> EcholetModelGate.Inputs,
            previousActive: @escaping () -> String?,
            rustSelect: @escaping (String) -> Result<Void, EcholetModelError>,
            rustRollback: @escaping (String) -> Void,
            swapRecognizer: @escaping (String, @escaping (Bool, String?) -> Void) -> Void,
            setBarrier: @escaping (Bool) -> Void
        ) {
            self.gateInputs = gateInputs
            self.previousActive = previousActive
            self.rustSelect = rustSelect
            self.rustRollback = rustRollback
            self.swapRecognizer = swapRecognizer
            self.setBarrier = setBarrier
        }
    }

    /// Honest outcome for the UI.
    public enum Outcome: Equatable {
        case switched(selectedId: String)
        case denied(guidance: String)
        case failed(message: String)
    }

    private let deps: Deps
    /// Decision ownership: gate + epoch + completion all run HERE.
    private let decisionQueue = DispatchQueue(label: "echolet.model.switch.decision")
    /// Generation fencing: only the newest attempt may mutate state.
    private var currentToken: UInt64 = 0
    /// Exclusive switch epoch (opened/closed on decisionQueue only).
    private var epochOpen = false

    public init(deps: Deps) {
        self.deps = deps
    }

    /// Whether a switch epoch is open (new ASR starts must be rejected).
    public var isSwitchActive: Bool {
        return decisionQueue.sync { epochOpen }
    }

    // MARK: - Entry (any queue)

    /// Attempts a serialized select. `completion` runs on `decisionQueue`.
    public func select(
        modelId: String,
        modelDirectory: String,
        completion: @escaping (Outcome) -> Void
    ) {
        decisionQueue.async { [self] in
            guard !epochOpen else {
                // Concurrent duplicate: fenced out with honest guidance.
                completion(.denied(guidance: "A model selection is already in progress; wait for it to finish."))
                return
            }

            switch EcholetModelGate.evaluate(self.deps.gateInputs()) {
            case .deny(let guidance):
                completion(.denied(guidance: guidance))
                return
            case .allow:
                break
            }

            currentToken += 1
            let token = currentToken
            epochOpen = true
            self.deps.setBarrier(true)

            let previous = self.deps.previousActive()
            if previous == modelId {
                // Already selected and persisted: still prove the recognizer
                // really has this model loaded (id != loaded truth guard).
                self.deps.swapRecognizer(modelDirectory) { [self] ok, swapError in
                    self.decisionQueue.async { [self] in
                        guard token == currentToken else { return }
                        epochOpen = false
                        deps.setBarrier(false)
                        completion(ok
                            ? .switched(selectedId: modelId)
                            : .failed(message: swapError.map { "Model already selected but failed to reload: \($0)" } ?? "Model already selected but failed to reload"))
                    }
                }
                return
            }

            switch self.deps.rustSelect(modelId) {
            case .failure(let rustError):
                // Persistence itself failed; nothing swapped. Rust kept the
                // previous selection internally, so no rollback is needed.
                currentToken -= 1
                epochOpen = false
                deps.setBarrier(false)
                completion(.failed(message: rustError.message))
                return

            case .success:
                self.deps.swapRecognizer(modelDirectory) { [self] ok, swapError in
                    self.decisionQueue.async { [self] in
                        guard token == currentToken else { return }
                        epochOpen = false
                        deps.setBarrier(false)
                        if ok {
                            completion(.switched(selectedId: modelId))
                        } else {
                            // Recognizer swap failed while persistence
                            // succeeded: restore the PREVIOUS persisted
                            // selection so config matches the live (known
                            // good) loaded recognizer.
                            if let previous = previous {
                                self.deps.rustRollback(previous)
                            }
                            let message = swapError
                                .map { "Model could not be loaded after selection: \($0)" }
                                ?? "Model could not be loaded after selection"
                            completion(.failed(message: message))
                        }
                    }
                }
            }
        }
    }
}
