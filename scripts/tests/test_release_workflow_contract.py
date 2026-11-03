"""Declarative contract of the `Release DMG` workflow.

Parses the real YAML and judges the shape that keeps a release honest: all
lanes build on the hosted Xcode 27 image, the smoke lane can never sign,
notarize or publish, the release lane validates its inputs before it imports a
certificate, publishes a draft and never overwrites release assets, and a
rehearsal runs the whole signing path without publishing anything. It proves
nothing about hosted execution; that is observed only on GitHub, cheapest
through a smoke dispatch.
"""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = Path(os.environ.get("WORKFLOWS_DIR", ROOT / ".github/workflows"))

# env name the build scripts read -> organization-wide secret name (shared with Vista)
RELEASE_SECRETS = {
    "CODESIGN_CERTIFICATE_BASE64": "MACOS_CERTIFICATE_BASE64",
    "CODESIGN_CERTIFICATE_PASSWORD": "MACOS_CERTIFICATE_PASSWORD",
    "CODESCRIBE_CODESIGN_IDENTITY": "MACOS_SIGNING_IDENTITY",
    "APPLE_ID": "NOTARY_APPLE_ID",
    "APPLE_TEAM_ID": "NOTARY_TEAM_ID",
    "APPLE_APP_SPECIFIC_PASSWORD": "NOTARY_PASSWORD",
    "SPARKLE_ED_PRIVATE_KEY": "SPARKLE_ED_PRIVATE_KEY",
}
RELEASE_VARIABLES = ("CODESCRIBE_LICENSE_PUBLIC_KEY_HEX", "SPARKLE_ED_PUBLIC_KEY")
NOT_REHEARSING = "env.REHEARSE != 'true'"


def load():
    document = yaml.safe_load((WORKFLOWS / "release.yml").read_text())
    # PyYAML (YAML 1.1) reads the bare key `on` as boolean True.
    document["on"] = document.pop(True, document.get("on"))
    return document


def runs(job):
    return [step.get("run", "") for step in job["steps"]]


def uses(job):
    return [step.get("uses", "") for step in job["steps"]]


def names(job):
    return [step.get("name", "") for step in job["steps"]]


class ReleaseWorkflowContractTests(unittest.TestCase):
    def setUp(self):
        self.workflow = load()
        self.jobs = self.workflow["jobs"]
        self.smoke = self.jobs["smoke"]
        self.release = self.jobs["release"]

    def step(self, name):
        return self.release["steps"][names(self.release).index(name)]

    def test_triggers_are_tags_and_manual_dispatch_only(self):
        trigger = self.workflow["on"]
        self.assertEqual(set(trigger), {"push", "workflow_dispatch"})
        self.assertEqual(trigger["push"], {"tags": ["v*"]})
        inputs = trigger["workflow_dispatch"]["inputs"]
        self.assertFalse(inputs["tag_name"]["required"])
        self.assertEqual(inputs["variant"]["type"], "choice")
        self.assertEqual(inputs["variant"]["options"], ["slim", "full"])
        self.assertEqual(inputs["variant"]["default"], "slim")
        for flag in ("smoke", "rehearse"):
            with self.subTest(flag=flag):
                self.assertEqual(inputs[flag]["type"], "boolean")
                self.assertIs(inputs[flag]["default"], False)

    def test_both_lanes_build_on_the_hosted_xcode_27_image(self):
        for job_id, job in (("smoke", self.smoke), ("release", self.release)):
            with self.subTest(job=job_id):
                self.assertEqual(job["runs-on"], "xcode-27")
                self.assertNotIn("self-hosted", str(job["runs-on"]))
                self.assertIn("xcodebuild -version", runs(job))
        self.assertEqual(self.workflow["env"]["DEVELOPER_DIR"], "/Applications/Xcode_27.2.app")

    def test_token_is_read_only_except_for_publishing(self):
        self.assertEqual(self.workflow["permissions"], {"contents": "read"})
        self.assertNotIn("permissions", self.smoke)
        self.assertEqual(self.release["permissions"], {"contents": "write"})

    def test_smoke_runs_only_on_request_and_never_signs_notarizes_or_publishes(self):
        self.assertEqual(self.smoke["if"], "github.event_name == 'workflow_dispatch' && inputs.smoke")
        self.assertNotIn("environment", self.smoke)
        text = yaml.safe_dump(self.smoke)
        for forbidden in (
            "secrets.",
            "softprops/action-gh-release",
            "gh release",
            "notarytool",
            "security import",
            "keychain-session",
            "release-standard",
            "release-full",
        ):
            self.assertNotIn(forbidden, text)
        joined = "\n".join(runs(self.smoke))
        self.assertIn("make dmg", joined)
        self.assertIn("--skip-notary --unsigned", joined)
        self.assertTrue(any(u.startswith("actions/upload-artifact@") for u in uses(self.smoke)))
        upload = next(s for s in self.smoke["steps"] if s.get("uses", "").startswith("actions/upload-artifact@"))
        self.assertEqual(upload["with"]["if-no-files-found"], "error")

    def test_release_runs_under_the_release_environment_and_skips_smoke_dispatches(self):
        self.assertEqual(self.release["environment"], {"name": "release"})
        self.assertEqual(self.release["if"], "github.event_name == 'push' || !inputs.smoke")
        self.assertEqual(self.release["env"]["RELEASE_TAG"], "${{ inputs.tag_name || github.ref_name }}")
        self.assertEqual(self.release["env"]["VARIANT"], "${{ inputs.variant || 'slim' }}")
        self.assertEqual(self.release["env"]["REHEARSE"], "${{ inputs.rehearse == true }}")

    def test_release_checks_out_the_approved_commit_with_full_history(self):
        checkout = next(step for step in self.release["steps"] if step.get("uses", "").startswith("actions/checkout@"))
        self.assertEqual(
            checkout["with"]["ref"],
            "${{ inputs.tag_name || (github.event_name == 'workflow_dispatch' && github.sha) || github.ref }}",
        )
        # CSBuildNumber is `git rev-list --count HEAD`: a shallow clone stamps 1.
        self.assertEqual(checkout["with"]["fetch-depth"], 0)

    def test_release_builds_slim_by_default_and_full_only_on_request(self):
        joined = "\n".join(runs(self.release))
        self.assertIn("make release-standard", joined)
        self.assertIn("make release-full", joined)
        self.assertNotIn("make release-dmgs", joined)
        # release-full depends on ensure-models itself; the slim build never reads those caches.
        self.assertNotIn("ensure-models", joined)
        self.assertNotIn("--unsigned", joined)

    def test_release_refuses_bad_refs_and_existing_assets_before_touching_secrets(self):
        order = names(self.release)
        guards = (
            "Refuse anything but a vX.Y.Z tag",
            "Refuse to publish over existing release assets",
            "Validate release keys and signing secrets",
            "Import Developer ID certificate",
            "Configure Apple notarization profile",
        )
        positions = [order.index(name) for name in guards]
        self.assertEqual(positions, sorted(positions))
        for step in self.release["steps"][: positions[2]]:
            self.assertNotIn("secrets.", yaml.safe_dump(step))
        validate = self.release["steps"][positions[2]]
        for env_name, secret in RELEASE_SECRETS.items():
            self.assertEqual(validate["env"][env_name], "${{ secrets.%s }}" % secret)
        for variable in RELEASE_VARIABLES:
            self.assertEqual(validate["env"][variable], "${{ vars.%s }}" % variable)

    def test_signing_secrets_use_the_organization_wide_names_everywhere(self):
        text = yaml.safe_dump(self.release)
        for env_name, secret in RELEASE_SECRETS.items():
            with self.subTest(secret=secret):
                self.assertIn("${{ secrets.%s }}" % secret, text)
                if env_name != secret:
                    self.assertNotIn("${{ secrets.%s }}" % env_name, text)

    def test_release_lookup_allows_only_missing_tag_or_empty_draft(self):
        script = self.step("Refuse to publish over existing release assets")["run"]
        cases = [
            ("missing", 404, {"message": "Not Found"}, 1, True),
            ("empty draft", 200, {"draft": True, "assets": []}, 0, True),
            ("published empty release", 200, {"draft": False, "assets": []}, 0, False),
            ("draft with assets", 200, {"draft": True, "assets": [{"name": "old.dmg"}]}, 0, False),
            ("missing draft state", 200, {"assets": []}, 0, False),
            ("missing assets state", 200, {"draft": True}, 0, False),
            ("invalid assets", 200, {"draft": True, "assets": None}, 0, False),
            ("authentication failure", 401, {"message": "Bad credentials"}, 1, False),
            ("permission failure", 403, {"message": "Not Found"}, 1, False),
            ("server failure", 503, {"message": "Unavailable"}, 1, False),
            ("transport failure", None, None, 1, False),
            ("invalid metadata", 200, "not JSON", 0, False),
            ("inconsistent command status", 404, {"message": "Not Found"}, 0, False),
        ]
        scratch = ROOT / "build/test-scratch"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="release-lookup-", dir=scratch) as temporary:
            binary = Path(temporary) / "gh"
            binary.write_text(
                '#!/bin/sh\n'
                '[ "$1" = api ] && [ "$2" = --include ] || exit 97\n'
                '[ "$3" = "repos/$GH_REPO/releases/tags/$RELEASE_TAG" ] || exit 97\n'
                'printf "%s" "$LOOKUP_RESPONSE"\n'
                'exit "$LOOKUP_EXIT"\n'
            )
            binary.chmod(0o700)
            for label, status, metadata, command_exit, allowed in cases:
                with self.subTest(case=label):
                    body = metadata if isinstance(metadata, str) else json.dumps(metadata)
                    response = "" if status is None else f"HTTP/2.0 {status} fixture\r\n\r\n{body}\n"
                    result = subprocess.run(
                        ["bash", "-e", "-c", script], capture_output=True, text=True, timeout=10,
                        env={"PATH": temporary + os.pathsep + os.environ["PATH"],
                             "GH_REPO": "vetcoders/codescribe", "RELEASE_TAG": "v0.18.0",
                             "LOOKUP_RESPONSE": response, "LOOKUP_EXIT": str(command_exit)},
                    )
                    self.assertEqual(result.returncode == 0, allowed, result.stdout + result.stderr)

    def test_rehearsal_stands_down_the_tag_guards_but_keeps_the_signing_path(self):
        for name in ("Refuse anything but a vX.Y.Z tag", "Refuse to publish over existing release assets"):
            with self.subTest(step=name):
                self.assertEqual(self.step(name)["if"], NOT_REHEARSING)
        for name in (
            "Validate release keys and signing secrets",
            "Import Developer ID certificate",
            "Configure Apple notarization profile",
            "Build, sign, notarize and gate the DMG",
            "Verify DMG payloads against the tag",
        ):
            with self.subTest(step=name):
                self.assertNotIn("if", self.step(name))
        verify = self.step("Verify DMG payloads against the tag")["run"]
        self.assertIn('if [[ "$REHEARSE" == true ]]', verify)
        self.assertIn("make -s version", verify)

    def test_rehearsal_publishes_nothing_and_keeps_the_dmg_as_a_run_artifact(self):
        keep = self.step("Keep the rehearsal DMG as a run artifact")
        self.assertEqual(keep["if"], "env.REHEARSE == 'true'")
        self.assertTrue(keep["uses"].startswith("actions/upload-artifact@"))
        self.assertEqual(keep["with"]["if-no-files-found"], "error")
        for asset in ("Codescribe_*.dmg", "Codescribe_*.dmg.sha256", "appcast.xml"):
            self.assertIn(asset, keep["with"]["path"])
        order = names(self.release)
        self.assertLess(order.index("Publish draft release with the full DMG and checksum"), order.index(keep["name"]))

    def test_publish_is_a_draft_that_never_overwrites_assets(self):
        publishes = [
            step
            for step in self.release["steps"]
            if step.get("uses", "").startswith("softprops/action-gh-release@")
        ]
        self.assertEqual(
            sorted(step["if"] for step in publishes),
            ["env.VARIANT == 'full' && " + NOT_REHEARSING, "env.VARIANT == 'slim' && " + NOT_REHEARSING],
        )
        for step in publishes:
            with self.subTest(step=step["if"]):
                options = step["with"]
                self.assertIs(options["draft"], True)
                self.assertIs(options["overwrite_files"], False)
                self.assertIs(options["fail_on_unmatched_files"], True)
                self.assertEqual(options["tag_name"], "${{ env.RELEASE_TAG }}")
        slim = next(step for step in publishes if step["if"].startswith("env.VARIANT == 'slim'"))
        for asset in ("Codescribe.dmg", "Codescribe.dmg.sha256", "appcast.xml"):
            self.assertIn(asset, slim["with"]["files"])

    def test_appcast_and_stable_alias_exist_only_for_the_slim_feed(self):
        for name in ("Alias slim DMG as stable Codescribe.dmg", "Generate signed Sparkle appcast"):
            self.assertEqual(self.step(name)["if"], "env.VARIANT == 'slim'")

    def test_signing_keychain_is_released_on_every_outcome(self):
        cleanup = [step for step in self.release["steps"] if "keychain-session.sh end" in step.get("run", "")]
        self.assertEqual(len(cleanup), 1)
        self.assertEqual(cleanup[0]["if"], "always()")
        self.assertIs(self.release["steps"][-1], cleanup[0])

    def test_every_action_is_pinned_to_a_commit(self):
        for job_id, job in self.jobs.items():
            for action in filter(None, uses(job)):
                with self.subTest(job=job_id, action=action):
                    self.assertRegex(action, r"@[0-9a-f]{40}$")


if __name__ == "__main__":
    unittest.main()
