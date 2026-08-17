import Foundation

/// Lab extras baked only by keyed `make install-app`.
enum DeveloperSurface {
  static func parse(_ raw: Any?) -> Bool {
    if let flag = raw as? Bool { return flag }
    if let number = raw as? NSNumber { return number.boolValue }
    if let text = raw as? String {
      let normalized = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
      return normalized == "1" || normalized == "true" || normalized == "yes"
    }
    return false
  }

  static func isEnabled(in bundle: Bundle = .main) -> Bool {
    parse(bundle.object(forInfoDictionaryKey: "CSDeveloperSurface"))
  }
}
