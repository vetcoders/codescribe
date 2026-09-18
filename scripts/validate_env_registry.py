#!/usr/bin/env python3
"""Validate the whole env registry and print sorted names; requires Python 3.11+."""

import argparse
from pathlib import Path
import re
import sys

try:
    import tomllib
except ImportError:
    print(
        "ERROR: Python 3.11+ with stdlib tomllib is required to validate the env registry",
        file=sys.stderr,
    )
    sys.exit(1)


def registered_names(path):
    """Return names only after TOML syntax and the variable table shape pass."""
    with path.open("rb") as source:
        registry = tomllib.load(source)
    variables = registry.get("vars")
    if not isinstance(variables, dict) or not variables:
        raise ValueError("registry must contain non-empty [vars.NAME] tables")
    for name, fields in variables.items():
        if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", name):
            raise ValueError("invalid environment variable name: " + repr(name))
        if not isinstance(fields, dict):
            raise ValueError("vars." + name + " must be a table")
    return sorted(variables)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("registry", type=Path)
    args = parser.parse_args()
    try:
        names = registered_names(args.registry)
    except (OSError, ValueError) as error:
        print("ERROR: invalid env registry {}: {}".format(args.registry, error), file=sys.stderr)
        return 1
    print("\n".join(names))
    return 0


if __name__ == "__main__":
    sys.exit(main())
