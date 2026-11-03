"""Build-channel normalization must agree with the app's boundary trimming."""

from pathlib import Path
import subprocess
import unittest


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


if __name__ == "__main__":
    unittest.main()
