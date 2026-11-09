# x9bus User Guide

x9bus is a native macOS app for dictation and conversations with agents.
Use it to turn speech into text, work with the built-in Agent, or talk to an
existing agent session through a named channel.

A fresh installation uses **Apple only** recognition. **Local power** uses a
local Whisper model; **Cloud** uses an online transcription service with your
consent. AI formatting and spoken replies are separate choices. See
[Privacy & Security](privacy.md) before enabling an online service.

## Start here

1. [Install and launch x9bus](installation.md).
2. [Make your first recording](QUICKSTART.md).
3. [Choose dictation, formatting or an agent conversation](modes.md).
4. [Navigate channels, use Sync and attach files](chat-overlay.md).

## Reference

| Guide                                 | What you can do                                                               |
| ------------------------------------- | ----------------------------------------------------------------------------- |
| [Installation](installation.md)       | Install the app, grant relevant permissions and check its version             |
| [Quick Start](QUICKSTART.md)          | Record, review and deliver a first transcript                                 |
| [Recording Modes](modes.md)           | Choose the recognition mode, formatting and destination independently         |
| [Chat & Overlay](chat-overlay.md)     | Use the overlay, agent roster, floating cards, composers and personal inbox   |
| [Settings](settings.md)               | Find providers, hotkeys, microphones, models, tool permissions and local data |
| [Troubleshooting](troubleshooting.md) | Resolve readiness, delivery and connection problems without losing a take     |
| [Privacy & Security](privacy.md)      | Understand local storage, outgoing data and audio retention                   |

## Which conversation am I using?

**The built-in Agent** has its own conversation window, configured model,
workspace and tools. Open it with **Show Agent**.

**Connected agents** are conversations running in another client, such as Codex
or Claude Code. x9bus connects them through channels **1–9** and delivers
speech and written messages to their existing sessions. Installing a plugin
alone does not connect a session.

**0 · All** is a shared channel with its own audience rules. **Your inbox (10)**
collects messages deliberately addressed to you. Neither is another agent.

## Version and availability

This guide follows the development source. The app installed on your Mac may
be older. Check its version and build in **Settings → User**, then compare with
[release notes](https://github.com/vetcoders/codescribe/releases).

Nightly-only features are marked where described. In particular, attachment
**Markup** is available in the nightly build, not every distribution. Changes
to the sidebar and recipient menu may also arrive before the next public release.

For source builds and launch/signing details, see the
[developer installation guide](../INSTALLATION.md). For implementation contracts,
start with the [repository README](../../README.md).
