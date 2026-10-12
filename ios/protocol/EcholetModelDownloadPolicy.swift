import Foundation

/// Pure Foundation policy for the iOS model download coordinator (app-owned).
///
/// Latent transport (URLSession) is glued in the App target; every DECISION
/// lives here so it is CLI-testable without network:
/// * duplicate taps are coalesced while a transfer is in flight;
/// * expected archive size is bounded (2 GiB hard product cap);
/// * free-space is checked against the FULL projected footprint
///   (archive + extracted install + margin) when both are known;
/// * downloads succeed into app-private storage only; retry begins a fresh
///   clean attempt (a partial body is never trusted — Rust owns the hash).
public enum EcholetModelDownloadPolicy {

    /// Hard product cap for a downloaded model archive (2 GiB).
    public static let maxArchiveBytes: UInt64 = 2 * 1024 * 1024 * 1024

    /// Disk margin required above (archive + installed) before starting.
    public static let spaceMarginBytes: UInt64 = 64 * 1024 * 1024

    public enum BeginDecision: Equatable {
        case begin
        case coalesce           // already downloading/verifying/installing
    }

    /// Whether a Download tap may begin a fresh transfer right now.
    public static func beginDecision(for state: EcholetHomePresenter.ModelTransferState?) -> BeginDecision {
        switch state {
        case .none, .idle, .failed, .installed:
            return .begin
        case .downloading, .verifying, .installing:
            return .coalesce
        }
    }

    public enum SizeVerdict: Equatable {
        case ok
        case tooLarge(limit: UInt64)
    }

    /// Validates the expected Content-Length against the product cap.
    /// A server that reports nothing at all is fine (the cap is re-checked on
    /// actual bytes received; the archive is never trusted anyway).
    public static func sizeVerdict(expectedTotalBytes: UInt64?) -> SizeVerdict {
        guard let total = expectedTotalBytes else { return .ok }
        if total > maxArchiveBytes {
            return .tooLarge(limit: maxArchiveBytes)
        }
        return .ok
    }

    public enum SpaceVerdict: Equatable {
        case ok
        case insufficient
        /// Free space unknown: proceed and let the filesystem surface
        /// errors honestly (never invent an error silently).
        case unknownLedAllowed
    }

    /// Conservative pre-flight: the FULL lifecycle footprint must fit
    /// (archive bytes + extracted install bytes + margin).
    public static func spaceVerdict(
        availableBytes: UInt64?,
        downloadSizeBytes: UInt64?,
        installedSizeBytes: UInt64?
    ) -> SpaceVerdict {
        guard let available = availableBytes else { return .unknownLedAllowed }
        guard let download = downloadSizeBytes, let installed = installedSizeBytes else {
            // Sizes unknown: require at least the advertised download + margin.
            guard let download = downloadSizeBytes else { return .unknownLedAllowed }
            return available > download + spaceMarginBytes ? .ok : .insufficient
        }
        let required = download + installed + spaceMarginBytes
        return available >= required ? .ok : .insufficient
    }

    /// Honest progress state for an observed write tick.
    public static func progressState(
        bytesWritten: UInt64,
        totalBytesExpected: UInt64?
    ) -> EcholetHomePresenter.ModelTransferState {
        if let total = totalBytesExpected, total >= bytesWritten, total > 0 {
            return .downloading(downloadedBytes: bytesWritten, totalBytes: total)
        }
        return .downloading(downloadedBytes: bytesWritten, totalBytes: totalBytesExpected)
    }

    /// Local staging file for one model's download (app-private; NEVER inside
    /// the .app bundle or the keyboard extension container).
    public static func archiveDestination(downloadsDirectory: URL, modelId: String) -> URL {
        let sanitized = modelId.map { c -> Character in
            (c.isLetter || c.isNumber || c == "-" || c == "_") ? c : "_"
        }
        return downloadsDirectory.appendingPathComponent(String(sanitized) + ".archive")
    }

    /// Whether a redirect target may be followed (HTTPS-only, conservative).
    public static func allowsRedirect(url: URL) -> Bool {
        return url.scheme?.lowercased() == "https"
    }
}
