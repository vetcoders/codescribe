"""Exercise the release generator's appcast identity gate through its CLI."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "verify-appcast-version.py"


class AppcastVersionTests(unittest.TestCase):
    def run_xml(self, xml, expected="0.17.0"):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "appcast.xml"
            path.write_bytes(xml if isinstance(xml, bytes) else xml.encode())
            return subprocess.run(
                [sys.executable, str(SCRIPT), str(path), "--version", expected],
                capture_output=True, text=True,
            )

    def run_feed(self, internal="0.17.0", display="0.17.0", *, copies=1, expected="0.17.0"):
        item = (
            f"<item><sparkle:version>{internal}</sparkle:version>"
            f"<sparkle:shortVersionString>{display}</sparkle:shortVersionString>"
            '<enclosure url="https://example.invalid/Codescribe.dmg"/></item>'
        )
        xml = (
            '<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">'
            f"<channel>{item * copies}</channel></rss>"
        )
        return self.run_xml(xml, expected)

    def test_dtds_are_rejected_before_entity_expansion_in_both_encodings(self):
        for declaration in (
            '<!DOCTYPE rss [<!ENTITY value "0.17.0">]>',
            '<!DOCTYPE rss [<!ENTITY value SYSTEM "file:///etc/passwd">]>',
            '<!DOCTYPE rss SYSTEM "https://example.invalid/external.dtd">',
        ):
            for encoding in ("utf-8", "utf-16"):
                with self.subTest(declaration=declaration, encoding=encoding):
                    result = self.run_xml((declaration + "<rss/>").encode(encoding))
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("DTDs and entities are prohibited", result.stderr)

    def test_oversized_and_malformed_documents_are_rejected(self):
        for xml in ("<rss>" + " " * (1024 * 1024) + "</rss>", "<rss>"):
            result = self.run_xml(xml)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("Traceback", result.stderr)

    def test_canonical_release_is_accepted(self):
        result = self.run_feed()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_historical_commit_counter_is_rejected(self):
        result = self.run_feed("1838", "0.15.2", expected="0.15.2")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sparkle:version", result.stderr)

    def test_marketing_version_cannot_disagree_with_signed_bundle_identity(self):
        result = self.run_feed(display="0.15.2")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sparkle:shortVersionString", result.stderr)

    def test_empty_or_multiple_stable_items_are_rejected(self):
        for copies in (0, 2):
            with self.subTest(copies=copies):
                result = self.run_feed(copies=copies)
                self.assertNotEqual(result.returncode, 0)

    def test_only_canonical_stable_versions_are_publishable(self):
        for version in ("0.17", "0.017.0", "0.17.0-beta", "1149"):
            with self.subTest(version=version):
                result = self.run_feed(version, version, expected=version)
                self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
