# Privacy & Security

Codescribe supports local speech recognition and optional configured services.
This document describes the data boundaries implemented in the app and website;
see the [public privacy page](https://codescribe.vetcoders.io/privacy/).

## Local speech and storage

Once local models and macOS permissions are ready, recording, local speech
recognition, hotkeys, and text delivery can run without a cloud AI service.
Downloading a speech model, checking for updates, and issuing a website licence
are separate network operations.

| Data        | Local storage                                                           | Optional outgoing use                                              |
| ----------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Audio       | Takes, session WAVs, daily archive                                      | Cloud transcription when enabled                                   |
| Transcripts | `~/.codescribe/transcriptions/` and conversation history                | Formatting, agent requests, or speech synthesis when used          |
| Settings    | `settings.json` and optional `.env`                                     | Configured services use the corresponding endpoint and credentials |
| API keys    | macOS Keychain; explicit environment/config imports may contain secrets | Provider authentication                                            |
| Prompts     | `~/.codescribe/prompts/`                                                | Included in configured AI requests                                 |

## Configured services

- **AI formatting:** transcript text and formatting instructions go to the
  configured model endpoint.
- **Agents:** a request can include conversation history, selected text, attached
  files, screenshots, and tool results relevant to the task. An external runtime
  has its own provider configuration and data policy.
- **Cloud transcription:** captured audio goes to the configured STT service
  when that path is enabled.
- **Spoken replies:** a remote speech provider receives the reply text to generate
  audio. A locally configured speech path has a different boundary.

Built-in remote providers use HTTPS. A custom endpoint uses the transport the
user configures. Data retention depends on the selected service and account
contract; Codescribe does not promise one retention period for all providers.
Choosing a local AI endpoint keeps that inference on the selected local service,
but does not disable update checks or other independently configured services.

## System permissions

| Permission         | Purpose                                               |
| ------------------ | ----------------------------------------------------- |
| Microphone         | Capture speech during a recording session             |
| Speech Recognition | Apple's speech recognition path                       |
| Accessibility      | Read selected text and deliver text to supported apps |
| Input Monitoring   | Detect configured shortcuts and modifier state        |
| Screen Recording   | Screenshots when used as context                      |

Agent file access follows the configured workspace and tool permissions. Do not
describe the product as unable to read outside `~/.codescribe/`: explicitly
attached files and agent tools can address other locations.

## Audio retention

Complete captured audio is saved locally by default, including takes whose
recognition, formatting, seal, or delivery failed. Settings > Audio > Audio
retention offers **Forever / 30 days / 7 days / 24h / Off**. Missing or unknown
settings preserve audio (Forever).

Full WAVs live under `~/.codescribe/takes/`, session WAVs under
`~/.codescribe/sessions/`, and daily audio under
`~/.codescribe/transcriptions/YYYY-MM-DD/`. Finite retention expires eligible
completed owned audio across these locations. Active captures, processing/read
leases, and protected retry evidence are preserved. A take keeps the choice it
started with; Off applies to new takes and does not purge existing recordings.
Unknown completion times and failed deletions remain visible as errors. Audio
expiration does not delete transcript text.

See [Take audio retention](../TAKE_AUDIO_RETENTION.md) for the storage contract.
Transcript history is controlled separately; `HISTORY_ENABLED=0` disables that
history path, rather than erasing every app or external-agent record.

## Updates, analytics, and website services

The desktop app does not ship a third-party transcript analytics SDK. Sparkle
update checks contact the configured feed according to the updater's settings.
Model downloads contact their download hosts. Website analytics are disabled in
the default build and can be explicitly enabled for a deployment.

The website licence form sends the entered email over HTTPS to the issuer.
The signed licence contains its SHA-256 hash; the issuance log records that hash,
client IP, and timestamp. This is pseudonymous data, not an anonymity guarantee.
The app verifies the signed licence locally. No activation call is needed for
that verification.

For licence-service data questions, contact
[hello@vetcoders.io](mailto:hello@vetcoders.io). Remote agent and AI service records
remain subject to those services' policies; deleting local app files does not
erase their records.

## Source availability

Codescribe is source-available under FSL-1.1-ALv2. The
[repository](https://github.com/vetcoders/codescribe) contains the implementation;
source availability does not by itself certify an installed artifact or a
provider's behavior.
