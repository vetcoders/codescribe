"""Build-channel normalization must agree with the app's boundary trimming."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "verify-dmg-payload.sh"


class BuildChannelTests(unittest.TestCase):
    def test_channel_normalization_preserves_internal_whitespace(self):
        source = SCRIPT.read_text()
        start = source.index('    BUILD_CHANNEL="${BUILD_CHANNEL#')
        end = source.index('    if [[ "$BUILD_CHANNEL" == ', start)
        normalize = source[start:end]
        for raw, expected in [
            ("NIGHTLY", "nightly"),
            (" \tNIGHTLY\r\n", "nightly"),
            ("NIGHT LY", "night ly"),
            (" NIGHT\tLY ", "night\tly"),
            ("stable", "stable"),
            (" \t\r\n", ""),
            ("", ""),
        ]:
            with self.subTest(channel=raw):
                result = subprocess.run(
                    ["bash", "-c", 'BUILD_CHANNEL="$1"\n' + normalize
                     + '\nprintf "%s" "$BUILD_CHANNEL"', "channel-fixture", raw],
                    check=True, capture_output=True, text=True,
                )
                self.assertEqual(result.stdout, expected)


class ProductBundleTests(unittest.TestCase):
    def test_payload_gate_selects_x9bus_and_does_not_accept_the_old_bundle(self):
        source = SCRIPT.read_text()
        start = source.index("find_expected_app_bundle() {")
        end = source.index("\n}\n", start) + 3
        function = source[start:end]
        command = (
            f'APP_NAME="${{CODESCRIBE_APP_NAME:-X9bus}}"\n'
            f'{function}\n'
            'find_expected_app_bundle "$1"'
        )
        environment = {**os.environ, "CODESCRIBE_APP_NAME": "X9bus"}
        with tempfile.TemporaryDirectory() as temporary:
            mount = Path(temporary)
            (mount / "X9bus.app").mkdir()
            result = subprocess.run(
                ["bash", "-c", command, "bundle-selection", str(mount)],
                check=True,
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(Path(result.stdout.strip()).name, "X9bus.app")

            (mount / "X9bus.app").rmdir()
            result = subprocess.run(
                ["bash", "-c", command, "legacy-bundle-only", str(mount)],
                check=True,
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
