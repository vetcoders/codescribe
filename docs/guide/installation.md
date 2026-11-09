# Installation

## Install a packaged app

1. Download the appropriate package from the
   [website](https://codescribe.vetcoders.io/) or
   [GitHub Releases](https://github.com/vetcoders/codescribe/releases).
2. Open the DMG and drag **Codescribe.app** to **Applications**.
3. Launch Codescribe from Finder or Spotlight. Complete the setup checklist in
   **Settings → Creator**.
4. Check **Settings → User** for the installed version and build.

Use a package intended for your Mac and macOS version. Release notes and the
package's requirements describe that build; the version in the source repository
is not proof that a corresponding installer has been published.

A packaged app does not require Rust, Cargo or Xcode. For development, use the
[separate source installation guide](../INSTALLATION.md).

## Permissions

Codescribe shows its permission checklist in **Settings → Creator**. macOS
controls the grants in **System Settings → Privacy & Security**.

| Permission         | What it enables                                             |
| ------------------ | ----------------------------------------------------------- |
| Microphone         | Recording your speech                                       |
| Speech Recognition | Apple's speech recognition path                             |
| Accessibility      | Reading selected text and delivering text to supported apps |
| Input Monitoring   | Configured global shortcuts and modifier keys               |
| Screen Recording   | Capturing screenshots as agent context                      |

The setup completion checklist can include Screen Recording even when your
first task is ordinary dictation. That checklist is distinct from the
requirements of the selected recording mode. Full Disk Access is optional;
it is not a universal prerequisite for dictation.

Each row states what macOS reports for the running copy of Codescribe:

- **Granted** — Codescribe has the access.
- **Denied** — macOS recorded a refusal (Microphone, Speech Recognition, Input
  Monitoring, Spotify control). For Microphone and Speech Recognition macOS does
  not ask again; turn Codescribe on in System Settings.
- **No access** — this copy of Codescribe has no effective access now
  (Accessibility, Screen Recording, Full Disk Access). These checks cannot tell
  a refusal from a grant that is missing, belongs to another copy of the app, or
  is not applied yet, so the row does not claim you refused.
- **Not determined** — macOS has not decided yet.

After changing a permission in System Settings, return to Codescribe: setup
re-reads every status when its window becomes active, and **Refresh status**
reads them again. If a permission you turned on still shows **No access**,
macOS may apply it only after you quit and reopen Codescribe; the row says so
in a grey hint with an info sign, because it is not an error and a reopen is not
guaranteed to help. Only a refusal macOS will not ask about again shows a red
note with a warning sign. Optional permissions say what stays unavailable and
never block setup.
Finish active recordings and preserve unsent drafts before quitting. Avoid
repeatedly resetting all permissions: development and distribution builds can
have different signing identities and grants.

## Recognition and downloads

Select the path in **Settings → Dictation → Engine**:

- **Apple only:** the fresh-install default; it does not require a Whisper model.
- **Local power:** download a local model in **Dictation → Whisper**. Use the
  availability and validation controls before recording with it.
- **Cloud:** configure the service and explicitly allow the captured audio to
  leave your Mac.

A model being installed, valid and loaded into memory are different states.
Download size and memory use depend on the chosen model. Do not remove model
files while a take is using them.

AI formatting, the built-in Agent and spoken replies have separate provider
settings. Basic local recognition does not require an AI-provider key.

## Configuration

Use Settings for normal setup. Preferences are stored in `settings.json` under
Codescribe's Application Support directory; provider credentials are stored in
macOS Keychain. An optional `~/.codescribe/.env` is available for advanced
configuration. You do not need to create it for a normal GUI installation.

See [Settings](settings.md) and [Privacy & Security](privacy.md) for providers,
local history, prompts and recordings. Do not post configuration files or keys
when asking for support.

## Updates and nightly builds

Use the app's update controls or a package from the release page. Read which
channel you are installing: a nightly build can contain features absent from a
public release. Attachment Markup is one such feature.

Finish the current take and any in-flight agent request before replacing or
reopening the app. An installed update and a running process can be different
builds until the app restarts. Check the version again after reopening it.

For source installations, the guarded install and idle-restart commands are
specified in the [developer guide](../INSTALLATION.md). Do not bypass an active
recording to make an update complete.

## Remove the app or reset data

Quit Codescribe when idle, then move its app bundle to Trash in Finder.
Removing the app does not by itself erase your transcripts, recordings or
provider credentials.

For a selective reset, use **Settings → User** and read the scope of the chosen
action. Preserve recordings and conversation history you want to keep. Audio
retention is configured separately in **Settings → Audio**; choosing Off affects
new takes and does not purge existing audio.

If the app does not start or permissions appear stale, use
[Troubleshooting](troubleshooting.md) before deleting local data.
