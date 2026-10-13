import Foundation
import UIKit

/// App-owned URLSession download transport for canonical model archives
/// (containing app only).
///
/// Guarantees:
/// * Only the HTTPS URL delivered by the Rust canonical download spec is
///   fetched (no Swift-side invented sources); redirects are followed ONLY
///   when they stay HTTPS.
/// * Disk streaming via URLSessionDownloadTask — the body is never buffered
///   into memory, and staging lives in the App-private downloads directory
///   (never the .app bundle, never the keyboard extension container).
/// * The expected/received archive size is bounded by
///   `EcholetModelDownloadPolicy.maxArchiveBytes`; out-of-space and oversize
///   errors cancel with an honest message.
/// * Download success is NOT "Installed": the finished body is moved to its
///   one-shot staging path and handed to
///   `EcholetModelStore.installFromArchive` (Rust SHA256 verify + atomic
///   install); the staging archive is deleted in BOTH outcomes.
/// * Duplicate Download taps are coalesced; failures surface as truthful
///   transfer states with a Download/Retry action; a retry starts a fresh
///   clean attempt (no partial-body resume).
/// * Reality is reported: background suspension of URLSession tasks is a
///   platform truth; the progress state only reflects observed task events.
public final class EcholetModelDownloadCoordinator: NSObject {

    public static let shared = EcholetModelDownloadCoordinator()

    /// Serial delegate/bookkeeping queue (all in-flight state owned here).
    private let delegateQueue: OperationQueue = {
        let q = OperationQueue()
        q.maxConcurrentOperationCount = 1
        q.qualityOfService = .utility
        q.name = "com.echolet.model.download.delegate"
        return q
    }()

    private lazy var session: URLSession = {
        URLSession(configuration: .ephemeral, delegate: self, delegateQueue: delegateQueue)
    }()

    private var transfers: [String: Transfer] = [:]

    private override init() {
        super.init()
    }

    final class Transfer {
        let modelId: String
        let spec: EcholetModelStore.DownloadSpec
        var task: URLSessionDownloadTask?

        init(modelId: String, spec: EcholetModelStore.DownloadSpec) {
            self.modelId = modelId
            self.spec = spec
        }
    }

    // MARK: - Entry (main thread)

    /// Starts the full honest pipeline for one canonical model id:
    /// Download → Verifying → Installing → Installed. NULL result wait is
    /// not needed: Home re-renders from the store's truthful transfer states.
    public func download(modelId: String) {
        // Duplicate-tap coalesce FIRST (honest current-state check).
        guard !isCoalescingCopy(modelId: modelId) else { return }
        // Visible immediately: the row must leave "Download" before the
        // spec/network round trip, or the tap looks like it did nothing.
        EcholetModelStore.shared.transferStates[modelId] =
            .downloading(downloadedBytes: 0, totalBytes: nil)

        EcholetModelStore.shared.downloadSpec(for: modelId) { [weak self] result in
            DispatchQueue.main.async {
                switch result {
                case .success(let spec):
                    self?.beginTransferOnDelegateQueue(modelId: modelId, spec: spec)
                case .failure(let error):
                    EcholetModelStore.shared.transferStates[modelId] = .failed(message: error.message)
                }
            }
        }
    }

    private func isCoalescingCopy(modelId: String) -> Bool {
        let state = EcholetModelStore.shared.transferStates[modelId]
        return EcholetModelDownloadPolicy.beginDecision(for: state) == .coalesce
    }

    private func beginTransferOnDelegateQueue(modelId: String, spec: EcholetModelStore.DownloadSpec) {
        delegateQueue.addOperation { [weak self] in
            guard let self = self else { return }

            // Late duplicate (another flow registered meanwhile).
            if self.transfers[modelId] != nil { return }

            // Free-space pre-flight on the FULL lifecycle footprint.
            let verdict = EcholetModelDownloadPolicy.spaceVerdict(
                availableBytes: Self.freeDiskBytes(),
                downloadSizeBytes: spec.downloadSizeBytes,
                installedSizeBytes: EcholetModelStore.shared.snapshot?.entry(id: modelId)?.installedSizeBytes
            )
            if verdict == .insufficient {
                DispatchQueue.main.sync {
                    EcholetModelStore.shared.transferStates[modelId] =
                        .failed(message: "Not enough free space for this model and its install.")
                }
                return
            }

            let destination = EcholetModelDownloadPolicy.archiveDestination(
                downloadsDirectory: EcholetModelStore.downloadsDirectory(),
                modelId: modelId
            )
            // Fresh clean attempt: never resume a discarded partial body.
            try? FileManager.default.removeItem(at: destination)

            var request = URLRequest(url: spec.url)
            request.httpMethod = "GET"
            let transfer = Transfer(modelId: modelId, spec: spec)
            let task = self.session.downloadTask(with: request)
            task.taskDescription = modelId
            transfer.task = task
            self.transfers[modelId] = transfer
            let advertisedTotal = spec.downloadSizeBytes
            DispatchQueue.main.async {
                if case .downloading(let done, _) = EcholetModelStore.shared.transferStates[modelId] {
                    EcholetModelStore.shared.transferStates[modelId] = .downloading(
                        downloadedBytes: done,
                        totalBytes: advertisedTotal
                    )
                }
            }
            task.resume()
        }
    }

    // MARK: - delegateQueue internals

    private func transferFor(_ task: URLSessionTask) -> Transfer? {
        guard let id = task.taskDescription else { return nil }
        return transfers[id]
    }

    private func dropTransfer(_ transfer: Transfer) {
        transfers.removeValue(forKey: transfer.modelId)
    }

    /// Moves the completed temp body to its one-shot staging path and hands
    /// it to the Rust-verified installer. Runs on delegateQueue.
    private func installFinishedTransfer(_ transfer: Transfer, location: URL) {
        let destination = EcholetModelDownloadPolicy.archiveDestination(
            downloadsDirectory: EcholetModelStore.downloadsDirectory(),
            modelId: transfer.modelId
        )
        try? FileManager.default.removeItem(at: destination)
        do {
            try FileManager.default.moveItem(at: location, to: destination)
        } catch {
            try? FileManager.default.removeItem(at: location)
            dropTransfer(transfer)
            let message = "Could not stage downloaded archive: \(error.localizedDescription)"
            publishFailure(transfer.modelId, message: message)
            return
        }

        EcholetModelStore.shared.installFromArchive(
            modelId: transfer.modelId,
            archivePath: destination
        ) { [weak self] _ in
            // The staging archive is a one-shot artifact either way.
            try? FileManager.default.removeItem(at: destination)
            guard let self = self else { return }
            self.delegateQueue.addOperation {
                self.dropTransfer(transfer)
            }
        }
    }

    private func publishFailure(_ modelId: String, message: String) {
        DispatchQueue.main.async {
            EcholetModelStore.shared.transferStates[modelId] = .failed(message: message)
        }
    }

    static func freeDiskBytes() -> UInt64? {
        let fm = FileManager.default
        let attributes = (try? fm.attributesOfFileSystem(
            forPath: NSHomeDirectory()
        )) as [FileAttributeKey: Any]?
        return attributes?[.systemFreeSize] as? UInt64
    }
}

// MARK: - URLSessionDownloadDelegate (runs on delegateQueue)

extension EcholetModelDownloadCoordinator: URLSessionDownloadDelegate {

    public func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        // Default handling only — trusted system certificate validation.
        completionHandler(.performDefaultHandling, nil)
    }

    /// HTTPS-only redirects (conservative, verified against policy).
    public func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        guard let transfer = transferFor(task) else {
            completionHandler(nil)
            return
        }
        guard let target = request.url,
              EcholetModelDownloadPolicy.allowsRedirect(url: target) else {
            dropTransfer(transfer)
            publishFailure(
                transfer.modelId,
                message: "Redirect to non-HTTPS target refused."
            )
            completionHandler(nil)
            return
        }
        completionHandler(request)
    }

    public func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64,
        totalBytesExpectedToWrite: Int64
    ) {
        guard let transfer = transferFor(downloadTask) else { return }
        // Hard cap enforcement on actually received bytes.
        if totalBytesWritten > EcholetModelDownloadPolicy.maxArchiveBytes {
            dropTransfer(transfer)
            downloadTask.cancel()
            publishFailure(
                transfer.modelId,
                message: "Archive exceeds the 2 GiB product limit."
            )
            return
        }
        let modelId = transfer.modelId
        let state = EcholetModelDownloadPolicy.progressState(
            bytesWritten: UInt64(totalBytesWritten),
            totalBytesExpected: totalBytesExpectedToWrite > 0
                ? UInt64(totalBytesExpectedToWrite)
                : nil
        )
        DispatchQueue.main.async {
            // Only while still in the downloading phase: a finished canceled
            // flow must not be resurrected by a stray tick.
            if case .downloading = EcholetModelStore.shared.transferStates[modelId] {
                EcholetModelStore.shared.transferStates[modelId] = state
            }
        }
    }

    public func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didFinishDownloadingTo location: URL
    ) {
        guard let transfer = transferFor(downloadTask) else {
            try? FileManager.default.removeItem(at: location)
            return
        }
        let statusCode = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200...299).contains(statusCode) else {
            try? FileManager.default.removeItem(at: location)
            dropTransfer(transfer)
            publishFailure(transfer.modelId, message: "Download failed (HTTP \(statusCode)).")
            return
        }
        installFinishedTransfer(transfer, location: location)
    }

    public func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        guard let transfer = transferFor(task) else { return }
        guard let error = error as NSError? else {
            // Success termination; the body was already handled in
            // didFinishDownloadingTo.
            return
        }
        let userCancelled = error.domain == NSURLErrorDomain
            && error.code == NSURLErrorCancelled
        if userCancelled {
            // Cancel = fresh attempt later; honest state repair here.
            dropTransfer(transfer)
            return
        }
        dropTransfer(transfer)
        publishFailure(
            transfer.modelId,
            message: "Download failed: \(error.localizedDescription)."
        )
    }
}
