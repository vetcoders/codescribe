#!/usr/bin/env bash
# Compile only the installer; exercise generation replacement in a disposable home.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
cat >"$WORKDIR/check.swift" <<'SWIFT'
import CryptoKit
import Darwin
import Foundation

enum CheckFailure: Error { case failed(String) }

func require(_ condition: Bool, _ message: String) throws {
  if !condition { throw CheckFailure.failed(message) }
}

func payload(_ root: URL, version: String?, commit: String?, commands: Bool) throws -> URL {
  let destination = root.appendingPathComponent(UUID().uuidString)
  let files = ["bin/bus-demux.py", "skills/codescribe/SKILL.md"]
    + (commands ? ["bin/cs-bus", "bin/cs-say"] : [])
  var entries: [[String: Any]] = []
  for name in files {
    let file = destination.appendingPathComponent(name)
    try FileManager.default.createDirectory(
      at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    let data = Data("fixture \(version ?? "unversioned") \(commit ?? "unknown") \(name)\n".utf8)
    try data.write(to: file)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: file.path)
    entries.append([
      "path": name, "bytes": data.count, "mode": "0755",
      "sha256": SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined(),
    ])
  }
  var manifest: [String: Any] = [
    "schema": "codescribe.agent-bridge.bundle.v1", "bundle_version": "0.15.2",
    "helper": "bin/bus-demux.py", "skill": "skills/codescribe", "files": entries,
  ]
  if let version { manifest["helper_version"] = version }
  if let commit { manifest["source_commit"] = commit }
  try JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]).write(
    to: destination.appendingPathComponent("manifest.json"))
  return destination
}

@main
struct Checks {
  static func main() {
    do {
      try check()
    } catch {
      FileHandle.standardError.write(Data("agent-bridge-generation: \(error)\n".utf8))
      Darwin.exit(1)
    }
  }

  static func check() throws {
    let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
    let current = try payload(root, version: "0.9.0", commit: "new-source", commands: true)
    let sharedHome = root.appendingPathComponent("shared-agents-home")
    let sharedInstaller = RealAgentBridgeInstaller(
      resourceRoot: current, homeDirectory: sharedHome, environment: [:])
    _ = try sharedInstaller.install(selectedClients: [.claudeCode])
    let copiedSkill = sharedHome.appendingPathComponent(".agents/skills/codescribe")
    try FileManager.default.createDirectory(
      at: copiedSkill.deletingLastPathComponent(), withIntermediateDirectories: true)
    try FileManager.default.copyItem(
      at: sharedHome.appendingPathComponent(".claude/skills/codescribe"), to: copiedSkill)
    try require(sharedInstaller.status().clientsNeedingRepair.contains(.agents),
      "copied Claude marker must offer shared-agent repair")
    _ = try sharedInstaller.installRuntime()
    let repaired = sharedInstaller.status()
    try require(Set(repaired.installedClients) == [.claudeCode, .agents],
      "runtime update must retain Claude and adopt the shared target")
    try require(!repaired.clientsNeedingRepair.contains(.agents), "shared skill repair incomplete")
    let sharedMarkerURL = copiedSkill.appendingPathComponent(".codescribe-managed.json")
    var sharedMarker = try JSONSerialization.jsonObject(with: Data(contentsOf: sharedMarkerURL)) as! [String: Any]
    try require(sharedMarker["client"] as? String == "agents", "shared marker not rewritten")
    try require(try Data(contentsOf: copiedSkill.appendingPathComponent("SKILL.md"))
      == Data(contentsOf: current.appendingPathComponent("skills/codescribe/SKILL.md")),
      "shared skill payload differs")
    sharedMarker["agent_bridge_root"] = "/foreign/bridge"
    try JSONSerialization.data(withJSONObject: sharedMarker).write(to: sharedMarkerURL)
    do {
      _ = try sharedInstaller.installRuntime()
      throw CheckFailure.failed("foreign shared marker was overwritten")
    } catch is AgentBridgeInstallationError {}

    let replacements: [(String?, String?, Bool)] = [
      (nil, nil, false), ("0.8.0", "old-source", true),
      ("0.9.0", "other-source", true),
    ]
    for (index, candidate) in replacements.enumerated() {
      let home = root.appendingPathComponent("home-\(index)")
      let installer = RealAgentBridgeInstaller(
        resourceRoot: current, homeDirectory: home, environment: [:])
      _ = try installer.installRuntime()
      for command in ["cs-bus", "cs-say"] {
        let file = home.appendingPathComponent(".local/bin/" + command)
        let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
        try require(values.isRegularFile == true && values.isSymbolicLink != true,
          "installed command must be a direct ordinary file: \(command)")
      }
      let receipt = home.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
      let before = try Data(contentsOf: receipt)
      let log = home.appendingPathComponent(".codescribe/agent-bridge/runtime/followers/live.log")
      try FileManager.default.createDirectory(
        at: log.deletingLastPathComponent(), withIntermediateDirectories: true)
      try Data("live follower".utf8).write(to: log)
      let descriptor = try FileHandle(forUpdating: log)
      defer { try? descriptor.close() }
      let old = try payload(root, version: candidate.0, commit: candidate.1, commands: candidate.2)
      let detail = try RealAgentBridgeInstaller(
        resourceRoot: old, homeDirectory: home, environment: [:]).installBundledRuntime()
      try require(detail.contains("retained"), "startup must retain installed generation: \(detail)")
      try require(try Data(contentsOf: receipt) == before, "startup replaced receipt")
      for command in ["cs-bus", "cs-say"] {
        try require(FileManager.default.isExecutableFile(
          atPath: home.appendingPathComponent(".local/bin/" + command).path),
          "startup stranded \(command)")
      }
      try descriptor.seekToEnd()
      try descriptor.write(contentsOf: Data(" preserved".utf8))
      try require(try String(contentsOf: log, encoding: .utf8) == "live follower preserved",
        "startup stranded live follower descriptor")
      print("retained-generation-\(index): ok")
      if index == 1 {
        _ = try RealAgentBridgeInstaller(
          resourceRoot: old, homeDirectory: home, environment: [:]).installRuntime()
        try require(try Data(contentsOf: receipt) != before,
          "explicit install must remain authoritative")
        print("explicit-install: ok")
      }
    }
    let home = root.appendingPathComponent("upgrade-home")
    _ = try RealAgentBridgeInstaller(
      resourceRoot: current, homeDirectory: home, environment: [:]).installRuntime()
    let next = try payload(root, version: "0.10.0", commit: "next-source", commands: true)
    let detail = try RealAgentBridgeInstaller(
      resourceRoot: next, homeDirectory: home, environment: [:]).installBundledRuntime()
    try require(detail.contains("synchronized"), "higher helper version must update: \(detail)")
    let receipt = try JSONSerialization.jsonObject(with: Data(contentsOf:
      home.appendingPathComponent(".codescribe/agent-bridge/receipt.json"))) as? [String: Any]
    try require(receipt?["helper_version"] as? String == "0.10.0", "receipt lacks helper version")
    try require(receipt?["source_commit"] as? String == "next-source", "receipt lacks source identity")
    print("versioned-upgrade: ok")
    let migrationHome = root.appendingPathComponent("migration-home")
    let migrationInstaller = RealAgentBridgeInstaller(
      resourceRoot: current, homeDirectory: migrationHome, environment: [:])
    _ = try migrationInstaller.installRuntime()
    for command in ["cs-bus", "cs-say"] {
      let file = migrationHome.appendingPathComponent(".local/bin/" + command)
      try FileManager.default.removeItem(at: file)
      let target = migrationHome.appendingPathComponent(
        ".codescribe/agent-bridge/runtime/bin/" + command)
      try FileManager.default.createSymbolicLink(at: file, withDestinationURL: target)
    }
    try require(try migrationInstaller.installBundledRuntime().contains("unchanged"),
      "same-generation startup must publish direct files")
    let bin = migrationHome.appendingPathComponent(".local/bin")
    try require(try FileManager.default.contentsOfDirectory(atPath: bin.path)
      .allSatisfy { !$0.contains(".backup-") }, "successful publication leaked link backups")
    let sayFile = bin.appendingPathComponent("cs-say")
    try FileManager.default.removeItem(at: sayFile)
    try FileManager.default.createSymbolicLink(at: sayFile, withDestinationURL:
      migrationHome.appendingPathComponent(".codescribe/agent-bridge/runtime/bin/cs-say"))
    // Include the real reported state: the cs-say link is already dangling.
    try FileManager.default.removeItem(at: migrationHome.appendingPathComponent(
      ".codescribe/agent-bridge/runtime/bin/cs-say"))
    _ = try migrationInstaller.installRuntime()
    for command in ["cs-bus", "cs-say"] {
      let file = migrationHome.appendingPathComponent(".local/bin/" + command)
      let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
      try require(values.isRegularFile == true && values.isSymbolicLink != true,
        "owned links must migrate to direct files: \(command)")
    }
    print("owned-and-dangling-link-migration: ok")
    let command = migrationHome.appendingPathComponent(".local/bin/cs-say")
    let receiptFile = migrationHome.appendingPathComponent(".codescribe/agent-bridge/receipt.json")
    let receiptBefore = try Data(contentsOf: receiptFile)
    try Data("foreign command".utf8).write(to: command)
    do {
      _ = try migrationInstaller.installRuntime()
      throw CheckFailure.failed("foreign command accepted")
    } catch is AgentBridgeInstallationError {
      try require(try Data(contentsOf: receiptFile) == receiptBefore, "foreign conflict changed receipt")
      try require(try String(contentsOf: command, encoding: .utf8) == "foreign command",
        "foreign command overwritten")
    }
    print("foreign-command-preserved: ok")
    print("agent-bridge-generation: 7 checks passed")
  }
}
SWIFT
xcrun swiftc -swift-version 6 -warnings-as-errors \
  "$ROOT/macos/Codescribe/Services/AgentBridgeInstaller.swift" \
  "$WORKDIR/check.swift" -o "$WORKDIR/check"
"$WORKDIR/check" "$WORKDIR"
