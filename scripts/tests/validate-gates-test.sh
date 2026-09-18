#!/bin/bash
# Real validator, disposable source roots, diagnostic-specific counterexamples.
# Optional argument retains source diffs, stdout, stderr, exits and fake-tool calls.
# No compiler, product import, private corpus, target directory or network access.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
python3 - "$ROOT" "${1:-}" <<'PY'
import difflib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv[1]).resolve()
make_source = (source / 'Makefile').read_text()
helper_source = (source / 'scripts/test-swift.sh').read_text()
validator = source / 'scripts/validate-gates.sh'
passed = 0


def replace(text, old, new):
    assert text.count(old) == 1, ('fixture anchor drift', old, text.count(old))
    return text.replace(old, new)


def target(text, name):
    lines = text.splitlines(keepends=True)
    start = next(i for i, line in enumerate(lines) if line.startswith(name + ':'))
    end = start + 1
    while end < len(lines) and (lines[end].startswith('\t') or not lines[end].strip() or lines[end].startswith('#')):
        end += 1
    return ''.join(lines[start:end]).rstrip() + '\n'


rule = target(make_source, 'test-swift').split('\n\n', 1)[0] + '\n'
invocation = '\t$(SHELL) scripts/test-swift.sh "$(PROFILE)" "$(ENGINE_BRIDGE)" \\\n'
selftest = '"${ENGINE_BRIDGE}" --phrase-restart-self-test || exit $?'
generation = '( cd macos && xcodegen generate ) || exit $?'

with tempfile.TemporaryDirectory(prefix='validate-gates-test-') as tmp:
    base = Path(tmp)
    evidence = Path(sys.argv[2]).resolve() if sys.argv[2] else base / 'evidence'
    evidence.mkdir(parents=True, exist_ok=True)
    # A whitelist prevents accidental tool escalation even if the validator drifts.
    bin_dir = base / 'bin'
    bin_dir.mkdir()
    for name in ('sed', 'sort', 'grep', 'find', 'cat', 'bash', 'mktemp', 'rm', 'wc', 'tr', 'dirname'):
        os.symlink('/bin/bash' if name == 'bash' else shutil.which(name), bin_dir / name)
    os.symlink(sys.executable, bin_dir / 'python3')
    env = dict(PATH=str(bin_dir), LC_ALL='C', TMPDIR=str(base))

    def check(label, make=make_source, helper=helper_source, diagnostic=None, workflow=None, code_case=None):
        global passed
        repo = base / label
        (repo / 'scripts').mkdir(parents=True)
        (repo / 'Makefile').write_text(make)
        shutil.copytree(source / '.github/workflows', repo / '.github/workflows')
        if workflow is not None:
            (repo / '.github/workflows/counterexample.yml').write_text(workflow)
        if helper is not None:
            (repo / 'scripts/test-swift.sh').write_text(helper)
        selected_validator = validator
        if code_case is not None:
            # Independent code root, including spaces; candidate sources stay in repo.
            code = base / (label + ' code') / 'scripts'
            code.mkdir(parents=True)
            selected_validator = code / validator.name
            selected_validator.write_text(validator.read_text())
            module = code / 'validate_swift_gate.py'
            module.write_text((source / 'scripts/validate_swift_gate.py').read_text())
            # A candidate-local decoy must never replace missing trusted code.
            (repo / 'scripts/validate_swift_gate.py').write_text(
                'from pathlib import Path\nPath("EXECUTED").touch()\n')
            if code_case == 'missing':
                module.unlink()
            elif code_case == 'unreadable':
                module.chmod(0)
                assert not os.access(module, os.R_OK), 'unreadable witness needs an unprivileged user'
            elif code_case == 'directory':
                module.unlink()
                module.mkdir()
            elif code_case == 'wrong-path':
                selected_validator.write_text(replace(selected_validator.read_text(),
                    '/validate_swift_gate.py"', '/disconnected_swift_gate.py"'))
            elif code_case == 'failure':
                module.write_text('import sys\nprint("fixture-module-exit-47", file=sys.stderr)\nsys.exit(47)\n')
            elif code_case == 'syntax':
                module.write_text('def broken(:\n')
            else:
                assert code_case == 'valid', code_case
        result = subprocess.run(['/bin/bash', str(selected_validator)], cwd=repo, env=env,
                                text=True, capture_output=True)
        receipt = evidence / label
        receipt.mkdir()
        (receipt / 'stdout').write_text(result.stdout)
        (receipt / 'stderr').write_text(result.stderr)
        (receipt / 'result.json').write_text(json.dumps(dict(
            argv=['/bin/bash', str(selected_validator)], cwd=str(repo), exit=result.returncode,
            expected_diagnostic=diagnostic), indent=2) + '\n')
        (receipt / 'source.diff').write_text(''.join(difflib.unified_diff(
            make_source.splitlines(True), make.splitlines(True), fromfile='Makefile', tofile=label + '/Makefile'))
            + ''.join(difflib.unified_diff(helper_source.splitlines(True), (helper or '').splitlines(True),
                                         fromfile='scripts/test-swift.sh', tofile=label + '/scripts/test-swift.sh')))
        if diagnostic is None:
            assert result.returncode == 0 and not result.stderr, (label, result.returncode, result.stderr)
            assert 'verification targets classified' in result.stdout, label
        else:
            assert result.returncode == 1 and diagnostic in result.stderr, (label, result.returncode, result.stderr)
            assert 'verification targets classified' not in result.stdout, label
        if code_case == 'failure':
            assert 'fixture-module-exit-47' in result.stderr and '[module-failure]' in result.stderr, result
        if code_case == 'syntax':
            assert 'SyntaxError' in result.stderr and '[module-failure]' in result.stderr, result
        if code_case is not None:
            (receipt / 'validator.sh').write_text(selected_validator.read_text())
            if module.is_file() and code_case != 'unreadable':
                (receipt / 'module.py').write_bytes(module.read_bytes())
            (receipt / 'module-state.json').write_text(json.dumps(dict(
                case=code_case, path=str(module), exists=module.exists(),
                mode=oct(module.stat().st_mode) if module.exists() else None), indent=2) + '\n')
            if code_case == 'unreadable':
                module.chmod(0o644)
        assert not (repo / 'EXECUTED').exists(), 'validator executed candidate shell text'
        passed += 1
        print(f'PASS {label}: exit={result.returncode}, diagnostic={diagnostic or "none"}', flush=True)

    check('positive-current')
    # Only whole-stage boundaries permit extra comments; continued code is exact.
    check('positive-boundary-comments', helper='# preamble\n' + helper_source.replace(
        'echo "=== Apple', '# boundary\n\necho "=== Apple'))
    for label, new in (
        ('removed-invocation', ''), ('comment-invocation', '\t#' + invocation[1:]),
        ('wrong-helper', invocation.replace('scripts/test-swift.sh', 'scripts/disconnected.sh')),
        ('wrong-bridge', invocation.replace('"$(ENGINE_BRIDGE)"', '"other-bridge"')),
        ('swapped-arguments', invocation.replace('"$(PROFILE)" "$(ENGINE_BRIDGE)"', '"$(ENGINE_BRIDGE)" "$(PROFILE)"')),
        ('dynamic-helper', invocation.replace('scripts/test-swift.sh', '$(SWIFT_HELPER)')),
        ('swallowed-helper', invocation.replace('$(SHELL)', 'true || $(SHELL)')),
    ):
        check(label, make=replace(make_source, invocation, new), diagnostic='[invocation]')
    check('missing-target', make=replace(make_source, rule, ''), diagnostic='[make-shape]')
    check('missing-prerequisite', make=replace(make_source, 'test-swift: $(ENGINE_BRIDGE)', 'test-swift:'), diagnostic='[make-shape]')
    check('comment-prerequisite', make=replace(make_source, 'test-swift: $(ENGINE_BRIDGE)', 'test-swift: # $(ENGINE_BRIDGE)'), diagnostic='[make-shape]')
    check('duplicate-target', make=make_source + '\ntest-swift:\n\t@true\n', diagnostic='[bridge-prerequisite]')
    check('disabled-make-rule', make=replace(make_source, rule, 'ifeq (1,0)\n' + rule + 'endif\n'), diagnostic='[make-shape]')
    check('rule-inside-define', make=replace(make_source, rule, 'define DISCONNECTED\n' + rule + 'endef\n'), diagnostic='[make-shape]')
    check('override-define-rule', make=replace(make_source, rule, 'override define DISCONNECTED\n' + rule + 'endef\n'), diagnostic='[make-shape]')
    check('indented-disabled-rule', make=replace(make_source, rule, ' ifeq (1,0)\n' + rule + ' endif\n'), diagnostic='[make-shape]')
    check('continued-comment-rule', make=replace(make_source, rule, '# disconnected \\\n' + rule), diagnostic='[make-shape]')
    check('alternate-setup-define', make=make_source + '\noverride define TEST_DATA_DIR_SETUP\ntrue\nendef\n', diagnostic='[make-shape]')
    check('alternate-shell', make=make_source + '\n SHELL := /bin/false\n', diagnostic='[make-shape]')
    check('ignore-Make-failure', make=make_source + '\n.IGNORE: test-swift\n', diagnostic='[make-shape]')
    check('dynamic-Make-include', make=make_source + '\n include other.mk\n', diagnostic='[make-shape]')
    check('helper-CRLF', helper=helper_source.replace('\n', '\r\n'), diagnostic='[source-bytes]')
    check('late-isolation', make=replace(make_source, rule, rule.replace(
        '\t@$(TEST_DATA_DIR_SETUP); \\\n', '').rstrip() + '; \\\n\t$(TEST_DATA_DIR_SETUP)\n'), diagnostic='[invocation]')
    check('separate-shell-isolation', make=replace(make_source, rule, rule.replace(
        '\t@$(TEST_DATA_DIR_SETUP); \\\n', '\t@$(TEST_DATA_DIR_SETUP)\n')), diagnostic='[invocation]')
    check('missing-helper', helper=None, diagnostic='[helper-missing]')
    for label, new in (
        ('missing-selftest', ''), ('comment-selftest', '# ' + selftest),
        ('no-selftest-failfast', selftest.split(' ||')[0]),
        ('swallowed-selftest', selftest.replace('exit $?', 'true')),
        ('selftest-function', 'unrelated() {\n' + selftest + '\n}'),
        ('selftest-unreachable', 'if false; then\n' + selftest + '\nfi'),
        ('selftest-heredoc', "cat <<'MARKER'\n" + selftest + '\nMARKER'),
        ('selftest-after-XCTest', 'xcodebuild test\n' + selftest),
        ('selftest-early-exit', 'exit 0\n' + selftest),
    ):
        check(label, helper=replace(helper_source, selftest, new), diagnostic='[self-test]')
    check('different-helper-argument', helper=replace(helper_source, 'ENGINE_BRIDGE="$2"', 'ENGINE_BRIDGE="$1"'), diagnostic='[bindings]')
    check('helper-never-executed', helper='touch EXECUTED\n' + helper_source, diagnostic='[bindings]')
    for label, new in (
        ('missing-generation', ''), ('comment-generation', '# ' + generation),
        ('no-generation-failfast', generation.split(' ||')[0]),
        ('swallowed-generation', generation.replace('exit $?', 'true')),
        ('generation-after-XCTest', 'xcodebuild test\n' + generation),
        ('generation-function', 'unrelated() {\n' + generation + '\n}'),
        ('generation-unreachable', 'if false; then\n' + generation + '\nfi'),
    ):
        check(label, helper=replace(helper_source, generation, new), diagnostic='[generation]')
    check('missing-XCTest', helper=replace(helper_source, 'xcodebuild test \\', '# xcodebuild test \\'), diagnostic='[XCTest]')
    check('swallowed-XCTest', helper=replace(helper_source, 'rc=${PIPESTATUS[0]}', 'rc=0'), diagnostic='[XCTest]')
    check('extra-helper-code', helper=helper_source + 'true\n', diagnostic='[helper-shape]')
    check('setup-shadow', make=make_source + '\nTEST_DATA_DIR_SETUP := true\n', diagnostic='[isolation]')
    check('comment-mktemp', make=replace(make_source, 'CODESCRIBE_TEST_DATA_DIR="$$(mktemp', '# CODESCRIBE_TEST_DATA_DIR="$$(mktemp'), diagnostic='[isolation]')
    check('missing-mktemp', make=replace(make_source, 'CODESCRIBE_TEST_DATA_DIR="$$(mktemp -d', 'CODESCRIBE_TEST_DATA_DIR="$$(mkdir'), diagnostic='must create a unique directory')
    check('missing-export', make=replace(make_source, 'export CODESCRIBE_DATA_DIR="$$CODESCRIBE_TEST_DATA_DIR"', 'CODESCRIBE_DATA_DIR="$$CODESCRIBE_TEST_DATA_DIR"'), diagnostic='must export CODESCRIBE_DATA_DIR')
    check('missing-cleanup', make=replace(make_source, 'trap cleanup_codescribe_test_data_dir EXIT', 'true'), diagnostic='must clean its exact mktemp directory')
    check('ledger-unclassified', make=make_source + '\ntest-counterexample:\n\t@true\n', diagnostic="'test-counterexample' is a verification target with no GATE LEDGER row")
    check('ledger-stale', make=make_source + '\n# gate: test-absent class=hermetic ci=no -- absent\n', diagnostic="ledger row 'test-absent' names no verification target")
    check('ledger-illegal-class', make=replace(make_source, '# gate: verify class=hermetic', '# gate: verify class=fiction'), diagnostic="'verify' has class=fiction")
    check('ci-drift', workflow='steps:\n  - run: make test-swift\n', diagnostic="'test-swift' claims ci=no")

    # Packaging failures cannot borrow the real checkout or candidate-local code.
    check('module-code-root-spaces', code_case='valid')
    check('module-candidate-mutant', code_case='valid',
          helper=replace(helper_source, selftest, 'exit 0\n' + selftest), diagnostic='[self-test]')
    for code_case in ('missing', 'unreadable', 'directory', 'wrong-path'):
        check('module-' + code_case, code_case=code_case, diagnostic='[module-missing]')
    check('module-failure', code_case='failure', diagnostic='[module-failure]')
    check('module-syntax', code_case='syntax', diagnostic='[module-failure]')

    # Execute only the extracted, unchanged verify recipe with tool stand-ins.
    # Do not parse/evaluate the full Makefile: it has unrelated shell expansions.
    make_bin = shutil.which('make')
    if sys.platform == 'darwin':
        make_bin = subprocess.check_output(['/usr/bin/xcrun', '--find', 'make'], text=True).strip()
        native = Path(make_bin).with_name('gnumake')
        if native.is_file():
            make_bin = str(native)
    wiring = base / 'wiring'
    wiring.mkdir()
    setup = make_source.split('define TEST_DATA_DIR_SETUP\n', 1)[1].split('\nendef', 1)[0]
    (wiring / 'Makefile').write_text('SHELL := /bin/bash\ndefine TEST_DATA_DIR_SETUP\n' + setup
                                  + '\nendef\n' + target(make_source, 'verify'))
    fake = base / 'fake'
    fake.mkdir()
    stub = '''#!PYTHON
import json, os, sys
from pathlib import Path
with open(os.environ['CALLS'], 'a') as out:
    out.write(json.dumps([Path(sys.argv[0]).name] + sys.argv[1:]) + '\\n')
if sys.argv[1:] == ['scripts/tests/validate-gates-test.sh']:
    assert Path(os.environ['CODESCRIBE_DATA_DIR']).is_dir()
    sys.exit(int(os.environ['HARNESS_EXIT']))
sys.exit(0)
'''.replace('PYTHON', sys.executable)
    for tool in ('python3', 'cargo', 'bash'):
        (fake / tool).write_text(stub)
        (fake / tool).chmod(0o755)
    for harness_exit in (0, 47):
        calls_path = evidence / f'verify-{harness_exit}.calls'
        calls_path.write_text('')
        result = subprocess.run([make_bin, '-f', str(wiring / 'Makefile'), 'verify'], cwd=wiring,
                                env=dict(env, PATH=str(fake) + ':' + str(bin_dir),
                                         CALLS=str(calls_path), HARNESS_EXIT=str(harness_exit)),
                                text=True, capture_output=True)
        (evidence / f'verify-{harness_exit}.stdout').write_text(result.stdout)
        (evidence / f'verify-{harness_exit}.stderr').write_text(result.stderr)
        (evidence / f'verify-{harness_exit}.exit').write_text(str(result.returncode) + '\n')
        calls = [json.loads(line) for line in calls_path.read_text().splitlines()]
        assert calls.count(['bash', 'scripts/tests/validate-gates-test.sh']) == 1, calls
        assert (result.returncode == 0) == (harness_exit == 0), result
        assert ('verify: hermetic gate passed.' in result.stdout) == (harness_exit == 0), result
        if harness_exit:
            assert 'Error 47' in result.stderr, result
            assert calls[-1] == ['bash', 'scripts/tests/validate-gates-test.sh'], calls
        passed += 1
        print(f'PASS verify-wiring-{harness_exit}: make exit={result.returncode}', flush=True)

    # Reuse all 32 scenarios, then independently inspect their recorded order.
    swift_evidence = evidence / 'swift-path'
    result = subprocess.run(['/bin/bash', str(source / 'scripts/tests/test-swift-target-root-test.sh'),
                             str(source), str(swift_evidence)], text=True, capture_output=True)
    (evidence / 'swift-path.stdout').write_text(result.stdout)
    (evidence / 'swift-path.stderr').write_text(result.stderr)
    (evidence / 'swift-path.exit').write_text(str(result.returncode) + '\n')
    assert result.returncode == 0 and 'scenarios=32 passed=32 failed=0' in result.stdout, result
    receipts = list(swift_evidence.glob('*.json'))
    assert len(receipts) == 32, len(receipts)
    for path in receipts:
        receipt = json.loads(path.read_text())
        names = [c['tool'] for c in receipt['calls'] if 'selected' not in c]
        if receipt['exit'] == 0:
            ordered = [n for n in names if n not in ('security', 'git')]
            assert ordered in (['codescribe-stt-bridge', 'cargo', 'xcodegen', 'xcodebuild'],
                               ['custom-helper', 'cargo', 'xcodegen', 'xcodebuild']), (path, names)
        if path.stem.endswith('-helper-error'):
            assert not set(names) & {'cargo', 'xcodegen', 'xcodebuild'}, names
        if path.stem.endswith('-generation-error'):
            assert 'xcodegen' in names and 'xcodebuild' not in names, names
        if path.stem.endswith('-metadata-error'):
            assert 'cargo' in names and not set(names) & {'xcodegen', 'xcodebuild'}, names
    passed += 32
    print('PASS real Make/helper: 32 scenarios plus event-order/short-circuit assertions', flush=True)
print(f'validate-gates-test: scenarios={passed} passed={passed} failed=0')
PY
