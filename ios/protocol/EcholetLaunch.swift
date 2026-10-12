import Foundation

/// Echolet keyboard <-> containing app launch handshake (pure Foundation).
///
/// Cross-process records persisted in the shared App Group UserDefaults:
/// 1. `DemandRecord`  — written by the keyboard right before it attempts to
///    open the containing app via the `echolet://` URL scheme. One URL has a
///    single nonce; the demand record carries it plus a short TTL.
/// 2. `PhaseRecord`   — written ONLY by the containing app (through
///    `WarmIPCService.ipcQueue`) as observable dictation-prepare progress:
///    uriReceived -> modelLoaded -> warmReady (native mic confirmed), or a
///    truthful failed phase. Lets an old UI snapshot never be mistaken for a
///    live app because it is nonce- and epoch-fenced and short-lived.
/// 3. `WarmLiveRecord` — written ONLY by the containing app on ipcQueue when
///    the native recorder hardware start/stop truth changes. `freshWarmLive`
///    is the keyboard's authority for "a positively warm mic exists": record
///    age within the freshness window AND app epoch matches the currently
///    published process epoch.
///
/// The URL format is `echolet://dictation/prepare?nonce=<UUID>&ts=<ms>`.
/// Everything here is testable on macOS via `swiftc` CLI without UIKit.
public enum EcholetLaunch {

    // MARK: - Constants

    public static let urlScheme = "echolet"
    public static let demandKey = "echolet.keyboard.launch.demand.v2"
    public static let phaseKey = "echolet.app.launch.phase.v2"
    public static let warmLiveKey = "echolet.app.warm.live.v2"

    /// Demand record lives slightly longer than a full cold launch requires.
    public static let defaultDemandTtlMs: UInt64 = 45_000
    /// Phase observations older than this no longer describe a live prepare.
    public static let defaultPhaseTtlMs: UInt64 = 45_000
    /// A WarmLive record must be at most this old to count as a genuinely
    /// live recorder (hardware start/stop events are the only writers).
    public static let defaultWarmLiveFreshnessMs: UInt64 = 2_500

    /// Host return context tied strictly to a launch nonce. Written by the
    /// keyboard immediately before opening the containing app; read by the app
    /// after MIC_NATIVE_READY to attempt returning the user to the host app.
    public static let hostReturnContextKey = "echolet.keyboard.host.return.context.v2"
    public static let defaultHostReturnTtlMs: UInt64 = 60_000

    /// Explicit user-initiated Start intent. Written ONLY when the user taps
    /// "Start Listening" and triggers a cold app launch. Enables the keyboard
    /// to resume and automatically transition to listening upon returning to host.
    public static let pendingStartIntentKey = "echolet.keyboard.pending.start.intent.v1"
    public static let defaultPendingStartIntentTtlMs: UInt64 = 60_000

    /// Legacy host bundle id key (maintained for backward compatibility)
    public static let hostBundleIdKey = "echolet.keyboard.host.bundleid.v1"

    // MARK: - Host Return Context (keyboard-written, nonce-bound)

    public struct HostReturnContext: Codable, Equatable {
        public let launchNonce: String
        public let hostBundleId: String
        public let capturedAtMs: UInt64

        public init(launchNonce: String, hostBundleId: String, capturedAtMs: UInt64) {
            self.launchNonce = launchNonce
            self.hostBundleId = hostBundleId
            self.capturedAtMs = capturedAtMs
        }
    }

    // MARK: - Pending Start Intent (user-tap initiated, recoverable handoff)

    public struct PendingStartIntent: Codable, Equatable {
        public let launchNonce: String
        public let initiatedAtMs: UInt64
        public let ttlMs: UInt64

        public init(launchNonce: String, initiatedAtMs: UInt64, ttlMs: UInt64 = defaultPendingStartIntentTtlMs) {
            self.launchNonce = launchNonce
            self.initiatedAtMs = initiatedAtMs
            self.ttlMs = ttlMs
        }
    }

    // MARK: - Host bundle id helpers

    /// Guards against nil, empty, or KVC "<null>" markers. Returns the
    /// normalized id or nil.
    public static func normalizeHostBundleId(_ raw: String?) -> String? {
        guard var id = raw else { return nil }
        id = id.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty, id != "<null>", id != "(null)" else { return nil }
        return id
    }

    /// A bundle id we are willing to bring forward with
    /// `openApplicationWithBundleID:`. Rejects empty values, process
    /// identifiers, SpringBoard / WebKit helpers, and our own bundles.
    /// `com.apple.Maps` and `com.google.chrome.ios` both pass.
    public static func isReturnableHostBundleId(_ raw: String?) -> Bool {
        guard let id = normalizeHostBundleId(raw) else { return false }
        let labels = id.split(separator: ".", omittingEmptySubsequences: false)
        guard labels.count >= 3 else { return false }
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-"))
        for label in labels {
            guard !label.isEmpty, label.unicodeScalars.allSatisfy({ allowed.contains($0) }) else {
                return false
            }
        }
        let lower = id.lowercased()
        if lower.contains("echolet") { return false }
        let blockedPrefixes = [
            "com.apple.springboard",
            "com.apple.backboard",
            "com.apple.frontboard",
            "com.apple.webkit"
        ]
        if blockedPrefixes.contains(where: { lower.hasPrefix($0) }) { return false }
        return true
    }

    /// Keyboard-resolved host first, then the URL source application, then a
    /// bundle id actually read from the system back-badge. A visible
    /// "◀ Chrome" label is not itself a bundle id, so the badge is last.
    /// Duplicates and non-returnable ids are dropped.
    public static func orderedReturnHosts(breadcrumb: String?, storedHost: String?, sourceApp: String?) -> [String] {
        var seen = Set<String>()
        var ordered: [String] = []
        for raw in [storedHost, sourceApp, breadcrumb] {
            guard let id = normalizeHostBundleId(raw), isReturnableHostBundleId(id) else { continue }
            if seen.insert(id).inserted {
                ordered.append(id)
            }
        }
        return ordered
    }

    public static let manualReturnHint = "Recording is ready. Tap ◀ Previous App to continue."

    /// Hosts we can try to foreground. The URL is only `scheme://` so Chrome
    /// is brought forward without opening a new tab.
    public struct KnownReturnHost: Equatable {
        public let name: String
        public let bundleId: String
        public let urlScheme: String

        public var returnURL: URL? { URL(string: "\(urlScheme)://") }
    }

    public static let knownReturnHosts: [KnownReturnHost] = [
        KnownReturnHost(name: "Apple Maps", bundleId: "com.apple.Maps", urlScheme: "maps"),
        KnownReturnHost(name: "Google Chrome", bundleId: "com.google.chrome.ios", urlScheme: "googlechrome"),
        KnownReturnHost(name: "Apple Files", bundleId: "com.apple.DocumentsApp", urlScheme: "shareddocuments"),
        KnownReturnHost(name: "ChatGPT", bundleId: "com.openai.chat", urlScheme: "chatgpt")
    ]

    public static func knownReturnHost(bundleId: String) -> KnownReturnHost? {
        guard let id = normalizeHostBundleId(bundleId) else { return nil }
        return knownReturnHosts.first { $0.bundleId.caseInsensitiveCompare(id) == .orderedSame }
    }

    public static func returnURL(forHostBundleId bundleId: String) -> URL? {
        knownReturnHost(bundleId: bundleId)?.returnURL
    }

    public enum HostReturnStep: Equatable {
        case openBundle(String)
        case openURL(URL, bundleId: String)
        case stay(String)
    }

    /// The only automatic bundle-id attempt. Callers must not repeat it when
    /// the API reports success.
    public static func initialReturnStep(hostBundleId: String?) -> HostReturnStep {
        guard let id = normalizeHostBundleId(hostBundleId), isReturnableHostBundleId(id) else {
            return .stay("no_returnable_host")
        }
        return .openBundle(id)
    }

    /// Used only when the bundle-id API explicitly rejects the open.
    public static func afterBundleOpenRejected(hostBundleId: String) -> HostReturnStep {
        guard let id = normalizeHostBundleId(hostBundleId), isReturnableHostBundleId(id),
              let url = returnURL(forHostBundleId: id) else {
            return .stay("bundle_open_rejected")
        }
        return .openURL(url, bundleId: id)
    }

    public static func writeHostReturnContext(_ ctx: HostReturnContext, defaults: UserDefaults) -> Bool {
        guard isInstanceValid(ctx.launchNonce), let normId = normalizeHostBundleId(ctx.hostBundleId) else { return false }
        let cleanCtx = HostReturnContext(launchNonce: ctx.launchNonce, hostBundleId: normId, capturedAtMs: ctx.capturedAtMs)
        guard let data = try? makeEncoder().encode(cleanCtx) else { return false }
        defaults.set(data, forKey: hostReturnContextKey)
        // Also update legacy key
        defaults.set(normId, forKey: hostBundleIdKey)
        defaults.synchronize()
        return true
    }

    public static func readHostReturnContext(defaults: UserDefaults) -> HostReturnContext? {
        guard let data = defaults.data(forKey: hostReturnContextKey) else { return nil }
        return try? makeDecoder().decode(HostReturnContext.self, from: data)
    }

    public static func clearHostReturnContext(defaults: UserDefaults) {
        defaults.removeObject(forKey: hostReturnContextKey)
        defaults.removeObject(forKey: hostBundleIdKey)
        defaults.synchronize()
    }

    public static func freshHostReturnContext(
        forNonce nonce: String,
        nowMs: UInt64,
        defaults: UserDefaults,
        ttlMs: UInt64 = defaultHostReturnTtlMs
    ) -> HostReturnContext? {
        guard let ctx = readHostReturnContext(defaults: defaults),
              ctx.launchNonce == nonce,
              nowMs >= ctx.capturedAtMs,
              nowMs - ctx.capturedAtMs <= ttlMs else {
            return nil
        }
        return ctx
    }

    // MARK: - Pending Start Intent helpers

    public static func writePendingStartIntent(_ intent: PendingStartIntent, defaults: UserDefaults) -> Bool {
        guard isInstanceValid(intent.launchNonce) else { return false }
        guard let data = try? makeEncoder().encode(intent) else { return false }
        defaults.set(data, forKey: pendingStartIntentKey)
        defaults.synchronize()
        return true
    }

    public static func readPendingStartIntent(defaults: UserDefaults) -> PendingStartIntent? {
        guard let data = defaults.data(forKey: pendingStartIntentKey) else { return nil }
        return try? makeDecoder().decode(PendingStartIntent.self, from: data)
    }

    public static func clearPendingStartIntent(defaults: UserDefaults) {
        defaults.removeObject(forKey: pendingStartIntentKey)
        defaults.synchronize()
    }

    public static func freshPendingStartIntent(
        nowMs: UInt64,
        defaults: UserDefaults
    ) -> PendingStartIntent? {
        guard let intent = readPendingStartIntent(defaults: defaults),
              nowMs >= intent.initiatedAtMs,
              nowMs - intent.initiatedAtMs <= intent.ttlMs else {
            return nil
        }
        return intent
    }

    /// Legacy writer maintained for compatibility; writes legacy key only.
    public static func writeHostBundleId(_ raw: String?, defaults: UserDefaults) -> Bool {
        guard let id = normalizeHostBundleId(raw) else { return false }
        defaults.set(id, forKey: hostBundleIdKey)
        defaults.synchronize()
        return true
    }

    public static func readHostBundleId(defaults: UserDefaults) -> String? {
        guard let id = defaults.string(forKey: hostBundleIdKey) else { return nil }
        return normalizeHostBundleId(id)
    }
    public struct DemandRecord: Codable, Equatable {
        public let nonce: String
        public let createdAtMs: UInt64
        public let ttlMs: UInt64
    }

    // MARK: - Phase record (app-written, single writer via WarmIPCService)

    public enum Phase: String, Codable, Equatable {
        case requested
        case uriReceived
        case modelLoaded
        case micNativeReady
        case warmReady
        case failed
    }

    public struct PhaseRecord: Codable, Equatable {
        public let nonce: String
        public let appEpoch: String
        public let phase: Phase
        public let updatedAtMs: UInt64
        public let errorCode: String?
    }

    // MARK: - Warm live recorder record (app-written, single writer)

    public struct WarmLiveRecord: Codable, Equatable {
        public let appEpoch: String
        public let recorderActive: Bool
        public let updatedAtMs: UInt64
    }

    // MARK: - Errors

    public enum HandshakeError: Error, Equatable {
        case malformedURL
        case missingNonce
    }

    // MARK: - URL construction / parsing

    /// Canonical deep-link URL for keyboard-initiated auto activation.
    public static func makeDictationPrepareURL(nonce: String, tsMs: UInt64) throws -> URL {
        guard !nonce.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw HandshakeError.missingNonce
        }
        var components = URLComponents()
        components.scheme = urlScheme
        components.host = "dictation"
        components.path = "/prepare"
        components.queryItems = [
            URLQueryItem(name: "nonce", value: nonce),
            URLQueryItem(name: "ts", value: String(tsMs))
        ]
        guard let url = components.url else {
            throw HandshakeError.malformedURL
        }
        return url
    }

    /// Parses a received URL. Returns nil for URLs that are not the
    /// dictation/prepare activation entry point.
    public static func parseDictationPrepareURL(_ url: URL) -> (nonce: String, tsMs: UInt64?)? {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.scheme?.lowercased() == urlScheme,
              components.host?.lowercased() == "dictation",
              components.path == "/prepare" else {
            return nil
        }
        let queryItems = components.queryItems ?? []
        guard let nonce = queryItems.first(where: { $0.name == "nonce" })?.value,
              nonce.trimmingCharacters(in: .whitespacesAndNewlines).count >= 8 else {
            return nil
        }
        let tsRaw = queryItems.first(where: { $0.name == "ts" })?.value
        let tsMs = tsRaw.flatMap { UInt64($0) }
        return (nonce: nonce, tsMs: tsMs)
    }

    // MARK: - Record helpers (UserDefaults-backed, Codable JSON)

    public static func isInstanceValid(_ instance: String) -> Bool {
        return instance.trimmingCharacters(in: .whitespacesAndNewlines).count >= 8
    }

    public static func writeDemand(_ demand: DemandRecord, defaults: UserDefaults) -> Bool {
        guard isInstanceValid(demand.nonce) else { return false }
        guard let data = try? makeEncoder().encode(demand) else { return false }
        defaults.set(data, forKey: demandKey)
        defaults.synchronize()
        return true
    }

    public static func readDemand(defaults: UserDefaults) -> DemandRecord? {
        guard let data = defaults.data(forKey: demandKey) else { return nil }
        return try? makeDecoder().decode(DemandRecord.self, from: data)
    }

    public static func clearDemand(defaults: UserDefaults) {
        defaults.removeObject(forKey: demandKey)
        defaults.synchronize()
    }

    /// One-shot consumption: removes the record. Call only after the app has
    /// positively validated the demand as fresh and nonce-matching.
    public static func consumeDemand(defaults: UserDefaults) -> DemandRecord? {
        guard let demand = readDemand(defaults: defaults) else { return nil }
        clearDemand(defaults: defaults)
        return demand
    }

    public static func isDemandFresh(_ demand: DemandRecord, nowMs: UInt64) -> Bool {
        return nowMs >= demand.createdAtMs &&
               nowMs - demand.createdAtMs <= demand.ttlMs
    }

    // MARK: - Phase record

    /// App-side writer. Must be invoked only from the app's single-writer
    /// ipcQueue. Overwrites unconditionally; `nonce` fences stale snapshots.
    public static func writePhase(
        nonce: String,
        appEpoch: String,
        phase: Phase,
        nowMs: UInt64,
        errorCode: String? = nil,
        defaults: UserDefaults
    ) -> Bool {
        guard isInstanceValid(nonce), !appEpoch.isEmpty else { return false }
        let record = PhaseRecord(
            nonce: nonce,
            appEpoch: appEpoch,
            phase: phase,
            updatedAtMs: nowMs,
            errorCode: errorCode
        )
        guard let data = try? makeEncoder().encode(record) else { return false }
        defaults.set(data, forKey: phaseKey)
        defaults.synchronize()
        return true
    }

    public static func readPhase(defaults: UserDefaults) -> PhaseRecord? {
        guard let data = defaults.data(forKey: phaseKey) else { return nil }
        return try? makeDecoder().decode(PhaseRecord.self, from: data)
    }

    /// Nonce/epoch/TTL-fenced read: a phase observation is only trustworthy
    /// when it describes the SAME nonce, the CURRENT app process epoch, and
    /// is still inside the phase TTL window.
    public static func freshPhaseMatching(
        nonce: String,
        appEpoch: String,
        nowMs: UInt64,
        defaults: UserDefaults,
        ttlMs: UInt64 = defaultPhaseTtlMs
    ) -> PhaseRecord? {
        guard let record = readPhase(defaults: defaults),
              record.nonce == nonce,
              record.appEpoch == appEpoch,
              nowMs >= record.updatedAtMs,
              nowMs - record.updatedAtMs <= ttlMs else {
            return nil
        }
        return record
    }

    // MARK: - Warm live / fresh warm recorder authority

    /// App-side writer. Must be invoked only from the app's single-writer
    /// ipcQueue. Called when native hardware recording truth changes.
    public static func writeWarmLive(appEpoch: String, recorderActive: Bool, nowMs: UInt64, defaults: UserDefaults) -> Bool {
        guard !appEpoch.isEmpty else { return false }
        let record = WarmLiveRecord(appEpoch: appEpoch, recorderActive: recorderActive, updatedAtMs: nowMs)
        guard let data = try? makeEncoder().encode(record) else { return false }
        defaults.set(data, forKey: warmLiveKey)
        defaults.synchronize()
        return true
    }

    public static func readWarmLiveRecord(defaults: UserDefaults) -> WarmLiveRecord? {
        guard let data = defaults.data(forKey: warmLiveKey) else { return nil }
        return try? makeDecoder().decode(WarmLiveRecord.self, from: data)
    }

    public static func clearWarmLive(defaults: UserDefaults) {
        defaults.removeObject(forKey: warmLiveKey)
        defaults.synchronize()
    }

    /// Keyboard-side authority check: a positively FRESH, ACTIVE warm recorder
    /// acknowledged by the currently-published app process epoch. Stale app
    /// epochs (dead app process) or aged records fail closed.
    public static func freshWarmLive(
        nowMs: UInt64,
        currentAppEpoch: String?,
        defaults: UserDefaults,
        freshnessMs: UInt64 = defaultWarmLiveFreshnessMs
    ) -> WarmLiveRecord? {
        guard let currentAppEpoch = currentAppEpoch, !currentAppEpoch.isEmpty,
              let record = readWarmLiveRecord(defaults: defaults), record.recorderActive,
              record.appEpoch == currentAppEpoch,
              nowMs >= record.updatedAtMs,
              nowMs - record.updatedAtMs <= freshnessMs else {
            return nil
        }
        return record
    }

    // MARK: - Shared codec helpers

    static func makeEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }

    static func makeDecoder() -> JSONDecoder {
        return JSONDecoder()
    }
}
