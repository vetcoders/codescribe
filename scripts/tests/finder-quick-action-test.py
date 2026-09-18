#!/usr/bin/env python3
"""Exercise the installed Automator command without touching Finder or clipboard."""
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest


class FinderQuickActionTests(unittest.TestCase):
    def test_selection_preserves_existing_results_and_continues_after_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            installer = Path(__file__).resolve().parents[1] / "install-finder-quick-action.sh"
            subprocess.run(["/bin/zsh", str(installer), str(root)], check=True, capture_output=True)
            info = root / "Transcribe with Codescribe.workflow/Contents/Info.plist"
            with info.open("rb") as handle:
                service = plistlib.load(handle)["NSServices"][0]
            self.assertEqual(service["NSRequiredContext"]["NSApplicationIdentifier"], "com.apple.finder")
            workflow = root / "Transcribe with Codescribe.workflow/Contents/document.wflow"
            with workflow.open("rb") as handle:
                document = plistlib.load(handle)
            metadata = document["workflowMetaData"]
            finder = "/System/Library/CoreServices/Finder.app"
            self.assertEqual(metadata["applicationBundleID"], "com.apple.finder")
            self.assertEqual(metadata["applicationPath"], finder)
            self.assertEqual(metadata["applicationPaths"], [finder])
            self.assertEqual(metadata["applicationBundleIDsByPath"], {finder: "com.apple.finder"})
            command = document["actions"][0]["action"]["ActionParameters"]["COMMAND_STRING"]
            fake = root / "codescribe"
            fake.write_text(
                '#!/bin/zsh\n'
                '[[ "$1" == transcribe && "$2" == --no-bus ]] || exit 9\n'
                '[[ "$3" == *broken.wav ]] && { print partial; exit 1; }\n'
                'print -r -- "transcript: ${3:t}"\n'
            )
            fake.chmod(0o755)
            for name, body in (("pbcopy", 'cat > "$TEST_CLIPBOARD"'), ("osascript", "exit 0")):
                stub = root / name
                stub.write_text("#!/bin/zsh\n" + body + "\n")
                stub.chmod(0o755)
            sources = [root / name for name in ("existing.wav", "broken.wav", "two words.wav", "last.m4a")]
            for source in sources:
                source.touch()
            existing = root / "existing.txt"
            existing.write_text("valuable prior transcript")
            clipboard = root / "clipboard"
            env = dict(os.environ, CODESCRIBE_BIN=str(fake), TEST_CLIPBOARD=str(clipboard))
            env["PATH"] = str(root) + ":" + env["PATH"]
            result = subprocess.run(["/bin/zsh", "-c", command, "finder-test", *map(str, sources)], env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(existing.read_text(), "valuable prior transcript")
            self.assertFalse((root / "broken.txt").exists())
            self.assertEqual((root / "two words.txt").read_text(), "transcript: two words.wav\n")
            self.assertEqual((root / "last.txt").read_text(), "transcript: last.m4a\n")
            self.assertEqual(clipboard.read_text(), "transcript: two words.wav\n\ntranscript: last.m4a\n\n")
            self.assertEqual(list(root.glob("*.txt.*")), [])


if __name__ == "__main__":
    unittest.main()
