import Foundation

/// Echolet iOS Cross-Process IPC Protocol Definition (v2).
///
/// Pure Foundation-only implementation of the Echolet wire data contract
/// between the iOS Containing App and the Keyboard Extension.
/// Matches `src/ios_ipc.rs` Rust serde schema and validation semantics exactly.

public enum EcholetIPC {
    /// Wire protocol version 2.
    public static let protocolVersion: UInt32 = 2

    /// App Group UserDefaults key written only by the Containing App containing its current launch process epoch UUID.
    public static let appEpochKey = "echolet.app.epoch.v2"

    /// App Group UserDefaults key written only by the Keyboard Extension.
    public static let keyboardRequestKey = "echolet.keyboard.request.v2"

    /// App Group UserDefaults key written only by the Containing App.
    public static let appResponseKey = "echolet.app.response.v2"

    /// Darwin notification posted by Keyboard Extension when writing a request.
    public static let darwinNotificationRequest = "com.echolet.ipc.request.v2"

    /// Darwin notification posted by Containing App when writing a response snapshot.
    public static let darwinNotificationResponse = "com.echolet.ipc.response.v2"

    /// Commands sent from the Keyboard Extension to the App.
    public enum Command: String, Codable, Equatable {
        case start
        case stop
        case cancel
    }

    /// Application lifecycle states reported to the Keyboard Extension.
    public enum AppState: String, Codable, Equatable {
        case idle
        case requested
        case preparing
        case listening
        case processing
        case completed
        case blocked
    }

    /// Validation error for IPC envelopes.
    public enum ValidationError: Error, Equatable {
        case unsupportedProtocolVersion(UInt32)
        case emptyAppEpoch
        case emptySessionId
        case emptyRequestId
        case invalidIntentSequence(UInt64)
        case invalidSequence(UInt64)
        case invalidAcknowledgedSequence(UInt64)
        case invalidRevision(UInt64)
    }

    /// Keyboard Extension request envelope (v2).
    public struct KeyboardRequest: Codable, Equatable {
        public let protocolVersion: UInt32
        public let appEpoch: String
        public let intentSequence: UInt64
        public let sessionId: String
        public let sequence: UInt64
        public let requestId: String
        public let command: Command
        public let clientTimestampMs: UInt64?

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case appEpoch = "app_epoch"
            case intentSequence = "intent_sequence"
            case sessionId = "session_id"
            case sequence
            case requestId = "request_id"
            case command
            case clientTimestampMs = "client_timestamp_ms"
        }

        public init(
            appEpoch: String,
            intentSequence: UInt64,
            sessionId: String,
            sequence: UInt64,
            requestId: String,
            command: Command,
            clientTimestampMs: UInt64? = nil
        ) throws {
            let trimmedEpoch = appEpoch.trimmingCharacters(in: .whitespacesAndNewlines)
            let trimmedSession = sessionId.trimmingCharacters(in: .whitespacesAndNewlines)
            let trimmedRequest = requestId.trimmingCharacters(in: .whitespacesAndNewlines)

            guard !trimmedEpoch.isEmpty else {
                throw ValidationError.emptyAppEpoch
            }
            guard intentSequence >= 1 else {
                throw ValidationError.invalidIntentSequence(intentSequence)
            }
            guard !trimmedSession.isEmpty else {
                throw ValidationError.emptySessionId
            }
            guard !trimmedRequest.isEmpty else {
                throw ValidationError.emptyRequestId
            }
            guard sequence >= 1 else {
                throw ValidationError.invalidSequence(sequence)
            }

            self.protocolVersion = EcholetIPC.protocolVersion
            self.appEpoch = trimmedEpoch
            self.intentSequence = intentSequence
            self.sessionId = trimmedSession
            self.sequence = sequence
            self.requestId = trimmedRequest
            self.command = command
            self.clientTimestampMs = clientTimestampMs
        }

        public func validate() throws {
            guard protocolVersion == EcholetIPC.protocolVersion else {
                throw ValidationError.unsupportedProtocolVersion(protocolVersion)
            }
            guard !appEpoch.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ValidationError.emptyAppEpoch
            }
            guard intentSequence >= 1 else {
                throw ValidationError.invalidIntentSequence(intentSequence)
            }
            guard !sessionId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ValidationError.emptySessionId
            }
            guard !requestId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ValidationError.emptyRequestId
            }
            guard sequence >= 1 else {
                throw ValidationError.invalidSequence(sequence)
            }
        }
    }

    /// Containing App response snapshot envelope (v2).
    public struct AppResponse: Codable, Equatable {
        public let protocolVersion: UInt32
        public let appEpoch: String
        public let sessionId: String
        public let acknowledgedRequestId: String
        public let acknowledgedSequence: UInt64
        public let revision: UInt64
        public let state: AppState
        public let recognizedText: String?
        public let isFinal: Bool
        public let errorCode: String?
        public let serverTimestampMs: UInt64?

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case appEpoch = "app_epoch"
            case sessionId = "session_id"
            case acknowledgedRequestId = "acknowledged_request_id"
            case acknowledgedSequence = "acknowledged_sequence"
            case revision
            case state
            case recognizedText = "recognized_text"
            case isFinal = "is_final"
            case errorCode = "error_code"
            case serverTimestampMs = "server_timestamp_ms"
        }

        public init(
            appEpoch: String,
            sessionId: String,
            acknowledgedRequestId: String,
            acknowledgedSequence: UInt64,
            revision: UInt64,
            state: AppState,
            recognizedText: String? = nil,
            isFinal: Bool = false,
            errorCode: String? = nil,
            serverTimestampMs: UInt64? = nil
        ) throws {
            let trimmedEpoch = appEpoch.trimmingCharacters(in: .whitespacesAndNewlines)
            let trimmedSession = sessionId.trimmingCharacters(in: .whitespacesAndNewlines)
            let trimmedReq = acknowledgedRequestId.trimmingCharacters(in: .whitespacesAndNewlines)

            guard !trimmedEpoch.isEmpty else {
                throw ValidationError.emptyAppEpoch
            }
            guard !trimmedSession.isEmpty else {
                throw ValidationError.emptySessionId
            }
            guard !trimmedReq.isEmpty else {
                throw ValidationError.emptyRequestId
            }
            guard acknowledgedSequence >= 1 else {
                throw ValidationError.invalidAcknowledgedSequence(acknowledgedSequence)
            }
            guard revision >= 1 else {
                throw ValidationError.invalidRevision(revision)
            }

            self.protocolVersion = EcholetIPC.protocolVersion
            self.appEpoch = trimmedEpoch
            self.sessionId = trimmedSession
            self.acknowledgedRequestId = trimmedReq
            self.acknowledgedSequence = acknowledgedSequence
            self.revision = revision
            self.state = state
            self.recognizedText = recognizedText
            self.isFinal = isFinal
            self.errorCode = errorCode
            self.serverTimestampMs = serverTimestampMs
        }

        public func validate() throws {
            guard protocolVersion == EcholetIPC.protocolVersion else {
                throw ValidationError.unsupportedProtocolVersion(protocolVersion)
            }
            guard !appEpoch.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ValidationError.emptyAppEpoch
            }
            guard !sessionId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ValidationError.emptySessionId
            }
            guard !acknowledgedRequestId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ValidationError.emptyRequestId
            }
            guard acknowledgedSequence >= 1 else {
                throw ValidationError.invalidAcknowledgedSequence(acknowledgedSequence)
            }
            guard revision >= 1 else {
                throw ValidationError.invalidRevision(revision)
            }
        }
    }

    /// JSON Encoder and Decoder helper with fixed configurations.
    public static func makeEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }

    public static func makeDecoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        return decoder
    }
}
