import Darwin
import Foundation

/// The source install and signed app use the same payload and installer.
@main
struct AgentBridgeRuntimeInstaller {
  static func main() {
    guard CommandLine.arguments.count == 2 else {
      print("usage: agent-bridge-install <payload-directory>")
      Darwin.exit(2)
    }
    do {
      let installer = RealAgentBridgeInstaller(
        resourceRoot: URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true))
      print(try installer.installRuntime())
    } catch {
      print("Agent bridge installation failed: " + error.localizedDescription)
      Darwin.exit(1)
    }
  }
}
