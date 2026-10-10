import Foundation

/// Echolet iOS Cross-Process IPC Protocol Definition (v1).
///
/// Pure Foundation-only implementation of the Echolet wire data contract
/// between the iOS Containing App and the Keyboard Extension.
/// Matches `src/ios_ipc.rs` Rust serde schema and validation semantics exactly.

public enum EcholetIPC {
    /// Wire protocol version 1.
    public static let protocolVersion: UInt32 = 1

    /// App Group UserDefaults key written only by the Keyboard Extension.
    public static let keyboardRequestKey = "echolet.keyboard.request.v1"

    /// App Group UserDefaults key written only by the Containing App.
    public static let appResponseKey = "echolet.app.response.v1"

    /// Darwin notification posted by Keyboard Extension when writing a request.
    public static let darwinNotificationRequest = "com.echolet.ipc.request.v1"

    /// Darwin notification posted by Containing App when writing a response snapshot.
    public static let darwinNotificationResponse = "com.echolet.ipc.response.v1"

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
        case emptySessionId
        case emptyRequestId
        case invalidSequence(UInt64)
        case invalidAcknowledgedSequence(UInt64)
        case invalidRevision(UInt64)
    }

    /// Keyboard Extension request envelope.
    public struct KeyboardRequest: Codable, Equatable {
        public let protocolVersion: UInt32
        public let sessionId: String
        public let sequence: UInt64
        public let requestId: String
        public let command: Command
        public let clientTimestampMs: UInt64?

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case sessionId = "session_id"
            case sequence
            case requestId = "request_id"
            case command
            case clientTimestampMs = "client_timestamp_ms"
        }

        public init(
            sessionId: String,
            sequence: UInt64,
            requestId: String,
            command: Command,
            clientTimestampMs: UInt64? = nil
        ) throws {
            let trimmedSession = sessionId.trimmingCharacters(in: .whitespacesAndNewlines)
            let trimmedRequest = requestId.trimmingCharacters(in: .whitespacesAndNewlines)

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

    /// Containing App response snapshot envelope.
    public struct AppResponse: Codable, Equatable {
        public let protocolVersion: UInt32
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
            let trimmedSession = sessionId.trimmingCharacters(in: .whitespacesAndNewlines)
            let trimmedReq = acknowledgedRequestId.trimmingCharacters(in: .whitespacesAndNewlines)

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
