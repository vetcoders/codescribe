#!/usr/bin/env python3
"""Check the release identity emitted by Sparkle's signed appcast generator."""

import argparse
import re
from pathlib import Path
import xml.etree.ElementTree as ET

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
RELEASE = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")


def verify(path: Path, version: str) -> None:
    if not RELEASE.fullmatch(version):
        raise ValueError("expected a stable major.minor.patch release version")
    items = ET.parse(path).getroot().findall("./channel/item")
    if len(items) != 1:
        raise ValueError("stable feed must contain exactly one release")
    item = items[0]
    enclosure = item.find("enclosure")
    if enclosure is None:
        raise ValueError("release enclosure is missing")
    for name in ("version", "shortVersionString"):
        actual = item.findtext(SPARKLE + name) or enclosure.get(SPARKLE + name)
        if actual != version:
            raise ValueError(f"sparkle:{name} must equal release {version}; got {actual!r}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("appcast", type=Path)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    try:
        verify(args.appcast, args.version)
    except (ValueError, ET.ParseError, OSError) as error:
        parser.exit(1, f"appcast version: {error}\n")
    print(f"appcast version: {args.version} (release identity verified)")


if __name__ == "__main__":
    main()
