import Foundation

/// The single simple error type used across the pure model flow surfaces
/// (Swift Result requires Failure: Error; a plain String is not one).
public struct EcholetModelError: Error, Equatable {
    public let message: String

    public init(message: String) {
        self.message = message
    }
}
