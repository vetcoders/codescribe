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
    let replacements: [(String?, String?, Bool)] = [
      (nil, nil, false), ("0.8.0", "old-source", true),
      ("0.9.0", "other-source", true),
    ]
    for (index, candidate) in replacements.enumerated() {
      let home = root.appendingPathComponent("home-\(index)")
      let installer = RealAgentBridgeInstaller(
        resourceRoot: current, homeDirectory: home, environment: [:])
      _ = try installer.installRuntime()
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
    print("agent-bridge-generation: 5 checks passed")
  }
}
SWIFT
xcrun swiftc -swift-version 6 -warnings-as-errors \
  "$ROOT/macos/Codescribe/Services/AgentBridgeInstaller.swift" \
  "$WORKDIR/check.swift" -o "$WORKDIR/check"
"$WORKDIR/check" "$WORKDIR"
