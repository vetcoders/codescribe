"""Keep native fixture presentation behind the single desktop-isolation owner."""

from pathlib import Path
import re
import unittest


class WindowPresentationBoundaryTests(unittest.TestCase):
    def test_native_fixtures_cannot_bypass_the_presentation_owner(self):
        tests = Path(__file__).resolve().parents[2] / "macos" / "CodescribeTests"
        direct_presentation = re.compile(
            r"\.(?:orderFrontRegardless|orderFront|makeKeyAndOrderFront)\s*\("
        )
        violations = []
        for source in sorted(tests.glob("*.swift")):
            if source.name == "BackgroundTestWindow.swift":
                continue
            for line_number, line in enumerate(source.read_text().splitlines(), 1):
                if direct_presentation.search(line):
                    violations.append(f"{source.name}:{line_number}: {line.strip()}")
        self.assertEqual(
            violations,
            [],
            "Use BackgroundTestWindow for fixture presentation; direct front-ordering "
            "can put test windows over the Founder's desktop:\n" + "\n".join(violations),
        )


if __name__ == "__main__":
    unittest.main()
