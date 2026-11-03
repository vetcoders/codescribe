"""Pinned plugin entry. Execute only the canonical, matching installed BUS owner."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def runtime():
    plugin = Path(__file__).resolve().parents[1]
    bundled = plugin / "server"
    manifest = json.loads((bundled / "manifest.json").read_text())
    if (manifest.get("api_version") != 1 or not isinstance(manifest.get("files"), list)
            or not {"cs-bus", "bus-demux.py", "bus-mcp.py", "bus-mcp-agent.py", "bus-mcp-sessions.py"}
            <= {entry.get("path") for entry in manifest["files"]}):
        raise ValueError("Codescribe server package is incomplete or incompatible")
    # Paths are derived locally. Tool arguments cannot select executables,
    # helper versions, bridge roots, environment variables or another scope.
    root = Path(os.environ.get("CODESCRIBE_AGENT_BRIDGE_HOME", str(Path.home() / ".codescribe/agent-bridge"))).expanduser().resolve()
    installed = root / "runtime" / "bin"
    for entry in manifest["files"]:
        name = entry["path"]
        if Path(name).name != name:
            raise ValueError("Invalid packaged server path")
        for base in (bundled, installed):
            path = base / name
            if (path.is_symlink() or not path.is_file()
                    or hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]):
                raise ValueError("Codescribe plugin and managed helpers differ; install the matching Codescribe BUS package, then reload this plugin")
    return root, load("codescribe_plugin_demux", installed / "bus-demux.py"), installed


def main():
    if sys.argv[1:] not in (["--hook"], ["--serve"]):
        raise ValueError("Use --hook or --serve")
    root, demux, installed = runtime()
    sessions = load("codescribe_plugin_sessions", installed / "bus-mcp-sessions.py")
    if sys.argv[1] == "--hook":
        payload = json.loads(sys.stdin.read(65537))
        if not isinstance(payload, dict):
            raise ValueError("Host hook object required")
        print(json.dumps(sessions.host_hook(demux, root, payload)))
        return
    native = demux.native_wake_module()
    router = sessions.SessionCapabilities(demux, native, root)
    native.serve(demux, root, None, mailbox=router, tools=router.tools)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print("Codescribe plugin unavailable: " + str(error), file=sys.stderr)
        raise SystemExit(3)
