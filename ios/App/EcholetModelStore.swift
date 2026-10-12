import Foundation

/// App-owned single writer for ALL Rust model-manager C-ABI state
/// (containing app only; the Keyboard extension never touches models).
///
/// Ownership model:
/// * ONE serial `storeQueue` exclusively owns the Rust
///   `EcholetIosModelManager` handle — every snapshot/spec/install/select
///   call runs there; nothing else may touch the handle.
/// * The UI receives IMMUTABLE snapshots (`EcholetModelStore.Snapshot`,
///   verbatim from Rust) published on the main thread from real read-back —
///   installed/selected truth is never guessed: after an install or select
///   completes the store re-reads the Rust snapshot and publishes exactly
///   what Rust reports.
/// * Rust owns the SHA256 authority and the safe atomic installer; Swift only
///   orchestrates transport (URLSession in `EcholetModelDownloadCoordinator`).
/// * Paths: Application Support/Echolet/models (user dir), …/downloads and
///   …/config.json — all App-private sandbox paths, never the .app bundle,
///   and model data is excluded from iCloud backup.
public final class EcholetModelStore {

    public static let shared = EcholetModelStore()

    // MARK: - FFI (Rust echolet_ios bridge; linked into the app target only)

    @_silgen_name("echolet_ios_model_manager_new")
    private static func ffi_manager_new(_ userModelsDir: UnsafePointer<CChar>?, _ configPath: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer?
    @_silgen_name("echolet_ios_model_manager_free")
    private static func ffi_manager_free(_ handle: UnsafeMutableRawPointer?)
    @_silgen_name("echolet_ios_model_string_free")
    private static func ffi_string_free(_ string: UnsafeMutablePointer<CChar>?)
    @_silgen_name("echolet_ios_model_snapshot_json")
    private static func ffi_snapshot_json(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>?
    @_silgen_name("echolet_ios_model_download_spec_json")
    private static func ffi_download_spec_json(_ handle: UnsafeMutableRawPointer?, _ modelId: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?
    @_silgen_name("echolet_ios_model_error_json")
    private static func ffi_error_json(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>?
    @_silgen_name("echolet_ios_model_install_archive")
    private static func ffi_install_archive(_ handle: UnsafeMutableRawPointer?, _ modelId: UnsafePointer<CChar>?, _ archivePath: UnsafePointer<CChar>?, _ progress: (@convention(c) (UnsafeMutableRawPointer?, Int32) -> Void)?, _ userData: UnsafeMutableRawPointer?) -> Int32
    @_silgen_name("echolet_ios_model_select")
    private static func ffi_select(_ handle: UnsafeMutableRawPointer?, _ modelId: UnsafePointer<CChar>?) -> Int32
    @_silgen_name("echolet_ios_model_active_dir")
    private static func ffi_active_dir(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>?

    /// Rust install progress phase codes (mirrors ios/ios-native/src/model.rs).
    enum InstallPhaseCode {
        static let starting: Int32 = 0
        static let verifying: Int32 = 1
        static let extracting: Int32 = 2
        static let installing: Int32 = 3
        static let completed: Int32 = 4
    }

    // MARK: - Immutable snapshot types (published to the UI)

    public struct Entry: Equatable {
        public let id: String
        public let displayName: String
        public let version: String
        public let languages: [String]
        public let verificationStatus: String
        public let runtimeCompatible: Bool
        public let installed: Bool
        public let installedDir: String?
        public let active: Bool
        public let downloadSizeBytes: UInt64?
        public let installedSizeBytes: UInt64?

        public init(
            id: String,
            displayName: String,
            version: String,
            languages: [String],
            verificationStatus: String,
            runtimeCompatible: Bool,
            installed: Bool,
            installedDir: String?,
            active: Bool,
            downloadSizeBytes: UInt64?,
            installedSizeBytes: UInt64?
        ) {
            self.id = id
            self.displayName = displayName
            self.version = version
            self.languages = languages
            self.verificationStatus = verificationStatus
            self.runtimeCompatible = runtimeCompatible
            self.installed = installed
            self.installedDir = installedDir
            self.active = active
            self.downloadSizeBytes = downloadSizeBytes
            self.installedSizeBytes = installedSizeBytes
        }
    }

    public struct Snapshot: Equatable {
        public let defaultModelId: String
        public let entries: [Entry]

        public init(defaultModelId: String, entries: [Entry]) {
            self.defaultModelId = defaultModelId
            self.entries = entries
        }

        public var anyInstalled: Bool {
            return entries.contains { $0.installed }
        }

        public var activeEntry: Entry? {
            return entries.first { $0.active }
        }

        public func entry(id: String) -> Entry? {
            return entries.first { $0.id == id }
        }
    }

    /// Canonical HTTPS download spec returned by Rust (registry-validated).
    public struct DownloadSpec: Equatable {
        public let modelId: String
        public let url: URL
        public let sha256: String
        public let downloadSizeBytes: UInt64?
    }

    // MARK: - State (queue ownership is explicit)

    /// storeQueue-authored Rust truth (handle + last snapshot).
    private var handle: UnsafeMutableRawPointer?
    private var privateSnapshot: Snapshot?
    private var privateActiveDirectory: String?

    /// Main-thread mirrors (mutated only on main).
    public private(set) var snapshot: Snapshot?
    public private(set) var ready = false
    /// Live transfer states per model id (main thread only).
    public var transferStates: [String: EcholetHomePresenter.ModelTransferState] = [:]

    /// Main-thread observer for snapshot changes (Home panel).
    public var snapshotObserver: ((_ snapshot: Snapshot) -> Void)?

    /// Serial single-writer queue owning the Rust manager handle.
    private let storeQueue = DispatchQueue(label: "com.echolet.model.store", qos: .userInitiated)

    /// Serialized selection orchestrator (gate/barrier/fencing/rollback).
    private var switchCoordinator: EcholetModelSwitchCoordinator?

    private init() {}

    // MARK: - Paths (App-private sandbox, never the .app bundle)

    public static func modelsDirectory() -> URL {
        return applicationSupportDirectory()
            .appendingPathComponent("Echolet", isDirectory: true)
            .appendingPathComponent("models", isDirectory: true)
    }

    public static func downloadsDirectory() -> URL {
        return applicationSupportDirectory()
            .appendingPathComponent("Echolet", isDirectory: true)
            .appendingPathComponent("downloads", isDirectory: true)
    }

    public static func configPath() -> URL {
        return applicationSupportDirectory()
            .appendingPathComponent("Echolet", isDirectory: true)
            .appendingPathComponent("config.json")
    }

    private static func applicationSupportDirectory() -> URL {
        let urls = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)
        return urls.first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
    }

    // MARK: - Bootstrap

    /// Creates the sandbox directories, the Rust manager, and publishes the
    /// first honest snapshot. Main-thread entry point.
    public func bootstrap() {
        storeQueue.async { [weak self] in
            guard let self = self, self.handle == nil else { return }

            let fm = FileManager.default
            do {
                try fm.createDirectory(at: Self.modelsDirectory(), withIntermediateDirectories: true)
                try fm.createDirectory(at: Self.downloadsDirectory(), withIntermediateDirectories: true)
            } catch {
                print("[EcholetModelStore] FATAL: cannot create sandbox dirs: \(error)")
                return
            }
            // Model bytes must never be mirrored into iCloud backup.
            for dir in [Self.modelsDirectory(), Self.downloadsDirectory()] {
                Self.excludeFromBackup(url: dir)
            }

            let modelsPath = Self.modelsDirectory().path
            let configP = Self.configPath().path
            let created = modelsPath.withCString { dirPtr -> UnsafeMutableRawPointer? in
                configP.withCString { configPtr in
                    Self.ffi_manager_new(dirPtr, configPtr)
                }
            }
            guard let handle = created else {
                print("[EcholetModelStore] FATAL: Rust model manager bootstrap failed")
                return
            }
            self.handle = handle

            // Wire the selection orchestrator (queue-aware; see below).
            self.wireSwitchCoordinator()

            // First truthful read-back + publish.
            self.refreshSnapshotLocked()
            let activeDir = self.privateActiveDirectory

            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.ready = true
                // Publish ONE truthful active-dir to the ASR service.
                EcholetAsrService.shared.publishActiveModelDirectory(activeDir)
            }
        }
    }

    static func excludeFromBackup(url: URL) {
        do {
            try (url as NSURL).setResourceValue(true, forKey: URLResourceKey.isExcludedFromBackupKey)
        } catch {
            print("[EcholetModelStore] backup exclusion error for \(url.path): \(error)")
        }
    }

    // MARK: - Snapshot read-back (storeQueue locked only)

    /// Re-reads the Rust snapshot, records the active truth, and mirrors the
    /// immutable value to main. MUST run on storeQueue with a live handle.
    private func refreshSnapshotLocked() {
        guard let handle = handle else { return }
        let cString = Self.ffi_snapshot_json(handle)
        defer { Self.ffi_string_free(cString) }
        guard let decoded = decodeSnapshot(from: cString) else {
            print("[EcholetModelStore] snapshot decode failure")
            return
        }
        privateSnapshot = decoded.snapshot
        privateActiveDirectory = {
            let dirString = Self.ffi_active_dir(handle)
            defer { Self.ffi_string_free(dirString) }
            return dirString.map { String(cString: $0) }
        }()

        let snapshot = decoded.snapshot
        let activeDirectory = privateActiveDirectory
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.snapshot = snapshot
            self.snapshotObserver?(snapshot)
            EcholetAsrService.shared.publishActiveModelDirectory(activeDirectory)
        }
    }

    private struct DecodedSnapshot {
        var snapshot: Snapshot
    }

    private func decodeSnapshot(from pointer: UnsafeMutablePointer<CChar>?) -> DecodedSnapshot? {
        guard let pointer = pointer else { return nil }
        let data = Data(bytes: pointer, count: strlen(pointer))
        guard
            let object = (try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])) as? [String: Any]
        else { return nil }
        let defaultId = object["default_model_id"] as? String ?? ""
        var entries: [Entry] = []
        if let models = object["models"] as? [[String: Any]] {
            for m in models {
                let id = m["id"] as? String ?? ""
                entries.append(
                    Entry(
                        id: id,
                        displayName: m["display_name"] as? String ?? id,
                        version: m["version"] as? String ?? "",
                        languages: m["languages"] as? [String] ?? [],
                        verificationStatus: m["verification_status"] as? String ?? "",
                        runtimeCompatible: (m["runtime_compatible"] as? NSNumber)?.boolValue ?? false,
                        installed: (m["installed"] as? NSNumber)?.boolValue ?? false,
                        installedDir: m["installed_dir"] as? String,
                        active: (m["active"] as? NSNumber)?.boolValue ?? false,
                        downloadSizeBytes: (m["download_size_bytes"] as? NSNumber)?.uint64Value,
                        installedSizeBytes: (m["installed_size_bytes"] as? NSNumber)?.uint64Value
                    )
                )
            }
        }
        return DecodedSnapshot(snapshot: Snapshot(defaultModelId: defaultId, entries: entries))
    }

    // MARK: - Download spec (registry-validated; Swift never invents URLs)

    /// Fetches the Rust canonical download spec on its serial queue.
    public func downloadSpec(
        for modelId: String,
        completion: @escaping (Result<DownloadSpec, EcholetModelError>) -> Void
    ) {
        storeQueue.async { [weak self] in
            guard let self = self, let handle = self.handle else {
                completion(.failure(EcholetModelError(message: "Model store unavailable")))
                return
            }
            let failure = {
                EcholetModelError(message: self.lastErrorLocked(handle) ?? "download spec unavailable")
            }
            let spec = modelId.withCString { idPtr -> DownloadSpec? in
                let ptr = Self.ffi_download_spec_json(handle, idPtr)
                defer { Self.ffi_string_free(ptr) }
                guard let ptr = ptr else { return nil }
                let data = Data(bytes: ptr, count: strlen(ptr))
                guard
                    let obj = (try? JSONSerialization.jsonObject(with: data, options: .fragmentsAllowed)) as? [String: Any],
                    let id = obj["model_id"] as? String,
                    let urlString = obj["url"] as? String,
                    let url = URL(string: urlString),
                    let sha = obj["sha256"] as? String
                else { return nil }
                return DownloadSpec(
                    modelId: id,
                    url: url,
                    sha256: sha,
                    downloadSizeBytes: (obj["download_size_bytes"] as? NSNumber)?.uint64Value
                )
            }
            guard let spec = spec else {
                completion(.failure(failure()))
                return
            }
            completion(.success(spec))
        }
    }

    // MARK: - Install (downloaded archive → Rust verify → atomic install)

    /// Installs a fully downloaded archive via Rust (SHA256 authority) into
    /// the sandbox models dir. Progress flips the model's transfer state on
    /// main; Installed is pronounced ONLY after Rust's verify + atomic commit
    /// returned success AND a fresh Rust snapshot confirms it. The download
    /// archive is deleted by the caller's coordinator on failure and after
    /// success (cleanup contract stays truthful).
    /// `completion` runs on main.
    public func installFromArchive(
        modelId: String,
        archivePath: URL,
        completion: @escaping (Result<Void, EcholetModelError>) -> Void
    ) {
        // One retained id box handed to the Rust progress callback.
        let context = Unmanaged.passRetained(InstallContext(modelId: modelId))
        storeQueue.async { [weak self] in
            let finish: () -> Void = { context.release() }
            guard let self = self, let handle = self.handle else {
                finish()
                completion(.failure(EcholetModelError(message: "Model store unavailable")))
                return
            }
            modelId.withCString { idPtr in
                archivePath.path.withCString { pathPtr -> Void in
                    let rc = Self.ffi_install_archive(
                        handle, idPtr, pathPtr,
                        { userData, phase in
                            guard let userData = userData else { return }
                            let boxed = Unmanaged<EcholetModelStore.InstallContext>
                                .fromOpaque(userData).takeUnretainedValue()
                            EcholetModelStore.handleInstallProgress(phase, modelId: boxed.modelId)
                        },
                        context.toOpaque()
                    )
                    let error = self.lastErrorLocked(handle)
                    if rc == 0 {
                        // Truth read-back: no premature Installed.
                        self.refreshSnapshotLocked()
                        DispatchQueue.main.async {
                            self.transferStates[modelId] = .installed
                            completion(.success(()))
                        }
                    } else {
                        DispatchQueue.main.async {
                            let message = error ?? "install failed"
                            self.transferStates[modelId] = .failed(message: message)
                            completion(.failure(EcholetModelError(message: message)))
                        }
                    }
                }
            }
            finish()
        }
    }

    final class InstallContext {
        let modelId: String

        init(modelId: String) {
            self.modelId = modelId
        }
    }

    /// Rust progress callback hop (any thread → main transfer state).
    static func handleInstallProgress(_ phase: Int32, modelId: String) {
        DispatchQueue.main.async {
            switch phase {
            case InstallPhaseCode.verifying:
                shared.transferStates[modelId] = .verifying
            case InstallPhaseCode.extracting, InstallPhaseCode.installing, InstallPhaseCode.completed:
                shared.transferStates[modelId] = .installing
            default:
                break
            }
        }
    }

    private func lastErrorLocked(_ handle: UnsafeMutableRawPointer) -> String? {
        let ptr = Self.ffi_error_json(handle)
        defer { Self.ffi_string_free(ptr) }
        guard let ptr = ptr else { return nil }
        let data = Data(bytes: ptr, count: strlen(ptr))
        guard
            let obj = (try? JSONSerialization.jsonObject(with: data, options: .fragmentsAllowed)) as? [String: Any]
        else { return nil }
        return obj["error"] as? String
    }

    // MARK: - Select (gated, fenced, rolled-back)

    /// Main-thread entry for "Select" on an installed row. The gate denies
    /// honestly while audio sessions/warm hardware/loads are live and does
    /// NOT implicitly terminate dictation (guidance says what to stop first).
    public func select(modelId: String, completion: @escaping (EcholetModelSwitchCoordinator.Outcome) -> Void) {
        storeQueue.async { [weak self] in
            guard let self = self else { return }
            guard let coordinator = self.switchCoordinator else {
                completion(.failed(message: "Model store unavailable"))
                return
            }
            // Resolve the real installed directory from Rust truth (never a
            // Swift-side assumed path).
            guard let entry = self.privateSnapshot?.entry(id: modelId),
                  entry.installed, let installedDir = entry.installedDir else {
                completion(.failed(message: "Model \(modelId) is not installed"))
                return
            }
            coordinator.select(modelId: modelId, modelDirectory: installedDir) { outcome in
                // Post-switch truth read-back happens back on the store's
                // queue (the Rust handle's single writer); completion on main.
                self.storeQueue.async {
                    self.refreshSnapshotLocked()
                    DispatchQueue.main.async {
                        completion(outcome)
                    }
                }
            }
        }
    }

    /// Wire-up with the REAL queue-aware dependencies. storeQueue only.
    ///
    /// The coordinator runs its decision logic on its decisionQueue; the
    /// store-queue-owned Rust handle ops hop back here via `storeQueue.sync`
    /// (no inversion: storeQueue never synchronously blocks on the
    /// decisionQueue — `coordinator.select` is fire-and-forget).
    private func wireSwitchCoordinator() {
        guard switchCoordinator == nil else { return }
        let deps = EcholetModelSwitchCoordinator.Deps(
            gateInputs: { [weak self] in
                guard let self = self else {
                    return EcholetModelGate.Inputs(selectionAlreadyActive: true)
                }
                return self.externalGateInputs()
            },
            previousActive: { [weak self] in
                guard let self = self else { return nil }
                return self.storeQueue.sync { self.privateSnapshot?.activeEntry?.id }
            },
            rustSelect: { [weak self] id -> Result<Void, EcholetModelError> in
                guard let self = self else {
                    return .failure(EcholetModelError(message: "Model store unavailable"))
                }
                return self.storeQueue.sync { self.performRustSelect(id) }
            },
            rustRollback: { [weak self] id in
                // Best-effort restore of the previous persisted selection.
                guard let self = self else { return }
                let failure = self.storeQueue.sync { self.performRustSelect(id) }
                if case .failure(let error) = failure {
                    print("[EcholetModelStore] rollback failed for \(id): \(error.message)")
                }
            },
            swapRecognizer: { directory, done in
                EcholetAsrService.shared.swapRecognizer(toDirectory: directory, done: done)
            },
            setBarrier: { on in
                EcholetAsrService.shared.setSwitchBarrier(on)
            }
        )
        self.switchCoordinator = EcholetModelSwitchCoordinator(deps: deps)
    }

    /// Blocking-truth snapshot for the model-select gate (queue-safe reads).
    private func externalGateInputs() -> EcholetModelGate.Inputs {
        return EcholetModelGate.Inputs(
            keyboardSessionActive: WarmIPCService.shared.hasActiveKeyboardSession,
            keyboardActivationPending: WarmIPCService.shared.hasPendingActivation,
            warmHardwareActive: WarmIPCService.shared.warmStandbyHardwareActive,
            micUserArmed: WarmIPCService.shared.isUserArmed,
            manualCaptureRecording: AudioCaptureController.shared.recordingFromAnyPathWindow,
            asrBusy: EcholetAsrService.shared.isBusyForModelSwitch,
            selectionAlreadyActive: false // epoch duplicates are fenced by the coordinator itself
        )
    }

    /// Synchronous Rust persisted select. storeQueue ONLY.
    private func performRustSelect(_ id: String) -> Result<Void, EcholetModelError> {
        guard let handle = handle else {
            return .failure(EcholetModelError(message: "Model store unavailable"))
        }
        return id.withCString { idPtr -> Result<Void, EcholetModelError> in
            let rc = Self.ffi_select(handle, idPtr)
            if rc == 0 { return .success(()) }
            let message = lastErrorLocked(handle) ?? "select failed"
            return .failure(EcholetModelError(message: message))
        }
    }
}
