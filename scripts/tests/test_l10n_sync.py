"""Real-compiler controls for the sync gate; all copy lives in a disposable tree."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class LocalizationSyncTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if sys.platform != "darwin":
            raise RuntimeError("test-l10n-sync requires macOS and Xcode; no skipped proof")
        subprocess.run(["xcrun", "--find", "swiftc"], check=True, capture_output=True)
        subprocess.run(["xcrun", "--find", "xcstringstool"], check=True, capture_output=True)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codescribe-l10n-control-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        scripts = self.repo / "scripts"
        scripts.mkdir()
        shutil.copyfile(Path(__file__).resolve().parents[1] / "l10n-sync.sh", scripts / "l10n-sync.sh")
        self.source_root = self.repo / "macos/Codescribe"
        self.catalog = self.source_root / "Resources/Localization/Localizable.xcstrings"
        self.catalog.parent.mkdir(parents=True)
        self.catalog.write_text(json.dumps({"sourceLanguage": "en", "strings": {}, "version": "1.0"}))
        self.source = self.source_root / "Probe.swift"
        self.source.write_text(
            'import SwiftUI\n'
            'struct Probe: View {\n'
            '  var body: some View {\n'
            '    Button("Gate positive") {}\n'
            '      .help("Gate tooltip")\n'
            '      .accessibilityLabel("Gate accessibility")\n'
            '  }\n'
            '}\n'
            '#if DEBUG\n'
            'let debugCopy = String(localized: "Gate Debug")\n'
            '#endif\n'
            'let identifierCopy = String(localized: "gate.identifier", defaultValue: "Gate default value")\n'
        )
        self.derived = self.repo / "derived"
        self.objects = self.derived / (
            "Build/Intermediates.noindex/Codescribe.build/Debug/"
            "Codescribe.build/Objects-normal/arm64"
        )
        self.objects.mkdir(parents=True)
        self.env = dict(os.environ, L10N_DERIVED=str(self.derived), L10N_CONFIG="Debug")

    def compile(self):
        # Module emission uses compiler extraction without linking an executable.
        # Fresh files prevent unchanged-extraction mtimes from masking the test.
        for data in self.objects.glob("*.stringsdata"):
            data.unlink()
        result = subprocess.run(
            ["xcrun", "swiftc", "-emit-module", "-parse-as-library", "-swift-version", "6",
             "-strict-concurrency=complete", "-warnings-as-errors", "-D", "DEBUG",
             "-emit-localized-strings", "-emit-localized-strings-path", str(self.objects),
             "-emit-module-path", str(self.objects / "Probe.swiftmodule"), str(self.source)],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((self.objects / "Probe.stringsdata").is_file())

    def sync(self, check=False):
        before = self.catalog.read_bytes()
        result = subprocess.run(
            ["bash", "scripts/l10n-sync.sh"] + (["--check"] if check else []),
            cwd=self.repo, env=self.env, capture_output=True, text=True,
        )
        if check:
            self.assertEqual(self.catalog.read_bytes(), before, "--check modified the catalog")
        return result

    def seed(self):
        self.compile()
        result = self.sync()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_positive_and_formatting_only_changes(self):
        self.seed()
        strings = json.loads(self.catalog.read_text())["strings"]
        self.assertTrue({"Gate positive", "Gate tooltip", "Gate accessibility", "Gate Debug"} <= strings.keys())
        self.catalog.write_text(json.dumps(json.loads(self.catalog.read_text()), indent=4) + "\n")
        result = self.sync(check=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_new_user_facing_swift_string_fails_without_catalog_sync(self):
        self.seed()
        with self.source.open("a") as handle:
            handle.write('\nstruct Negative: View { var body: some View { Text("Gate negative new copy") } }\n')
        self.compile()
        result = self.sync(check=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Gate negative new copy", result.stdout)
        self.assertIn("make l10n-sync", result.stderr)
        self.assertIn("make verify-l10n-catalog", result.stderr)

    def test_changed_user_facing_swift_string_fails_without_catalog_sync(self):
        self.seed()
        self.source.write_text(self.source.read_text().replace("Gate positive", "Gate changed copy"))
        self.compile()
        result = self.sync(check=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Gate changed copy", result.stdout)

    def test_changed_default_value_with_the_same_key_fails_without_catalog_sync(self):
        self.seed()
        self.source.write_text(self.source.read_text().replace("Gate default value", "Gate changed default"))
        self.compile()
        result = self.sync(check=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_missing_or_malformed_extraction_fails_closed(self):
        self.seed()
        data = self.objects / "Probe.stringsdata"
        for content in (None, "not JSON"):
            with self.subTest(content=content):
                if content is None:
                    data.unlink()
                else:
                    data.write_text(content)
                result = self.sync(check=True)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn("never compiled", result.stderr)

    def test_new_swift_file_without_extraction_fails_closed(self):
        self.seed()
        (self.source_root / "New.swift").write_text('import SwiftUI\nlet newText = Text("Uncompiled copy")\n')
        result = self.sync(check=True)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("New.swift", result.stderr)
        self.assertIn("never compiled", result.stderr)

    def test_edited_source_without_rebuild_fails_closed(self):
        self.seed()
        newer = (self.objects / "Probe.stringsdata").stat().st_mtime + 10
        os.utime(self.source, (newer, newer))
        result = self.sync(check=True)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("edited since the build", result.stderr)


if __name__ == "__main__":
    unittest.main()
