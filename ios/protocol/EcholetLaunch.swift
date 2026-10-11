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
    public static let defaultWarmLiveFreshnessMs: UInt64 = 10_000

    // MARK: - Demand record (keyboard-written)

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
