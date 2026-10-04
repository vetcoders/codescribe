"""Publication contracts for the macOS UniFFI binding generator."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(sys.platform == "darwin", "producer uses macOS sed")
class BindingPublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codescribe-bindings-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.destination = self.root / "Bridge with spaces"
        self.destination.mkdir()
        self.spec = self.root / "output.json"
        self.bindgen = self.root / "fake-bindgen"
        self.bindgen.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "stage = pathlib.Path(sys.argv[sys.argv.index('--out-dir') + 1])\n"
            "spec = json.loads(pathlib.Path(os.environ['BINDINGS_TEST_SPEC']).read_text())\n"
            "for name, data in spec['files'].items():\n"
            "    (stage / name).write_bytes(data.encode('utf-8'))\n"
            "if spec.get('invalid') == 'symlink':\n"
            "    (stage / 'bad.modulemap').symlink_to(os.environ['BINDINGS_TEST_SPEC'])\n"
            "if spec.get('invalid') == 'directory':\n"
            "    (stage / 'directory').mkdir()\n"
            "sys.exit(spec.get('exit', 0))\n"
        )
        self.bindgen.chmod(0o755)
        self.helper = Path(__file__).resolve().parents[2] / "scripts/lib/generate-swift-bindings.sh"

    def run_generator(self, files, **options):
        self.spec.write_text(json.dumps(dict(files=files, **options)))
        result = subprocess.run(
            ["bash", str(self.helper), str(self.bindgen), "library with spaces.dylib", str(self.destination)],
            env=dict(os.environ, BINDINGS_TEST_SPEC=str(self.spec)),
            capture_output=True, text=True,
        )
        self.assertEqual(list(self.destination.glob('.uniffi.*')), [])
        return result

    def snapshot(self):
        return {p.name: (p.read_bytes(), p.stat().st_mtime_ns, p.stat().st_ino)
                for p in self.destination.iterdir()}

    def seed(self):
        self.assertEqual(self.run_generator({"bridge.swift": "let value = 1\n", "bridge.h": "int value;\n", "bridge.modulemap": "module Bridge {}\n"}).returncode, 0)
        for p in self.destination.iterdir():
            os.utime(p, ns=(1_500_000_000_000_000_000, 1_500_000_000_000_000_000))

    def test_identical_normalized_files_preserve_inode_and_nanosecond_mtime(self):
        self.seed()
        before = self.snapshot()
        result = self.run_generator({"bridge.swift": "let value = 1  \t\n\n", "bridge.h": "int value; \n\n", "bridge.modulemap": "module Bridge {}\n"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.snapshot(), before)

    def test_only_changed_file_is_replaced(self):
        self.seed()
        before = self.snapshot()
        result = self.run_generator({"bridge.swift": "let value = 2\n", "bridge.h": "int value;\n", "bridge.modulemap": "module Bridge {}\n"})
        self.assertEqual(result.returncode, 0, result.stderr)
        after = self.snapshot()
        self.assertEqual(after['bridge.h'], before['bridge.h'])
        self.assertEqual(after['bridge.modulemap'], before['bridge.modulemap'])
        self.assertEqual(after['bridge.swift'][0], b'let value = 2\n')
        self.assertNotEqual(after['bridge.swift'][1:], before['bridge.swift'][1:])

    def test_changed_header_and_modulemap_are_published(self):
        self.seed()
        before = self.snapshot()
        result = self.run_generator({"bridge.swift": "let value = 1\n", "bridge.h": "int changed;\n", "bridge.modulemap": "module Changed {}\n"})
        self.assertEqual(result.returncode, 0, result.stderr)
        after = self.snapshot()
        self.assertEqual(after['bridge.swift'], before['bridge.swift'])
        self.assertEqual(after['bridge.h'][0], b'int changed;\n')
        self.assertEqual(after['bridge.modulemap'][0], b'module Changed {}\n')

    def test_initial_publish_normalizes_swift_header_but_preserves_modulemap(self):
        result = self.run_generator({"bridge.swift": "// żółć  \t\nlet x = 1\t", "bridge.h": "int x; \t\n\n\n", "empty.swift": "", "bridge.modulemap": "module Bridge {}  \n\n"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.destination / 'bridge.swift').read_bytes(), '// żółć\nlet x = 1\n'.encode())
        self.assertEqual((self.destination / 'bridge.h').read_bytes(), b'int x;\n')
        self.assertEqual((self.destination / 'empty.swift').read_bytes(), b'')
        self.assertEqual((self.destination / 'bridge.modulemap').read_bytes(), b'module Bridge {}  \n\n')

    def test_partial_generator_failure_preserves_existing_outputs(self):
        self.seed()
        before = self.snapshot()
        result = self.run_generator({"bridge.swift": "partial\n"}, exit=42)
        self.assertEqual(result.returncode, 42)
        self.assertEqual(self.snapshot(), before)

    def test_partial_generator_failure_leaves_initial_destination_empty(self):
        result = self.run_generator({"bridge.swift": "partial\n"}, exit=42)
        self.assertEqual(result.returncode, 42)
        self.assertEqual(self.snapshot(), {})

    def test_empty_stage_is_rejected_without_publication(self):
        self.seed()
        before = self.snapshot()
        self.assertNotEqual(self.run_generator({}).returncode, 0)
        self.assertEqual(self.snapshot(), before)

    def test_nonregular_stage_is_rejected_without_publication(self):
        self.seed()
        before = self.snapshot()
        for invalid in ('symlink', 'directory'):
            with self.subTest(invalid=invalid):
                result = self.run_generator({'bridge.swift': 'changed\n'}, invalid=invalid)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.snapshot(), before)

    def test_normalizer_failure_is_not_published(self):
        self.seed()
        before = self.snapshot()
        fake_bin = self.root / 'bin'
        fake_bin.mkdir()
        fake_sed = fake_bin / 'sed'
        fake_sed.write_text('#!/bin/sh\nexit 43\n')
        fake_sed.chmod(0o755)
        old_path = os.environ['PATH']
        try:
            os.environ['PATH'] = str(fake_bin) + ':' + old_path
            result = self.run_generator({'bridge.swift': 'changed  \n'})
        finally:
            os.environ['PATH'] = old_path
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.snapshot(), before)


if __name__ == '__main__':
    unittest.main()
