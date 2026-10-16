#!/usr/bin/env python3
"""Check the release identity emitted by Sparkle's signed appcast generator."""

import argparse
import re
from pathlib import Path
import xml.etree.ElementTree as ET
from xml.parsers import expat

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
RELEASE = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")
MAX_APPCAST_BYTES = 1024 * 1024


def read_appcast(path: Path) -> ET.Element:
    # Appcasts need neither DTDs nor entities. Reject them before expansion,
    # including UTF-16 declarations, and bound allocation before parsing.
    with path.open("rb") as source:
        data = source.read(MAX_APPCAST_BYTES + 1)
    if len(data) > MAX_APPCAST_BYTES:
        raise ValueError("appcast exceeds 1 MiB")
    tree = ET.TreeBuilder()
    parser = expat.ParserCreate(namespace_separator="}")

    def name(value):
        return "{" + value if "}" in value else value

    def reject_declaration(*_):
        raise ValueError("appcast DTDs and entities are prohibited")

    parser.StartDoctypeDeclHandler = reject_declaration
    parser.EntityDeclHandler = reject_declaration
    parser.ExternalEntityRefHandler = reject_declaration
    parser.StartElementHandler = lambda tag, attrs: tree.start(
        name(tag), {name(key): value for key, value in attrs.items()}
    )
    parser.EndElementHandler = lambda tag: tree.end(name(tag))
    parser.CharacterDataHandler = tree.data
    parser.Parse(data, True)
    return tree.close()


def verify(path: Path, version: str) -> None:
    if not RELEASE.fullmatch(version):
        raise ValueError("expected a stable major.minor.patch release version")
    items = read_appcast(path).findall("./channel/item")
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
    except (ValueError, expat.ExpatError, OSError) as error:
        parser.exit(1, f"appcast version: {error}\n")
    print(f"appcast version: {args.version} (release identity verified)")


if __name__ == "__main__":
    main()
