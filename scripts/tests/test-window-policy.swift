import Foundation

// Compile with Core/QualityCaptureHost.swift. No AppKit app, windows or FFI.
@main
struct TestWindowPolicySmoke {
  static func main() {
    let flag = QualityCaptureHost.isolatedGUIVariable
    precondition(!QualityCaptureHost.testProcess(classLoaded: false, environment: [:]))
    precondition(QualityCaptureHost.testProcess(classLoaded: true, environment: [:]))
    for marker in ["XCTestConfigurationFilePath", "XCTestSessionIdentifier"] {
      precondition(
        QualityCaptureHost.testProcess(classLoaded: false, environment: [marker: "test"]))
    }
    precondition(QualityCaptureHost.hidesTestWindows(runningTests: true, environment: [:]))
    for invalid in ["", "0", "true", "yes"] {
      precondition(
        QualityCaptureHost.hidesTestWindows(runningTests: true, environment: [flag: invalid]))
    }
    precondition(!QualityCaptureHost.hidesTestWindows(runningTests: true, environment: [flag: "1"]))
    precondition(!QualityCaptureHost.hidesTestWindows(runningTests: false, environment: [:]))
    precondition(
      !QualityCaptureHost.hidesTestWindows(runningTests: false, environment: [flag: "1"]))
    print(
      "PASS: early XCTest detection, invisible default, exact isolated-GUI opt-in, production unaffected; no windows"
    )
  }
}
