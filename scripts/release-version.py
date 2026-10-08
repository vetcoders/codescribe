#!/usr/bin/env python3
"""Local version/changelog transaction. Git history describes source, not shipment."""

import argparse
import ast
import base64
import datetime
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def version_tuple(value):
    if not re.fullmatch(r"\d+\.\d+\.\d+", value):
        raise ValueError(f"Expected a stable major.minor.patch version: {value}")
    return tuple(map(int, value.split(".")))


def cargo_version(text):
    # Read only the two owning tables; dependency versions remain untouched.
    values = []
    for table in ("workspace.package", "package"):
        block = re.search(r"(?ms)^\[" + re.escape(table) + r"\]\s*\n(.*?)(?=^\[|\Z)", text)
        match = re.search(r'^version\s*=\s*"([^"]+)"', block[1], re.M) if block else None
        if not match:
            raise ValueError(f"Missing {table}.version")
        values.append(match[1])
    if values[0] != values[1]:
        raise ValueError("Workspace and package versions disagree; no files changed")
    version_tuple(values[0])
    return values[0]


def replace_cargo_version(text, current, target):
    for table in ("workspace.package", "package"):
        pattern = r"(?ms)(^\[" + re.escape(table) + r"\]\s*\n)(.*?)(?=^\[|\Z)"
        def update(match):
            body = re.sub(r'(?m)^(version\s*=\s*)"' + re.escape(current) + r'"',
                          lambda m: m[1] + '"' + target + '"', match[2], count=1)
            return match[1] + body
        text = re.sub(pattern, update, text)
    return text


def replace_lock_versions(root, cargo, lock, current, target):
    members = re.search(r"(?ms)^members\s*=\s*(\[.*?\])", cargo)
    paths = ast.literal_eval(members[1]) if members else ["."]
    names = set()
    for member in set(paths + ["."]):
        manifest = cargo if member == "." else (root / member / "Cargo.toml").read_text()
        block = re.search(r"(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)", manifest)
        if not block:
            raise ValueError(f"Missing package table for workspace member {member}")
        if member == "." or re.search(r"(?m)^version\.workspace\s*=\s*true", block[1]):
            name = re.search(r'^name\s*=\s*"([^"]+)"', block[1], re.M)
            if not name:
                raise ValueError(f"Missing package name for {member}")
            names.add(name[1])
    found = set()
    def update(match):
        body = match[1]
        name = re.search(r'^name = "([^"]+)"', body, re.M)
        if not name or name[1] not in names or re.search(r"(?m)^source\s*=", body):
            return match[0]
        if name[1] in found:
            raise ValueError(f"Duplicate local lockfile package: {name[1]}")
        found.add(name[1])
        version = re.search(r'^version = "([^"]+)"', body, re.M)
        if not version or version[1] not in (current, target):
            raise ValueError(f"Lockfile version disagrees for {name[1]}")
        body = re.sub(r'(?m)^version = "[^"]+"', 'version = "' + target + '"', body, count=1)
        return "[[package]]\n" + body
    result = re.sub(r"(?ms)^\[\[package\]\]\n(.*?)(?=^\[\[package\]\]|\Z)", update, lock)
    if found != names:
        raise ValueError(f"Lockfile lacks owning workspace packages: {sorted(names - found)}")
    return result


def atomic_write(path, data, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".release-version-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, mode)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def read_optional(path):
    return path.read_bytes() if path.exists() else None


def encode(data):
    return None if data is None else base64.b64encode(data).decode("ascii")


def decode(data):
    return None if data is None else base64.b64decode(data)


def recover(journal):
    if not journal.exists():
        return
    entries = json.loads(journal.read_text())["files"]
    # Validate the entire set before restoring anything; never overwrite a later edit.
    for entry in entries:
        actual = read_optional(Path(entry["path"]))
        if actual not in (decode(entry["before"]), decode(entry["after"])):
            raise ValueError(f"Interrupted bump meets a newer edit: {entry['path']}; preserve journal")
    if not all(read_optional(Path(e["path"])) == decode(e["after"]) for e in entries):
        for entry in reversed(entries):
            path = Path(entry["path"])
            before = decode(entry["before"])
            if read_optional(path) == before:
                continue
            if before is None:
                path.unlink(missing_ok=True)
            else:
                atomic_write(path, before, entry["mode"])
    journal.unlink()


def transact(changes, journal, expected=None):
    for path, before in (expected or {}).items():
        if read_optional(path) != before:
            raise ValueError(f"Concurrent edit while preparing bump: {path}")
    entries = [{"path": str(path), "before": encode(read_optional(path)),
                "after": encode(data), "mode": path.stat().st_mode & 0o777 if path.exists() else 0o644}
               for path, data in changes.items() if read_optional(path) != data]
    if not entries:
        return
    atomic_write(journal, (json.dumps({"files": entries}) + "\n").encode(), 0o600)
    try:
        for entry in entries:
            path = Path(entry["path"])
            if read_optional(path) != decode(entry["before"]):
                raise ValueError(f"Concurrent edit during bump: {path}")
            atomic_write(path, decode(entry["after"]), entry["mode"])
    except BaseException:
        recover(journal)
        raise
    journal.unlink()


def clean_subject(subject):
    subject = re.sub(r"^(?:\[[^\]]+\]\s*)+", "", subject.strip())
    subject = re.sub(r"https?://\S+|/(?:Users|private|Volumes)/\S+", "[local reference]", subject)
    return subject


def history(root, start, end):
    # Record each real commit and the headings preserved by GitHub squash commits.
    hashes = git(root, "rev-list", "--reverse", f"{start}..{end}").splitlines()
    commits = []
    for sha in hashes:
        raw = git(root, "show", "-s", "--format=%cs%n%s%n%b", sha).splitlines()
        headings = list(dict.fromkeys(clean_subject(line[2:]) for line in raw[2:]
                                     if line.startswith("* ") and line[2:].strip()))
        commits.append({"sha": sha, "date": raw[0], "subject": clean_subject(raw[1]),
                        "squash_headings": headings})
    return commits


def section(text, version):
    pattern = r"(?m)^## \[" + re.escape(version) + r"\][^\n]*\n"
    match = re.search(pattern, text)
    if not match:
        return None
    next_heading = re.search(r"(?m)^## ", text[match.end():])
    end = match.end() + next_heading.start() if next_heading else len(text)
    return match.start(), end


def previous_boundary(root, changelog, target, explicit):
    if explicit:
        return git(root, "rev-parse", explicit + "^{commit}")
    choices = []
    tagged_versions = set()
    for tag in git(root, "tag", "--merged", "HEAD").splitlines():
        value = tag.removeprefix("v")
        if re.fullmatch(r"\d+\.\d+\.\d+", value) and version_tuple(value) < version_tuple(target):
            tagged_versions.add(value)
            choices.append((version_tuple(value), git(root, "rev-parse", tag + "^{commit}")))
    for value, sha in re.findall(r'<!-- release-boundary: ([\d.]+) ([0-9a-f]{40}) -->', changelog):
        if value not in tagged_versions and version_tuple(value) < version_tuple(target):
            choices.append((version_tuple(value), sha))
    if not choices:
        raise ValueError("No earlier release boundary; supply --from <commit-or-tag>")
    return max(choices)[1]


def generated_notes(commits):
    buckets = {"Added": [], "Fixed": [], "Changed": [], "Documentation": []}
    seen = set()
    for commit in commits:
        for subject in commit["squash_headings"] or [commit["subject"]]:
            match = re.match(r"(feat|fix|docs|refactor|perf|chore|test|ci|build|style)(?:\([^)]*\))?!?:\s*(.*)", subject)
            kind, title = (match[1], match[2]) if match else ("change", subject)
            if kind in ("chore", "test", "ci", "build", "style"):
                continue
            if title.casefold() in seen:
                continue
            seen.add(title.casefold())
            bucket = {"feat": "Added", "fix": "Fixed", "docs": "Documentation"}.get(kind, "Changed")
            if len(buckets[bucket]) < 2:
                buckets[bucket].append(title[:220])
    return "\n\n".join("### " + key + "\n\n" + "\n".join("- " + title for title in titles)
                         for key, titles in buckets.items() if titles)


def update_changelog(text, version, date, end, commits, promote=False, summarize=True):
    span = section(text, version)
    marker_start = f"<!-- release-notes:{version}:start -->"
    marker_end = f"<!-- release-notes:{version}:end -->"
    if span and "<!-- release-summary: generated -->" not in text[span[0]:span[1]]:
        summarize = False  # Existing human release notes stay the main narrative.
    block = marker_start + "\n"
    if summarize:
        block += "<!-- release-summary: generated -->\n"
        summary = generated_notes(commits)
        if summary:
            block += summary + "\n\n"
    block += (f"Source history: {len(commits)} commits; [complete inventory](docs/releases/{version}-commits.json). "
              "Squash headings describe intermediate work and may include later revisions.\n"
              f"<!-- release-boundary: {version} {end} -->\n" + marker_end + "\n")
    if span:
        body = text[span[0]:span[1]]
        if marker_start in body:
            body = re.sub(re.escape(marker_start) + r".*?" + re.escape(marker_end) + r"\n?",
                          lambda _: block, body, flags=re.S)
        else:
            body = body.rstrip() + "\n\n" + block + "\n"
        return text[:span[0]] + body + text[span[1]:]
    unreleased = re.search(r"(?ms)^## Unreleased\s*\n(.*?)(?=^## |\Z)", text)
    manual = unreleased[1].strip() if unreleased and promote else ""
    if manual == "No changes recorded yet.":
        manual = ""
    if unreleased and promote:
        text = text[:unreleased.start()] + "## Unreleased\n\nNo changes recorded yet.\n\n" + text[unreleased.end():]
    release = (f"## [{version}] - {date}\n\nSource changes in preparation; validation and distribution have separate receipts.\n\n"
               + (manual + "\n\n" if manual else "") + block + "\n")
    insertion = re.search(r"(?m)^## (?!Unreleased)", text)
    position = insertion.start() if insertion else len(text)
    return text[:position] + release + text[position:]


def run(args):
    root = Path(args.root).resolve()
    git_dir = Path(git(root, "rev-parse", "--absolute-git-dir"))
    with (git_dir / "codescribe-version.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        journal = git_dir / "codescribe-version-transaction.json"
        recover(journal)
        manifest = root / args.version_file
        cargo = manifest.read_text()
        current = cargo_version(cargo)
        changelog_path = root / "CHANGELOG.md"
        changelog = changelog_path.read_text()
        expected = {manifest: cargo.encode(), changelog_path: changelog.encode()}
        if args.command == "bump":
            parts = list(version_tuple(current))
            index = {"major": 0, "minor": 1, "patch": 2}[args.kind]
            parts[index] += 1
            parts[index + 1:] = [0] * (2 - index)
            target = args.to_version or ".".join(map(str, parts))
            # A not-yet-tagged generated release is still being prepared. Repeated
            # bumps refresh that note rather than silently skipping another version.
            tags = git(root, "tag", "--merged", "HEAD", "--list", "v" + current).splitlines()
            if not args.to_version and not tags and f"<!-- release-notes:{current}:start -->" in changelog:
                target = current
            if version_tuple(target) < version_tuple(current):
                raise ValueError("Version rollback is not a bump")
        else:
            target = args.version
        version_tuple(target)
        end = git(root, "rev-parse", args.until + "^{commit}")
        start = previous_boundary(root, changelog, target, args.since)
        if subprocess.run(["git", "-C", str(root), "merge-base", "--is-ancestor", start, end],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            raise ValueError("Release start is not an ancestor of release end")
        commits = history(root, start, end)
        existing = section(changelog, target)
        date = args.date or (re.search(r"^## \[[^]]+\] - (\d{4}-\d{2}-\d{2})", changelog[existing[0]:])[1]
                             if existing and re.search(r"^## \[[^]]+\] - (\d{4}-\d{2}-\d{2})", changelog[existing[0]:])
                             else datetime.date.today().isoformat())
        receipt = {"version": target, "from_sha": start, "to_sha": end,
                   "scope": "Committed source history; not runtime or distribution acceptance.", "commits": commits}
        inventory_path = root / f"docs/releases/{target}-commits.json"
        expected[inventory_path] = read_optional(inventory_path)
        changes = {
            inventory_path: (json.dumps(receipt, ensure_ascii=False, indent=2) + "\n").encode(),
            changelog_path: update_changelog(changelog, target, date, end, commits,
                promote=args.command == "bump" and not existing,
                summarize=not args.inventory_only).encode(),
        }
        if args.command == "bump" and target != current:
            changes[manifest] = replace_cargo_version(cargo, current, target).encode()
            readme_path = root / "README.md"
            if readme_path.exists():
                readme = readme_path.read_text()
                expected[readme_path] = readme.encode()
                readme = readme.replace(f"badge/version-{current}-", f"badge/version-{target}-")
                readme = readme.replace(f"current source version is `{current}`", f"current source version is `{target}`")
                changes[readme_path] = readme.encode()
        if args.command == "bump":
            lock_path = root / "Cargo.lock"
            locked = lock_path.read_text()
            expected[lock_path] = locked.encode()
            changes[lock_path] = replace_lock_versions(root, cargo, locked, current, target).encode()
        transact(changes, journal, expected)
        print(f"Prepared {target}: version + changelog + source inventory; no publication performed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".")
    parser.add_argument("--version-file", default="Cargo.toml")
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("bump", "notes"):
        child = sub.add_parser(command)
        child.add_argument("--from", dest="since")
        child.add_argument("--until", default="HEAD")
        child.add_argument("--date")
        child.add_argument("--inventory-only", action="store_true", help="Keep human-authored summary; add history receipt only")
        if command == "bump":
            child.add_argument("kind", choices=("patch", "minor", "major"))
            child.add_argument("--to", dest="to_version", help="Explicit desired version; safe to repeat")
        else:
            child.add_argument("--version", required=True)
    try:
        run(parser.parse_args())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Version preparation refused: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
