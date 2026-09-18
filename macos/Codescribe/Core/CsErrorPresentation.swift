import Foundation

/// The bridge's `CsError` conforms to `LocalizedError` with
/// `errorDescription = String(reflecting: self)` (generated binding), so
/// `localizedDescription` renders as `Codescribe.CsError.Agent(msg: "…")` —
/// an enum dump in the chat bubble (Founder 2026-09-09 16:15). Surfaces that
/// show an error to a person read the payload through this seam instead.
extension CsError {
  /// The message the core composed, without the enum wrapper.
  var userFacingMessage: String {
    switch self {
    case .Agent(let msg), .Config(let msg), .Recording(let msg), .License(let msg),
      .Quality(let msg), .Runtime(let msg):
      return msg
    }
  }
}

extension Error {
  /// `CsError` payload when the error crossed the bridge; the system
  /// description otherwise.
  var userFacingMessage: String {
    (self as? CsError)?.userFacingMessage ?? localizedDescription
  }
}
