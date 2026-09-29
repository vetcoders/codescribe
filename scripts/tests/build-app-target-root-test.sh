#!/usr/bin/env bash
# Runs a byte-for-byte copy of production build-app with disposable tools/files.
# Cargo metadata/artifact messages are fixtures, not a second precedence resolver.
# Optional argument tests another revision (e.g. git show BASE:scripts/build-app.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
python3 - "${1:-$ROOT/scripts/build-app.sh}" <<'PY'
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv[1]).resolve()
# Every external build/signing boundary is a stub, including git (no ancestor
# checkout discovery), and security (must never reach a real keychain).
stub = r'''
import hashlib, json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
spec = json.loads(Path(os.environ["FIXTURE_SPEC"]).read_text())
repo = Path(spec["repo"])
root = Path(spec["root"])
profile = spec["profile"]
out = root / profile
mode = spec["mode"]
def record(**extra):
    with open(spec["log"], "a") as f:
        f.write(json.dumps(dict(tool=name, argv=args, cwd=os.getcwd(), **extra)) + "\n")
def identity(path, kind):
    path = Path(path)
    assert path == out / kind, ("wrong artifact path", str(path), str(out / kind))
    assert path.read_bytes() == ("fresh:" + kind).encode(), ("stale artifact", str(path))
    return hashlib.sha256(path.read_bytes()).hexdigest()
record(local_install=os.environ.get("CODESCRIBE_LOCAL_INSTALL"))
if name == "git":
    sys.exit(1)
elif name == "cargo":
    assert Path.cwd() == repo
    if args[0] == "metadata":
        assert args == ["metadata", "--no-deps", "--format-version", "1"], args
        if mode == "metadata-error":
            print(json.dumps({"target_directory": str(root)}))
            sys.exit(41)
        if mode == "metadata-malformed":
            print("invalid-json")
        else:
            print(json.dumps({"target_directory": "relative-invalid" if mode == "metadata-relative" else str(root)}))
        sys.exit(0)
    package = args[args.index("-p") + 1]
    expected = ["build", "-p", package]
    if package == "codescribe-core":
        expected += ["--bin", "codescribe-stt-sidecar"]
    expected += {"debug": [], "release": ["--release"], "local-release": ["--profile", "local-release"]}[profile]
    # The old-script witness must get through its unchanged Cargo invocation.
    assert args in [expected, expected + ["--message-format=json-render-diagnostics"]], args
    assert os.environ.get("CODESCRIBE_LOCAL_INSTALL") == ("1" if profile == "local-release" else None)
    if mode == "ffi-error" and package == "codescribe-ffi" or mode == "sidecar-error" and package == "codescribe-core":
        sys.exit(42)
    actual = root / "aarch64-unknown-linux-gnu" / profile if mode.startswith("cross-") else out
    actual.mkdir(parents=True, exist_ok=True)
    names = [("codescribe_ffi", "libcodescribe_ffi.dylib"), ("uniffi-bindgen", "uniffi-bindgen")] if package == "codescribe-ffi" else [("codescribe-stt-sidecar", "codescribe-stt-sidecar")]
    for target, filename in names:
        path = actual / filename
        if filename == "uniffi-bindgen":
            shutil_source = Path(spec["tools"]) / "bindgen-template"
            path.write_bytes(shutil_source.read_bytes())
            path.chmod(0o755)
        else:
            path.write_bytes(("fresh:" + filename).encode())
        if mode == "missing-file" and filename == "libcodescribe_ffi.dylib":
            path.unlink()
        if mode == "missing-receipt" and filename == "uniffi-bindgen":
            continue
        if "--message-format=json-render-diagnostics" in args:
            print(json.dumps({"reason": "compiler-artifact", "target": {"name": target}, "filenames": [str(path)], "executable": str(path) if target != "codescribe_ffi" else None, "fresh": mode == "fresh-receipt"}))
    if mode == "malformed-receipt":
        print("{broken-json")
    print(json.dumps({"reason": "build-finished", "success": mode != "unfinished"}))
elif name == "install_name_tool":
    assert args[:2] == ["-id", "@rpath/libcodescribe_ffi.dylib"] and len(args) == 3
    record(sha256=identity(args[2], "libcodescribe_ffi.dylib"))
elif name == "uniffi-bindgen":
    assert Path(sys.argv[0]) == out / "uniffi-bindgen", sys.argv[0]
    assert Path(sys.argv[0]).read_bytes() == (Path(spec["tools"]) / "bindgen-template").read_bytes()
    assert args == ["generate", "--library", str(out / "libcodescribe_ffi.dylib"), "--language", "swift", "--out-dir", "macos/Codescribe/Bridge"], args
    record(sha256=identity(args[2], "libcodescribe_ffi.dylib"), executable=str(Path(sys.argv[0])))
elif name == "xcodegen":
    assert args == ["generate"] and Path.cwd() == repo / "macos"
elif name == "xcodebuild":
    import shlex
    setting = next(a.split("=", 1)[1] for a in args if a.startswith("LIBRARY_SEARCH_PATHS="))
    assert shlex.split(setting) == [str(out)], ("Xcode library list", setting, str(out))
    record(sha256=identity(Path(shlex.split(setting)[0]) / "libcodescribe_ffi.dylib", "libcodescribe_ffi.dylib"))
    config = "Debug" if profile == "debug" else "Release"
    assert args[args.index("-configuration") + 1] == config
    assert args[args.index("-derivedDataPath") + 1] == str(repo / "macos/build")
    (repo / "macos/build/Build/Products" / config / "Codescribe.app/Contents").mkdir(parents=True)
elif name == "cp":
    import subprocess
    assert len(args) == 2, args
    source_path, destination = map(Path, args)
    assert source_path.parent == out, args
    assert destination.is_relative_to(repo / "macos/build"), args
    record(sha256=identity(source_path, source_path.name))
    subprocess.run(["/bin/cp", *args], check=True)
elif name == "swiftc":
    dest = Path(args[args.index("-o") + 1])
    assert dest == out / "codescribe-stt-bridge", args
    dest.write_bytes(b"fresh:codescribe-stt-bridge")
elif name == "codesign":
    assert args[:6] == ["--force", "--deep", "--sign", "fixture-identity", "--identifier", "com.vetcoders.codescribe"]
    app = Path(args[-1])
    for folder, filename in [("Frameworks", "libcodescribe_ffi.dylib"), ("MacOS", "codescribe-stt-sidecar"), ("MacOS", "codescribe-stt-bridge")]:
        copied = app / "Contents" / folder / filename
        assert copied.read_bytes() == (out / filename).read_bytes() == ("fresh:" + filename).encode(), copied
        record(copy=str(copied), sha256=hashlib.sha256(copied.read_bytes()).hexdigest())
    assert (app / "Contents/Resources/agent-bridge/manifest.json").is_file()
else:
    raise AssertionError("forbidden external boundary: " + name)
'''

passed = failed = 0

def run_case(base, profile, layout, mode="success", skip=False):
    global passed, failed
    label = f"{profile}/{layout}/{mode}" + ("/bindings-only" if skip else "")
    case = base / str(passed + failed)
    repo = case / "repo with spaces"
    tools = case / "tools"
    for p in [repo / "scripts", repo / "macos", repo / "skills/codescribe", tools, case / "home", case / "cargo-home", case / "tmp"]:
        p.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, repo / "scripts/build-app.sh")
    (repo / "Cargo.toml").write_text('version = "9.8.7"\n')
    (repo / "skills/codescribe/SKILL.md").write_text("fixture skill\n")
    (repo / "scripts/bus-demux.py").write_text("# fixture helper\n")
    roots = {"default": repo / "target", "absolute": case / "shared", "spaces": case / "shared artifacts", "relative": repo / "relative artifacts", "config": case / "config artifacts"}
    root = roots[layout]
    spec = dict(repo=str(repo), root=str(root), profile=profile, mode=mode, tools=str(tools), log=str(case / "calls.jsonl"))
    spec_path = case / "spec.json"
    spec_path.write_text(json.dumps(spec))
    for name in ["cargo", "git", "install_name_tool", "bindgen-template", "xcodegen", "xcodebuild", "swiftc", "cp", "codesign", "security"]:
        p = tools / name
        p.write_text("#!" + sys.executable + "\n" + stub)
        p.chmod(0o755)
    # All unrelated local artifacts exist and are deliberately distinguishable.
    old = repo / "target" / profile
    old.mkdir(parents=True)
    for name in ["libcodescribe_ffi.dylib", "codescribe-stt-sidecar", "uniffi-bindgen"]:
        (old / name).write_text("stale:" + name)
    env = {"PATH": str(tools) + ":" + str(Path(sys.executable).parent) + ":/usr/bin:/bin", "HOME": str(case / "home"), "CARGO_HOME": str(case / "cargo-home"), "TMPDIR": str(case / "tmp"), "FIXTURE_SPEC": str(spec_path), "CODESCRIBE_EMBED_EMBEDDER": "1", "CODESCRIBE_CODESIGN_IDENTITY": "fixture-identity", "CODESCRIBE_LOCAL_INSTALL": "inherited-must-be-cleared", "CODESCRIBE_VOICE_LAB_SRC": str(case / "absent-voice-lab")}
    if layout in ("absolute", "spaces"):
        env["CARGO_TARGET_DIR"] = str(root)
    elif layout == "relative":
        env["CARGO_TARGET_DIR"] = "relative artifacts"
    elif layout == "config":
        (repo / ".cargo").mkdir()
        (repo / ".cargo/config.toml").write_text('[build]\ntarget-dir = "../config artifacts"\n')
    if mode == "cross-env":
        env["CARGO_BUILD_TARGET"] = "aarch64-unknown-linux-gnu"
    elif mode == "cross-config":
        (repo / ".cargo").mkdir(exist_ok=True)
        (repo / ".cargo/config.toml").write_text('[build]\ntarget = "aarch64-unknown-linux-gnu"\n')
    if skip:
        env["SKIP_XCODEBUILD"] = "1"
    args = ["/bin/bash", str(repo / "scripts/build-app.sh"), profile]
    if mode == "stage":
        args = args[:2] + ["--stage-agent-bridge", str(case / "staged"), "9.8.7"]
    result = subprocess.run(args, cwd=case, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    calls = [json.loads(line) for line in (case / "calls.jsonl").read_text().splitlines()] if (case / "calls.jsonl").exists() else []
    names = [c["tool"] for c in calls]
    try:
        if mode == "stage":
            assert result.returncode == 0 and not calls, (result.returncode, names)
            assert (case / "staged/manifest.json").is_file()
        elif mode in ("success", "fresh-receipt"):
            assert result.returncode == 0, f"script exit {result.returncode}"
            for name in ["install_name_tool", "uniffi-bindgen"] + ([] if skip else ["xcodebuild"]):
                assert any(c["tool"] == name and "sha256" in c for c in calls), name
            assert sum(c["tool"] == "codesign" and "copy" in c for c in calls) == (0 if skip else 3)
            assert sum(c["tool"] == "cp" and "sha256" in c for c in calls) == (0 if skip else 3)
            if skip:
                assert not set(names) & {"xcodebuild", "swiftc", "codesign"}
        else:
            assert result.returncode != 0, "failure silently accepted stale artifacts"
            if mode in ("ffi-error", "sidecar-error"):
                assert result.returncode == 42, result.stdout
            elif mode.startswith("metadata-"):
                assert "cannot resolve Cargo artifact root" in result.stdout, result.stdout
            elif mode in ("missing-receipt", "unfinished"):
                assert "incomplete Cargo artifact receipts" in result.stdout, result.stdout
            elif mode == "missing-file":
                assert "Cargo artifact for codescribe_ffi is not" in result.stdout, result.stdout
            elif mode == "malformed-receipt":
                assert "JSONDecodeError" in result.stdout, result.stdout
            assert not set(names) & {"install_name_tool", "uniffi-bindgen", "xcodegen", "xcodebuild", "swiftc", "codesign"}, names
            if mode.startswith("metadata-"):
                assert not any(c["tool"] == "cargo" and c["argv"][0] == "build" for c in calls)
            if mode.startswith("cross-"):
                assert "cross-compilation is unsupported" in result.stdout
            assert (old / "libcodescribe_ffi.dylib").read_text() == "stale:libcodescribe_ffi.dylib"
        passed += 1
        print(f"PASS {label} (script exit {result.returncode})", flush=True)
    except AssertionError as e:
        failed += 1
        print(f"FAIL {label}: {e}\n{result.stdout}", flush=True)

with tempfile.TemporaryDirectory(prefix="build-app-target-root-") as tmp:
    base = Path(tmp).resolve()
    for profile in ("debug", "local-release", "release"):
        for layout in ("default", "absolute", "spaces", "relative", "config"):
            run_case(base, profile, layout)
        run_case(base, profile, "spaces", skip=True)
    for mode in ("metadata-error", "metadata-malformed", "metadata-relative", "ffi-error", "sidecar-error", "missing-receipt", "missing-file", "malformed-receipt", "unfinished", "cross-env", "cross-config", "fresh-receipt", "stage"):
        run_case(base, "debug", "spaces", mode)
print(f"build-app-target-root: scenarios={passed + failed} passed={passed} failed={failed}")
sys.exit(1 if failed or not passed else 0)
PY
