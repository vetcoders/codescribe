import XCTest

extension XCTestCase {
  @MainActor
  func awaitCondition(
    timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line,
    _ condition: @escaping @MainActor () -> Bool
  ) async {
    let deadline = Date().addingTimeInterval(timeout)
    while !condition() {
      if Date() >= deadline {
        XCTFail("Asynchronous credential operation did not settle", file: file, line: line)
        return
      }
      try? await Task.sleep(nanoseconds: 1_000_000)
    }
  }
}
