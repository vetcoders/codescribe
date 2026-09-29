#!/usr/bin/env bash
# Exercise an unchanged copy of the real Make target with disposable boundaries.
# Optional source root and evidence directory allow baseline/repaired receipts.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
python3 - "${1:-$ROOT}" "${2:-}" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv[1]).resolve()
make = shutil.which('make')
if sys.platform == 'darwin':
    make = subprocess.check_output(['/usr/bin/xcrun', '--find', 'make'], text=True).strip()
    native_make = Path(make).with_name('gnumake')
    if native_make.is_file():
        make = str(native_make)
evidence = Path(sys.argv[2]).resolve() if sys.argv[2] else None
if evidence:
    evidence.mkdir(parents=True, exist_ok=True)
stub = r'''
import hashlib, json, os, shlex, sys
from pathlib import Path
s = json.loads(Path(os.environ['FIXTURE_SPEC']).read_text())
name, args = Path(sys.argv[0]).name, sys.argv[1:]
repo, root = Path(s['repo']), Path(s['root'])
def record(**extra):
    with open(s['calls'], 'a') as f:
        f.write(json.dumps(dict(tool=name, argv=args, cwd=os.getcwd(), **extra)) + '\n')
record()
if name == 'cargo':
    assert args == ['metadata', '--no-deps', '--format-version', '1'], args
    assert Path.cwd() == repo
    assert os.environ.get('CARGO_TARGET_DIR') == s['configured']
    values = {'malformed': '{bad', 'relative-metadata': json.dumps({'target_directory': 'relative'}),
              'empty-metadata': '{}', 'null-metadata': '{"target_directory":null}',
              'control-metadata': json.dumps({'target_directory': str(root) + '\tbad'})}
    print(values.get(s['mode'], json.dumps({'target_directory': str(root)})))
    sys.exit(41 if s['mode'] == 'metadata-error' else 0)
elif name == 'xcodegen':
    assert args == ['generate'] and Path.cwd() == repo / 'macos'
    sys.exit(42 if s['mode'] == 'generation-error' else 0)
elif name == 'xcodebuild':
    assert Path.cwd() == repo / 'macos'
    assert args[:3] == ['test', '-scheme', 'Codescribe'], args
    assert args[args.index('-destination') + 1] == 'platform=macOS,arch=arm64'
    assert 'CODE_SIGN_IDENTITY=fixture-signing' in args
    assert 'ONLY_ACTIVE_ARCH=YES' in args
    assert 'ENABLE_TESTABILITY=YES' in args
    assert '-only-testing:CodescribeTests/Fixture' in args
    assert os.environ['DEVELOPER_DIR'] == '/fixture/xcode'
    data = Path(os.environ['CODESCRIBE_DATA_DIR'])
    assert data.is_dir() and data.parent == Path(s['tmp'])
    # Record the effective old project defaults too: baseline guards must execute.
    config = args[args.index('-configuration') + 1] if '-configuration' in args else 'Debug'
    settings = [a.split('=', 1)[1] for a in args if a.startswith('LIBRARY_SEARCH_PATHS=')]
    selected = shlex.split(settings[0]) if settings else [str(repo / 'target' / config.lower())]
    assert len(selected) == 1
    path = Path(selected[0]) / 'libcodescribe_ffi.dylib'
    record(selected=str(path), identity=path.read_text() if path.is_file() else 'missing',
           sha256=hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None,
           config=config, data=str(data))
    if s['mode'] == 'xcode-error':
        print('unfiltered tool failure')
        sys.exit(65)
    count = 0 if s['mode'] == 'zero' else 8 if s['mode'] == 'suite-over' else 7
    seconds = (75 if s['mode'] in ('slow', 'budget') else
               68 if s['mode'] == 'suite-over' else
               30 if s['mode'] == 'two-over' else
               18 if s['mode'] == 'one-over' else
               25 if s['mode'] == 'under' else 1.25)
    durations = {
        'one-over': [('testSlow', 12)],
        'two-over': [('testSlow', 12), ('testAlsoSlow', 11.5)],
        'under': [('testFirst', 9), ('testSecond', 8)],
        'suite-over': [(f'testWait{i}', 8.5) for i in range(8)],
    }
    for test, duration in durations.get(s['mode'], []):
        print(f"Test Case '-[CodescribeTests.FixtureTests {test}]' passed ({duration} seconds).")
    print(f'Executed {count} tests, with 0 failures (0 unexpected) in {seconds} (1.0) seconds')
    print('** TEST SUCCEEDED **')
elif name == 'codescribe-stt-bridge' or name == 'custom-helper':
    assert args == ['--phrase-restart-self-test'], args
    assert str(Path(sys.argv[0]).resolve()) == s['helper']
    sys.exit(43 if s['mode'] == 'helper-error' else 0)
elif name == 'swiftc' and s['mode'] == 'prerequisite-error':
    assert '-o' in args and args[args.index('-o') + 1] == 'target/release/codescribe-stt-bridge'
    sys.exit(44)
elif name in ('security', 'git'):
    sys.exit(1)
else:
    raise AssertionError('forbidden compiler/runtime boundary: ' + name)
'''
passed = failed = 0

def run_case(base, profile, layout, mode='success'):
    global passed, failed
    label = f'{profile}-{layout}-{mode}'
    case = base / label
    repo, bin_dir = case / 'repo', case / 'bin'
    for p in (repo / 'scripts/lib', repo / 'macos', bin_dir, case / 'tmp', case / 'cargo-home'):
        p.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / 'Makefile', repo / 'Makefile')
    helper_source = source / 'scripts/test-swift.sh'
    if helper_source.is_file():
        shutil.copyfile(helper_source, repo / 'scripts/test-swift.sh')
    shutil.copyfile(source / 'scripts/lib/data-assets.sh', repo / 'scripts/lib/data-assets.sh')
    (repo / 'scripts/lib/data-assets.sh').chmod(0o755)
    (repo / 'Cargo.toml').write_text('[package]\nname="fixture"\nversion="0.0.0"\n')
    roots = {'default': repo / 'target', 'absolute': case / 'shared',
             'spaces': case / 'shared artifacts', 'relative': case / 'config relative',
             'relative-env': case / 'env relative'}
    root = roots[layout]
    configured = '../config relative' if layout == 'relative' else (str(root) if layout != 'default' else None)
    (repo / '.cargo').mkdir()
    (repo / '.cargo/config.toml').write_text('[build]\ntarget-dir = "../config relative"\n' if layout == 'relative' else '')
    # The configured relative value is intentionally different from metadata output.
    if layout == 'relative':
        configured = None
    elif layout == 'relative-env':
        configured = '../env relative'
    local = repo / 'target' / profile
    local.mkdir(parents=True)
    (local / 'libcodescribe_ffi.dylib').write_text('stale-local')
    if mode != 'missing':
        (root / profile).mkdir(parents=True, exist_ok=True)
        (root / profile / 'libcodescribe_ffi.dylib').write_text('selected-' + profile)
    bridge = repo / ('custom-helper' if mode == 'override-helper' else 'target/release/codescribe-stt-bridge')
    bridge.parent.mkdir(parents=True, exist_ok=True)
    spec = dict(repo=str(repo), root=str(root), configured=configured, mode=mode,
                helper=str(bridge), calls=str(case / 'calls.jsonl'), tmp=str(case / 'tmp'))
    (case / 'spec.json').write_text(json.dumps(spec))
    for name in ('cargo', 'xcodebuild', 'swiftc', 'codesign', 'security', 'git', 'xcrun', 'clang', 'cc', 'rustc'):
        p = bin_dir / name
        p.write_text('#!' + sys.executable + '\n' + stub)
        p.chmod(0o755)
    if mode != 'missing-xcodegen':
        (bin_dir / 'xcodegen').write_text('#!' + sys.executable + '\n' + stub)
        (bin_dir / 'xcodegen').chmod(0o755)
    # Restrict PATH to stubs and a whitelist; even missing tools cannot hit host Xcode.
    for name in ('python3', 'sed', 'head', 'tail', 'grep', 'tee', 'awk', 'sort', 'mktemp', 'rm', 'wc', 'tr', 'dirname', 'bash', 'mkdir'):
        os.symlink(sys.executable if name == 'python3' else shutil.which(name), bin_dir / name)
    for name in ('codescribe-stt-bridge.swift', 'bridge-Info.plist'):
        p = repo / 'core/stt/apple_stt' / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text('fixture prerequisite\n')
        os.utime(p, (1, 1))
    bridge.write_text('#!' + sys.executable + '\n' + stub)
    bridge.chmod(0o755)
    if mode == 'prerequisite-error':
        bridge.unlink()
    env = dict(PATH=str(bin_dir), TMPDIR=str(case / 'tmp'), CARGO_HOME=str(case / 'cargo-home'),
               FIXTURE_SPEC=str(case / 'spec.json'), DEVELOPER_DIR='/fixture/xcode')
    if configured:
        env['CARGO_TARGET_DIR'] = configured
    args = [make, '-C', str(repo), 'test-swift',
            'SWIFT_TEST_LOG=' + str(case / 'swift.log'), 'SWIFT_TEST_CODESIGN_IDENTITY=fixture-signing',
            'SWIFT_TEST_ARGS=-only-testing:CodescribeTests/Fixture', 'DATA_ASSETS_DIR=' + str(case / 'absent-assets')]
    if profile != 'debug':
        args.append('PROFILE=' + profile)
    if mode == 'override-helper':
        args.append('ENGINE_BRIDGE=' + str(bridge))
    if mode == 'budget':
        args.append('SWIFT_TEST_MAX_SECONDS=90')
    result = subprocess.run(args, cwd=base, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    calls = [json.loads(l) for l in Path(spec['calls']).read_text().splitlines()] if Path(spec['calls']).exists() else []
    if evidence:
        (evidence / (label + '.log')).write_text(result.stdout)
        (evidence / (label + '.json')).write_text(json.dumps(dict(argv=args, exit=result.returncode, calls=calls), indent=2) + '\n')
    try:
        names = [c['tool'] for c in calls]
        assert not set(names) & ({'swiftc'} if mode != 'prerequisite-error' else set()), names
        assert not set(names) & {'codesign', 'xcrun', 'clang', 'cc', 'rustc'}, names
        helpers = [c for c in calls if c['tool'] in ('codescribe-stt-bridge', 'custom-helper')]
        assert len(helpers) == (0 if mode == 'prerequisite-error' else 1), helpers
        if mode == 'prerequisite-error':
            assert names.count('swiftc') == 1 and 'Error 44' in result.stdout, result.stdout
        refusal = mode not in ('success', 'override-helper', 'budget', 'under') or profile == 'unsupported'
        if refusal:
            assert result.returncode != 0, 'refusal accepted'
            if mode in ('xcode-error', 'zero', 'slow', 'one-over', 'two-over', 'suite-over'):
                expected = {'xcode-error': 65, 'zero': 3, 'slow': 4,
                            'one-over': 5, 'two-over': 5, 'suite-over': 4}[mode]
                assert f'Error {expected}' in result.stdout, result.stdout
                assert 'xcodebuild' in names
                if mode == 'one-over':
                    assert 'FixtureTests.testSlow took 12 s' in result.stdout, result.stdout
                if mode == 'two-over':
                    assert 'FixtureTests.testSlow took 12 s' in result.stdout, result.stdout
                    assert 'FixtureTests.testAlsoSlow took 11.5 s' in result.stdout, result.stdout
                if mode == 'suite-over':
                    assert 'per-test ceiling' not in result.stdout, result.stdout
            else:
                assert 'xcodebuild' not in names, names
                if mode == 'helper-error':
                    assert 'Error 43' in result.stdout
        else:
            assert result.returncode == 0, result.stdout
            assert names.count('cargo') == 1 and names.count('xcodegen') == 1, names
            receipt = next(c for c in calls if 'selected' in c)
            assert receipt['selected'] == str(root / profile / 'libcodescribe_ffi.dylib'), receipt
            assert receipt['identity'] == 'selected-' + profile, receipt
            assert receipt['config'] == 'Debug', receipt
            argv = next(c['argv'] for c in calls if c['tool'] == 'xcodebuild')
            for key in ('LIBRARY_SEARCH_PATHS', 'LD_RUNPATH_SEARCH_PATHS'):
                settings = [a.split('=', 1)[1] for a in argv if a.startswith(key + '=')]
                expected = [str(root / profile)] + (['@executable_path/../Frameworks'] if key == 'LD_RUNPATH_SEARCH_PATHS' else [])
                assert len(settings) == 1 and shlex.split(settings[0]) == expected, (key, settings)
            assert not Path(receipt['data']).exists(), 'host fixture leaked'
            assert (case / 'swift.log').is_file()
            if mode == 'under':
                assert 'per-test ceiling' not in result.stdout, result.stdout
        assert not list((case / 'tmp').iterdir()), 'fixture cleanup failed'
        passed += 1
        print(f'PASS {label} (make exit {result.returncode})', flush=True)
    except (AssertionError, StopIteration) as e:
        failed += 1
        print(f'FAIL {label}: {e}\n{result.stdout}', flush=True)

with tempfile.TemporaryDirectory(prefix='test-swift-target-root-') as tmp:
    base = Path(tmp).resolve()
    for profile in ('debug', 'release', 'local-release'):
        for layout in ('default', 'absolute', 'spaces', 'relative', 'relative-env'):
            run_case(base, profile, layout)
    for mode in ('missing', 'malformed', 'relative-metadata', 'empty-metadata', 'null-metadata',
                 'control-metadata', 'metadata-error', 'missing-xcodegen', 'generation-error',
                 'xcode-error', 'zero', 'slow', 'budget', 'one-over', 'two-over',
                 'under', 'suite-over', 'helper-error', 'override-helper', 'prerequisite-error'):
        run_case(base, 'debug', 'spaces', mode)
    run_case(base, 'unsupported', 'spaces')
print(f'test-swift-target-root: scenarios={passed + failed} passed={passed} failed={failed}')
sys.exit(1 if failed or not passed else 0)
PY
