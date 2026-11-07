# Settings

Open the native Settings window from Codescribe's menu. Search matches section
names and the tabs inside them, so a term such as “microphone”, “Whisper” or
“MCP” can reveal the relevant page.

The sidebar contains **Creator, Hotkeys, Providers, Agent, Dictation, Audio,
Dictionary** and **User**. **Lab** is a developer surface and is hidden unless
developer mode is enabled. Interface translations can use different labels.

## Where configuration lives

Normal preferences are saved in `settings.json` in Codescribe's Application
Support directory. Provider credentials are stored in macOS Keychain. Advanced
configuration can also use `~/.codescribe/.env`; it is not required for normal
GUI setup, and retired environment names are not interchangeable with current
settings.

Native presentation preferences, including interface language and appearance,
use macOS preferences rather than every value living in settings.json. The
first-dictation confirmation preference also uses UserDefaults.

A recording uses its configuration snapshot. A saved change can apply to the
next take rather than altering the take already in progress. A value shown in a
picker, a successful save and a working service are different observations.

Most preferences apply as you change them. **Hotkeys**, **Creator → Keywords** and **Agent → Workspace**
have explicit save behavior; use the visible Save action. Connection tests,
model downloads and destructive resets are actions, not autosaved preferences.
Never post configuration files or credentials without reviewing their contents.

## Creator

### Language and appearance

Choose the interface language, recognition language and System, Light or Dark
appearance. Recognition language and interface language are separate choices. Follow a
restart prompt for changes that require reopening. Appearance affects the UI,
not the language recognized from speech or the configured AI provider.

macOS accessibility choices also matter: Reduce Transparency and Increase
Contrast can replace glass with opaque surfaces, and Reduce Motion limits
animation.

### Setup and permissions

The setup checklist shows Microphone, Speech Recognition, Accessibility, Input
Monitoring and Screen Recording. Use its links to open the appropriate macOS
permission pane. The setup wizard can be reopened here.

Setup completion and recording readiness are separate. Apple's path needs
Speech Recognition; Local power needs a valid local model. Full Disk Access
is optional, not a blanket prerequisite for ordinary dictation. See
[Installation](installation.md#permissions).

### Overlay and intercom

Choose the available overlay/intercom layout and channel-map visibility.
Presentation choices change how you reach the same channels; they do not
create new agent sessions or start a recording. Experimental layouts can differ
between builds. See [Chat & Overlay](chat-overlay.md).

### Formatting level

Choose **Off / Correction / Smart** for automatic formatting. Off skips the
AI formatting pass; Correction is the default. **Max** is an explicit per-take
consultation from the overlay Format menu, using the Agent model and prompt.
It is not offered as an automatic level. The other formatting levels use the
formatting lane under **Agent → AI models**; credentials live under **Providers**.

**Keywords** supplies familiar names and terms as context to Correction, Smart
and Max. Edit the list and use **Save**; a refused save retains the unsaved
draft. Keywords are not always-replace Dictionary rules and do not bias speech
recognition.

Formatting acts on recognized text. It does not establish what was spoken and
must not be used to hide missing words in recognition.

### Agent integrations

Managed integration controls install or update the helpers and selected client
skills. Read any ownership or replacement confirmation. Installing integration
files is different from connecting the conversation you are currently using.
A named session must still attach to its channel.

## Hotkeys

Configure Dictation, Formatting and Assistive bindings. Dictation uses
**hold Fn/Globe** by default. Formatting and Assistive do not have a shortcut
until you opt in. Where offered, double-tap Ctrl, left Option or right Option
are explicit choices, not universal factory defaults.

Review gesture conflicts and use **Save** after editing a binding. A disabled
binding should be treated as disabled, not as a failed microphone.

Other controls cover overlay visibility, pasting and selected-text capture.
**⌘⇧Space** opens the built-in Agent; **Ctrl+Shift+B** toggles the overlay.
Use the bindings actually shown in your build when customizing them.

Fn-tap toggling and middle-mouse gestures are optional and initially off.
The channel modifier must not also be a key that records dictation; unavailable
choices are disabled. Double-tap timing and typing-quiet controls affect gesture
recognition, not audio recognition.

### Paste policy

**Safe / Comfort / Off** control delivery to another application. Choose the
behavior deliberately; recording a take does not promise unconditional paste.
Agent-directed requests have their own explicit destination. Changing the
conversation you view does not redirect an already running take.

## Providers

Configure an account or API key for each provider you intend to use. Keys are
stored in Keychain; a saved key is not proof that authentication succeeded.
Use the provider's connection test and inspect its result.

Model discovery and inference can have different requirements. In particular,
an account that can submit a request may not expose the same model-list API as
an API key. A previously successful test is evidence of that test, not a
permanent guarantee of service availability.

Custom services use the endpoint and transport you configure. Their data policy
and retention are not set by Codescribe. Workspace path validation does not
sandbox a provider process or an external service.

## Agent

The Agent page has **AI models, Prompts, Workspace, Diagnostics, Tools** and
**MCP** tabs. These configure the built-in Agent and related functions; an
external attached agent also has its own client's configuration.

### AI models

Choose the provider and model separately for Agent and formatting functions.
Accounts and keys remain in **Providers**. For Correction and Smart, the
formatting-engine row can select the configured provider or experimental
**Apple on this Mac**. The latter tries Apple first and can fall back to the
configured provider; selecting it is not a promise that no text leaves the Mac. Do not infer that changing the Agent
model also changes speech recognition; the latter lives in **Dictation**.

The channel-voice controls on this page govern connected-agent sessions:

- **Keep listening** or **End session** determines what happens after an
  utterance. Keep listening is the default.
- **Speech pause** determines when silence can finish a message.
- **Close without voice** controls a session that receives no speech.
- **Finish each message** and its wait control determine whether to wait for
  punctuation/formatting before submission.
- A **publication hold** can delay delivery after a completed utterance.

These settings are not several microphone owners. They affect the existing
recording session. Do not confuse an utterance being submitted with the whole
session ending, or with the built-in Agent's silent-take timeout.

### Prompts

Edit the prompts offered for correction, formatting and Agent work. The app
adds runtime instructions where required; an editable prompt is not necessarily
the entire request sent to a provider. Keep secrets out of prompts.

Prompt customization changes the requested transformation. It cannot recover
acoustic evidence that was never recognized or make an unavailable tool work.

### Workspace

Select the folders available to Codescribe's own file/terminal tools and the
MCP calls whose paths it can validate. Use **Save** to commit the folder list.

This list is **not an operating-system sandbox**. Child processes and MCP
servers have their own permissions; not every third-party tool exposes a path
Codescribe can validate. Choose tool permissions as well as workspace roots.

### Diagnostics

Inspect configuration and connection results for the Agent environment.
Optional integrations are not prerequisites for ordinary dictation. A detected
configuration, an installed executable and a live connection are different
states; use an explicit test where offered and read its result.

### Tools

Choose **Allow / Ask / Deny** for individual tools and broader defaults.
Specific tool decisions take precedence over the corresponding broader rule.
Review destructive, clipboard, file and process actions according to the access
you intend to grant. A tool appearing in the catalogue does not mean its use has
already been approved or that execution succeeded.

### MCP

Configure servers and inspect the tools they expose. Discovery or a connection
test can start a server process. Enabled configuration is not proof of a live,
healthy server. Review server commands, environment and permissions before
connecting; an MCP server is not sandboxed by the workspace folder list alone.

## Dictation

### Engine

Choose **Apple only / Local power / Cloud**. A fresh installation uses Apple
only. Changes apply to the next recording.

- Apple only uses Apple's on-device recognition path without requiring Whisper.
- Local power uses a local Whisper model that must be available first.
- Cloud uses an online transcription service and requires audio-egress consent.

The selected mode, the last recognizer that served a result and the current
readiness state answer different questions. Inspect each rather than assuming
that saving a mode has made it ready.

### Whisper

Choose and manage the model used by Local power. Download, validate and remove
models with the provided controls. **Not installed, installed, valid** and
**loaded into memory** are different states. Disk size is not memory usage.

Apple only does not require a Whisper model. Do not download a model merely to
clear an unrelated permission or delivery problem, or delete one during use.

### Preview

Choose **Smooth, Snappy, Relaxed, No Preview** or **Custom** pacing where offered.
These presets coordinate transcript display and timing; some timing choices
also affect audio chunking. They are not proof that recognition quality is
unchanged.

No Preview suppresses the ordinary preview; it does not erase an edited draft
or necessarily close a view kept open by another active state. Developer timing
experiments belong in Lab rather than a normal first-run setup.

### Privacy

Inspect when recognition or related services use the network. Cloud speech
recognition sends captured audio; formatting sends transcript text; remote
speech synthesis sends reply text. Those are separate outgoing boundaries.
Choosing local recognition does not disable independently configured AI or
update services. See [Privacy & Security](privacy.md).

## Audio

### Input device and readiness

Choose System Default or a named microphone. If the selected device is absent,
inspect the effective input shown by the app instead of assuming that a saved
name is the input currently recording.

Permissions, available input and recognition requirements contribute to
readiness. Calibration is not a blanket requirement for ordinary dictation;
additional calibration requirements can apply when developer seal-ledger
admission is enabled.

### Sound feedback

Enable or disable the recording-start sound and adjust its volume. The volume
control is disabled while the start sound is off; it does not control remote
agent speech volume or change the microphone input level.

### Audio retention

Choose **Forever / 30 days / 7 days / 24h / Off**. Forever is the preserving
default. Finite policies expire eligible completed audio while protecting active
captures, readers and retry evidence. Off applies to new takes and does not
purge existing audio. Deleting audio does not delete transcript text.

Read the [retention contract](../TAKE_AUDIO_RETENTION.md) for storage and
protection details. Conversation history is a separate control.

## Dictionary

Recent takes, corrections and learned vocabulary are related but distinct
records. Play the paired original audio when available before deciding whether
the problem is recognition or formatting. Retranscription requires the
corresponding audio; a transcript by itself is not that evidence.

Corrections can inform vocabulary/rules according to the offered action. Counts
of takes, corrections and learned entries do not mean the same thing. Inspect
what will be saved or removed rather than treating every correction as an
immediate global rewrite.

## Lab

Lab is a developer-only surface. Experiments, timing controls and evidence
views can change the recognition path or its diagnostics. They are not
prerequisites for using Codescribe and should not be enabled simply to complete
setup. Preserve a known configuration when comparing recordings.

### Glass tuning

Developer builds expose **Lab → Glass tuning** for the experimental
**Onboarding**, **Mini and MIDI**, and **Bus header** surfaces. Select a surface,
then adjust **Glass tint**. Already-open views update immediately; each surface
retains its own macOS preference. **Reset glass** restores the selected surface
to the unmodified system material, with zero additional tint.

This control changes the native glass tint, not blur radius, refractive strength
or an opaque layer over the window. Reading canvases and the accepted Agent and
Settings chrome keep their own material policy. Reduce Transparency and Increase
Contrast still take precedence.

The controls require a developer bundle with `CSDeveloperSurface` enabled and do
not require enabling the Voice Lab recording experiments. Production bundles
ignore saved developer tint values. The UI-only preferences are
`codescribe.dev_glass_tint.onboarding.v1`,
`codescribe.dev_glass_tint.compact.v1`, and
`codescribe.dev_glass_tint.bus_header.v1`; they do not alter `settings.json` or a
recording snapshot. Values from the earlier backing-opacity experiment are not
reused as native tint values.

## User

This page shows the installed app's version/build, licence and local data
controls. Record the version after reopening an update; a newer bundle on disk
and an older running process can coexist until restart.

Basic dictation and licensed Agent functionality have different requirements.
Use the licence controls and their validation result rather than publishing a
licence key in a support request.

The optional **First dictation confirmation** preference is off by default.
The current production analytics domain is empty, so this confirmation remains
disabled even if the local preference is turned on. If enabled in a configured
build, its event contains app/macOS version information, not speech or text.

Reset actions have different scopes. Read the confirmation, preserve wanted
takes and drafts, and distinguish moving local data to Trash from permanently
removing Keychain credentials. Audio retention and transcript history are
configured separately; do not use a whole-app reset for a single failed send.

## Troubleshooting

For a readiness, provider, delivery or UI failure, start with
[Troubleshooting](troubleshooting.md). State which setting was changed, whether
it was saved, whether a new take/session was started, and what the runtime
actually did. A visible control and a persisted value alone do not prove the
behavior was exercised.
