#!/usr/bin/env bash
# Runs a byte-for-byte copy of production build-app with disposable tools/files.
# Cargo metadata/artifact messages are fixtures, not a second precedence resolver.
# Optional argument tests another revision (e.g. git show BASE:scripts/build-app.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
python3 - "${1:-$ROOT/scripts/build-app.sh}" "$ROOT" <<'PY'
import json
import os
import plistlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv[1]).resolve()
helpers = Path(sys.argv[2]) / "scripts/lib"
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
app_name = spec["app_name"]
bundle_id = spec["bundle_id"]
marketing_version = spec["marketing_version"]
build_version = spec["build_version"]
icon_name = spec["icon_name"]
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
elif name == "security":
    # build-app reads the default keychain's name before stage 1 for its
    # headless-signing diagnostics. The fixture names a keychain under the
    # disposable HOME; any other query (identities, keychain state) is refused.
    assert args == ["default-keychain"], ("forbidden keychain query", args)
    print('    "' + os.environ["HOME"] + '/Library/Keychains/login.keychain-db"')
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
    elif package == "codescribe":
        expected += ["--bin", "codescribe"]
    expected += {"debug": [], "release": ["--release"], "local-release": ["--profile", "local-release"]}[profile]
    # The old-script witness must get through its unchanged Cargo invocation.
    assert args in [expected, expected + ["--message-format=json-render-diagnostics"]], args
    assert os.environ.get("CODESCRIBE_LOCAL_INSTALL") == ("1" if profile == "local-release" else None)
    if mode == "ffi-error" and package == "codescribe-ffi" or mode == "sidecar-error" and package == "codescribe-core":
        sys.exit(42)
    actual = root / "aarch64-unknown-linux-gnu" / profile if mode.startswith("cross-") else out
    actual.mkdir(parents=True, exist_ok=True)
    names = [("codescribe_ffi", "libcodescribe_ffi.dylib"), ("uniffi-bindgen", "uniffi-bindgen")] if package == "codescribe-ffi" else [("codescribe", "codescribe")] if package == "codescribe" else [("codescribe-stt-sidecar", "codescribe-stt-sidecar")]
    if package == "codescribe":
        # Cargo emits the identically named library before the requested bin.
        print(json.dumps({"reason": "compiler-artifact", "target": {"name": "codescribe", "kind": ["lib"]}, "filenames": [str(out / "libcodescribe.rlib")], "executable": None}))
        if mode == "publisher-library-only":
            names = []
    for target, filename in names:
        path = actual / filename
        if filename == "uniffi-bindgen":
            shutil_source = Path(spec["tools"]) / "bindgen-template"
            path.write_bytes(shutil_source.read_bytes())
            path.chmod(0o755)
        else:
            path.write_bytes(("fresh:" + filename).encode())
            path.chmod(0o755)
        if mode == "missing-file" and filename == "libcodescribe_ffi.dylib":
            path.unlink()
        if mode == "missing-receipt" and filename == "uniffi-bindgen":
            continue
        if "--message-format=json-render-diagnostics" in args:
            print(json.dumps({"reason": "compiler-artifact", "target": {"name": target, "kind": ["cdylib"] if target == "codescribe_ffi" else ["bin"]}, "filenames": [str(path)], "executable": str(path) if target != "codescribe_ffi" else None, "fresh": mode == "fresh-receipt"}))
    if mode == "malformed-receipt":
        print("{broken-json")
    print(json.dumps({"reason": "build-finished", "success": mode != "unfinished"}))
elif name == "install_name_tool":
    # The build tree's copy is renamed before the link; the bundle's copy and
    # the app's load command are named again after it.
    embedded = Path(args[-1]).parent.name == "Frameworks"
    if args[0] == "-id" and not embedded:
        assert args[:2] == ["-id", "@rpath/libcodescribe_ffi.dylib"] and len(args) == 3
        record(sha256=identity(args[2], "libcodescribe_ffi.dylib"))
    elif args[0] == "-id":
        assert args[:2] == ["-id", "@rpath/libcodescribe_ffi.dylib"] and len(args) == 3, args
        assert Path(args[2]).read_bytes() == b"fresh:libcodescribe_ffi.dylib", args
        Path(str(args[2]) + ".id").write_text(args[1])
        record()
    else:
        assert args[0] == "-change" and len(args) == 4, args
        assert args[2] == "@rpath/libcodescribe_ffi.dylib" and Path(args[3]).name == app_name, args
        Path(str(args[3]) + ".ffi").write_text(args[2])
        record()
elif name == "otool":
    assert args[0] == "-L" and len(args) == 2, args
    path = Path(args[1])
    if path.name == app_name:
        marker = Path(str(path) + ".ffi")
        ffi = marker.read_text() if marker.exists() else str(out / "deps/libcodescribe_ffi.dylib")
        print(f"{path}:\n\t{ffi} (compatibility version 0.0.0, current version 0.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)")
    else:
        marker = Path(str(path) + ".id")
        own = marker.read_text() if marker.exists() else str(out / "deps/libcodescribe_ffi.dylib")
        print(f"{path}:\n\t{own} (compatibility version 0.0.0, current version 0.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)")
elif name == "uniffi-bindgen":
    assert Path(sys.argv[0]) == out / "uniffi-bindgen", sys.argv[0]
    assert Path(sys.argv[0]).read_bytes() == (Path(spec["tools"]) / "bindgen-template").read_bytes()
    assert args[:-1] == ["generate", "--library", str(out / "libcodescribe_ffi.dylib"), "--language", "swift", "--out-dir"], args
    generated = Path(args[-1])
    assert generated.parent == Path("macos/Codescribe/Bridge") and generated.name.startswith(".uniffi."), args
    (generated / "codescribe_ffi.swift").write_text("// fixture binding\n")
    record(sha256=identity(args[2], "libcodescribe_ffi.dylib"), executable=str(Path(sys.argv[0])))
elif name == "xcodegen":
    assert args == ["generate"] and Path.cwd() == repo / "macos"
elif name == "xcodebuild":
    import shlex
    setting = next(a.split("=", 1)[1] for a in args if a.startswith("LIBRARY_SEARCH_PATHS="))
    assert shlex.split(setting) == [str(out)], ("Xcode library list", setting, str(out))
    for required in [
        f"CODESCRIBE_APP_NAME={app_name}",
        f"CODESCRIBE_EXECUTABLE_NAME={app_name}",
        f"CODESCRIBE_DISPLAY_NAME={app_name}",
        f"CODESCRIBE_BUNDLE_ID={bundle_id}",
        f"CODESCRIBE_APP_ICON_NAME={icon_name}",
        f"MARKETING_VERSION={marketing_version}",
        f"CURRENT_PROJECT_VERSION={build_version}",
    ]:
        assert required in args, ("missing Xcode product setting", required, args)
    record(sha256=identity(Path(shlex.split(setting)[0]) / "libcodescribe_ffi.dylib", "libcodescribe_ffi.dylib"))
    config = "Debug" if profile == "debug" else "Release"
    assert args[args.index("-configuration") + 1] == config
    assert args[args.index("-derivedDataPath") + 1] == str(repo / "macos/build")
    contents = repo / "macos/build/Build/Products" / config / f"{app_name}.app/Contents"
    contents.mkdir(parents=True)
    import plistlib
    product_id = bundle_id + ".dev" if config == "Debug" else bundle_id
    (contents / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleIdentifier": product_id,
        "CFBundleDisplayName": app_name,
        "CFBundleName": app_name,
    }))
    (contents / "MacOS").mkdir()
    (contents / f"MacOS/{app_name}").write_bytes(("fresh:" + app_name).encode())
    localized = contents / "Resources/en.lproj"
    localized.mkdir(parents=True)
    localized_plist = plistlib.dumps({
        "CFBundleDisplayName": "X9Bus",
        "CFBundleName": "X9Bus",
    }, fmt=plistlib.FMT_XML).decode("utf-8").encode("utf-16")
    (localized / "InfoPlist.strings").write_bytes(localized_plist)
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
    signing_identity = spec["signing_identity"]
    expected_id = bundle_id + ".dev" if profile == "debug" else bundle_id
    # A real identity signs with the hardened runtime and a timestamp, and the
    # app (not the publisher) with the default entitlements; ad-hoc stays plain.
    hardened = [] if signing_identity == "-" else ["--options", "runtime", "--timestamp"]
    entitlements = [] if signing_identity == "-" else ["--entitlements", str(repo / "scripts/entitlements.plist")]
    if Path(args[-1]).name == "codescribe":
        assert args[:-1] == ["--force", *hardened, "--sign", signing_identity, "--identifier", expected_id + ".publisher"], args
        publisher = Path(args[-1])
        assert publisher.read_bytes() == b"fresh:codescribe"
        publisher.write_bytes(b"fresh:codescribe:signed")
        record(explicit_publisher_signature=True)
        sys.exit(0)
    assert args[:-1] in [["--force", "--deep", *hardened, *entitlements, "--sign", signing_identity, "--identifier", expected_id], ["--force", *hardened, *entitlements, "--sign", signing_identity, "--identifier", expected_id]], args
    app = Path(args[-1])
    for folder, filename in [("Frameworks", "libcodescribe_ffi.dylib"), ("MacOS", "codescribe-stt-sidecar"), ("MacOS", "codescribe-stt-bridge")]:
        copied = app / "Contents" / folder / filename
        assert copied.read_bytes() == (out / filename).read_bytes() == ("fresh:" + filename).encode(), copied
        record(copy=str(copied), sha256=hashlib.sha256(copied.read_bytes()).hexdigest())
    payload = app / "Contents/Resources/agent-bridge"
    assert (payload / "manifest.json").is_file()
    publisher = payload / "bin/codescribe"
    # Real codesign --deep does not visit a Mach-O under Resources/bin.
    # The publisher therefore must already have its own signature.
    assert publisher.read_bytes() == b"fresh:codescribe:signed"
    if "--deep" not in args:
        manifest = json.loads((payload / "manifest.json").read_text())
        entry = next(row for row in manifest["files"] if row["path"] == "bin/codescribe")
        assert publisher.read_bytes() == b"fresh:codescribe:signed"
        assert entry["bytes"] == publisher.stat().st_size
        assert entry["sha256"] == hashlib.sha256(publisher.read_bytes()).hexdigest()
elif name == "security":
    # build-app probes the default keychain before any identity lookup; the
    # fixture answers with a disposable path and never reaches a real keychain.
    if args[:1] == ["default-keychain"]:
        print('    "' + str(Path(os.environ["HOME"]) / "Library/Keychains/login.keychain-db") + '"')
    elif args[:1] == ["show-keychain-info"]:
        pass
    elif args[:1] == ["find-identity"]:
        record(identity_query=True)
    else:
        raise AssertionError("unexpected security subcommand: " + " ".join(args))
else:
    raise AssertionError("forbidden external boundary: " + name)
'''

passed = failed = 0

def run_case(base, profile, layout, mode="success", skip=False, signing_identity="fixture-identity",
             app_name="X9Bus", bundle_id="com.vetcoders.x9bus", marketing_version="0.18.1",
             build_version="0.18.1", icon_name="X9busAppIcon"):
    global passed, failed
    label = f"{profile}/{layout}/{mode}" + ("/bindings-only" if skip else "")
    if signing_identity == "-":
        label += "/explicit-adhoc"
    case = base / str(passed + failed)
    repo = case / "repo with spaces"
    tools = case / "tools"
    for p in [repo / "scripts", repo / "macos", repo / "skills/codescribe", tools, case / "home", case / "cargo-home", case / "tmp"]:
        p.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, repo / "scripts/build-app.sh")
    (repo / "Cargo.toml").write_text('version = "9.8.7"\n')
    (repo / "skills/codescribe/SKILL.md").write_text("fixture skill\n")
    # The agent bridge sources stage_agent_bridge in build-app.sh refuses to
    # stage without: every bin/ entrypoint, the Claude marketplace manifest and
    # the codescribe plugin directory that receives the server/ copies.
    for entry in ["bus-demux.py", "x9bus", "x9say", "cs-bus", "cs-say", "cs-junie", "bus-mcp.py", "bus-mcp-agent.py", "bus-mcp-events.py", "bus-mcp-runtime.py", "bus-mcp-key-store.py", "bus-mcp-sessions.py"]:
        (repo / "scripts" / entry).write_text("# fixture helper\n")
    (repo / "plugins/.claude-plugin").mkdir(parents=True)
    (repo / "plugins/.claude-plugin/marketplace.json").write_text('{"name":"fixture","plugins":[]}\n')
    (repo / "plugins/codescribe").mkdir()
    (repo / "plugins/codescribe/plugin.json").write_text('{"name":"codescribe"}\n')
    (repo / "scripts/lib").mkdir()
    for helper in ["generate-swift-bindings.sh", "refresh-agent-publisher-manifest.py"]:
        shutil.copyfile(helpers / helper, repo / "scripts/lib" / helper)
    roots = {"default": repo / "target", "absolute": case / "shared", "spaces": case / "shared artifacts", "relative": repo / "relative artifacts", "config": case / "config artifacts"}
    root = roots[layout]
    spec = dict(repo=str(repo), root=str(root), profile=profile, mode=mode, tools=str(tools), log=str(case / "calls.jsonl"), signing_identity=signing_identity,
                app_name=app_name, bundle_id=bundle_id, marketing_version=marketing_version,
                build_version=build_version, icon_name=icon_name)
    spec_path = case / "spec.json"
    spec_path.write_text(json.dumps(spec))
    for name in ["cargo", "git", "install_name_tool", "otool", "bindgen-template", "xcodegen", "xcodebuild", "swiftc", "cp", "codesign", "security"]:
        p = tools / name
        p.write_text("#!" + sys.executable + "\n" + stub)
        p.chmod(0o755)
    # All unrelated local artifacts exist and are deliberately distinguishable.
    old = repo / "target" / profile
    old.mkdir(parents=True)
    for name in ["libcodescribe_ffi.dylib", "codescribe-stt-sidecar", "uniffi-bindgen"]:
        (old / name).write_text("stale:" + name)
    env = {"PATH": str(tools) + ":" + str(Path(sys.executable).parent) + ":/usr/bin:/bin", "HOME": str(case / "home"), "CARGO_HOME": str(case / "cargo-home"), "TMPDIR": str(case / "tmp"), "FIXTURE_SPEC": str(spec_path), "CODESCRIBE_EMBED_EMBEDDER": "1", "CODESCRIBE_CODESIGN_IDENTITY": signing_identity, "CODESCRIBE_LOCAL_INSTALL": "inherited-must-be-cleared", "CODESCRIBE_VOICE_LAB_SRC": str(case / "absent-voice-lab"),
            "CODESCRIBE_APP_NAME": app_name, "CODESCRIBE_EXECUTABLE_NAME": app_name,
            "CODESCRIBE_DISPLAY_NAME": app_name, "CODESCRIBE_BUNDLE_ID": bundle_id,
            "CODESCRIBE_APP_MARKETING_VERSION": marketing_version,
            "CODESCRIBE_APP_BUILD_VERSION": build_version,
            "CODESCRIBE_APP_ICON_NAME": icon_name}
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
    if mode in ("missing-sparkle-key", "missing-license-key"):
        # An install lane without its keys must stop with its own message;
        # under set -u an unset key used to die on "unbound variable".
        env["CODESCRIBE_INSTALL_LANE"] = "1"
        if mode == "missing-license-key":
            env["SPARKLE_ED_PUBLIC_KEY"] = "A" * 43 + "="
    if mode == "custom-key-files":
        # An install lane whose keys live only behind the documented
        # CODESCRIBE_*_PUBLIC_KEY_FILE paths must pass the key gate; HOME holds
        # neither default key directory.
        env["CODESCRIBE_INSTALL_LANE"] = "1"
        keys = case / "custom keys"
        keys.mkdir()
        (keys / "sparkle.b64").write_text("A" * 43 + "=\n")
        (keys / "license.hex").write_text("ab" * 32 + "\n")
        env["CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE"] = str(keys / "sparkle.b64")
        env["CODESCRIBE_LICENSE_PUBLIC_KEY_FILE"] = str(keys / "license.hex")
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
            assert result.returncode == 0 and set(names) <= {"git"}, (result.returncode, names)
            assert (case / "staged/manifest.json").is_file()
        elif mode in ("missing-sparkle-key", "missing-license-key"):
            assert result.returncode == 1, (result.returncode, result.stdout)
            assert "unbound variable" not in result.stdout, result.stdout
            key = "SPARKLE_ED_PUBLIC_KEY" if mode == "missing-sparkle-key" else "CODESCRIBE_LICENSE_PUBLIC_KEY_HEX"
            assert f"install/developer build requires" in result.stdout and key in result.stdout, result.stdout
            assert not set(names) & {"install_name_tool", "uniffi-bindgen", "xcodegen", "xcodebuild", "swiftc", "codesign"}, names
        elif mode in ("success", "fresh-receipt", "custom-key-files"):
            assert result.returncode == 0, f"script exit {result.returncode}: {result.stdout}"
            assert "install/developer build requires" not in result.stdout, result.stdout
            assert all(c["argv"] == ["default-keychain"] for c in calls if c["tool"] == "security"), "Explicit signing must not query identities"
            for name in ["install_name_tool", "uniffi-bindgen"] + ([] if skip else ["xcodebuild"]):
                assert any(c["tool"] == name and "sha256" in c for c in calls), name
            assert sum(c["tool"] == "codesign" and "copy" in c for c in calls) == (0 if skip else 6)
            assert sum(c["tool"] == "cp" and "sha256" in c for c in calls) == (0 if skip else 3)
            if skip:
                assert not set(names) & {"xcodebuild", "swiftc", "codesign"}
            else:
                config = "Debug" if profile == "debug" else "Release"
                product = repo / "macos/build/Build/Products" / config
                assert (product / f"{app_name}.app/Contents/MacOS/{app_name}").is_file()
                assert not (product / ("Codescribe.app" if app_name == "X9Bus" else "X9Bus.app")).exists()
                assert str(product / f"{app_name}.app") in result.stdout
                localized_plist = product / f"{app_name}.app/Contents/Resources/en.lproj/InfoPlist.strings"
                localized_info = plistlib.loads(localized_plist.read_bytes())
                assert localized_info["CFBundleDisplayName"] == app_name, localized_info
                assert localized_info["CFBundleName"] == app_name, localized_info
        else:
            assert result.returncode != 0, "failure silently accepted stale artifacts"
            if mode in ("ffi-error", "sidecar-error"):
                assert result.returncode == 42, result.stdout
            elif mode.startswith("metadata-"):
                assert "cannot resolve Cargo artifact root" in result.stdout, result.stdout
            elif mode in ("missing-receipt", "unfinished", "publisher-library-only"):
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
        run_case(base, profile, "spaces", signing_identity="-")
    run_case(base, "debug", "spaces", "custom-key-files", skip=True)
    for mode in ("metadata-error", "metadata-malformed", "metadata-relative", "ffi-error", "sidecar-error", "missing-receipt", "missing-file", "malformed-receipt", "unfinished", "cross-env", "cross-config", "fresh-receipt", "stage", "publisher-library-only", "missing-sparkle-key", "missing-license-key"):
        run_case(base, "debug", "spaces", mode)
    run_case(base, "local-release", "spaces", "success", app_name="Codescribe",
             bundle_id="com.vetcoders.codescribe", marketing_version="0.18.0",
             build_version="0.18.0", icon_name="AppIcon")
print(f"build-app-target-root: scenarios={passed + failed} passed={passed} failed={failed}")
sys.exit(1 if failed or not passed else 0)
PY
