---
run_id: impl-123451-61361
prompt_id: 20260502_chat_shell_forensics_claude_20260502
agent: claude
skill: impl
model: unknown
status: completed
---

# Agent Chat Shell Forensics

## Current State

CodeScribe's agent chat does not diverge because of STT/provider/runtime logic.
It diverges because the chat UI still owns an experimental Rust/AppKit overlay
stack while Settings and parts of the shell contract have moved toward a
first-class shared AppKit window policy.

The duplicated shell/window policies are:

- Agent chat policy in `app/ui/shared/helpers.rs:1740`: titled, closable,
  miniaturizable, full-size-content, resizable, `CanJoinAllSpaces |
  FullScreenAuxiliary`, floating level, transparent titlebar, non-opaque,
  movable by background, min 380x360, max clamped to visible frame.
- Settings policy in `app/ui/shared/helpers.rs:1765`: titled, closable,
  miniaturizable, full-size-content, fixed size, `FullScreenNone`, normal level,
  transparent titlebar, opaque, not movable by background.
- Onboarding policy in `app/ui/onboarding/mod.rs:576`: hand-built `NSWindow`
  with titled/full-size-content style, hidden transparent titlebar, non-opaque
  glass background, fixed 720x540, `FullScreenNone`, `window_show` rather than
  `present_shared_shell_panel`.
- Voice chat still adds a custom window class in `app/ui/voice_chat/handlers.rs:342`
  to force key/main behavior and custom key equivalents, then constructs the
  window in `app/ui/voice_chat/mod.rs:209` with `agent_chat_shell_panel_policy`.

This means the policy matrix is only partially shared. Chat reuses the policy
function and presentation helper, but it keeps separate window-class behavior,
state ownership, resize handling, markdown rendering, scroll behavior, and
stream update mechanics.

## Behavioral Divergence

The load-bearing split is `app/ui/voice_chat/state.rs:280`, where chat keeps a
global `OVERLAY_STATE: Mutex<VoiceChatOverlayState>`. That state stores both
domain/UI data and raw retained AppKit pointers. Public API calls in
`app/ui/voice_chat/api.rs:109`, `app/ui/voice_chat/api.rs:132`, and
`app/ui/voice_chat/api.rs:171` hop to the main queue and then take that mutex.

The most important hot path is streaming layout:

- `app/ui/voice_chat/api.rs:1070` throttles streaming layout to 50 ms.
- `app/ui/voice_chat/api.rs:1086` applies deltas and tries an in-place update.
- `app/ui/voice_chat/api.rs:1101` falls back to full chat rebuild.
- `app/ui/voice_chat/api.rs:1111` spawns a deferred thread, then re-enters the
  main queue and locks `OVERLAY_STATE` again at `app/ui/voice_chat/api.rs:1114`.
- `app/ui/voice_chat/api.rs:1808` clears and rebuilds the entire message stack,
  recreates bubble views, syncs document view size, and scrolls.

That explains the visible regression: a long answer with table-ish Markdown can
still be made readable by bypassing AppKit native Markdown for tables, but the
streaming feel is vulnerable because large message updates can trigger
full-stack AppKit rebuilds under one global state lock.

The quick fixes are real but local:

- `app/ui/shared/helpers.rs:2309` bypasses native Markdown when
  `looks_like_markdown_table` returns true. Tests cover this at
  `app/ui/shared/helpers.rs:1877`.
- `app/controller/helpers.rs:345` locally streams final-only assistant text when
  the provider emits only `TextDone`. Tests cover Unicode order preservation.

Those are good guardrails. They do not make chat use the same shell/runtime
matrix as Settings or Onboarding.

## Smallest Shared-Shell Contract

Do not start by rewriting STT, transcription, provider, IPC, packaging, or the
agent runtime. The smallest useful contract is:

1. Shell owns the `NSWindow`/`NSPanel` policy, allocation, presentation, and
   sizing limits.
2. Each feature owns only its content view tree and callbacks.
3. Shell policy is explicit per kind; no caller hand-sets level, opacity,
   collection behavior, titlebar, release policy, min/max size, or presentation.
4. Chat is allowed a keyable custom window class until it moves to Swift/native
   hosting, but that class is an implementation detail passed into shell
   allocation, not a separate policy surface.

Concrete API shape in `app/ui/shared/helpers.rs`:

```rust
pub enum SharedShellKind {
    AgentChat,
    Settings,
    Onboarding,
}

pub struct SharedShellRequest {
    pub kind: SharedShellKind,
    pub frame: CGRect,
    pub title: &'static str,
    pub fixed_size: Option<CGSize>,
    pub visible_frame: Option<CGRect>,
}

pub unsafe fn shared_shell_policy(request: &SharedShellRequest) -> SharedShellPanelPolicy;
pub unsafe fn create_shared_shell_window(
    request: &SharedShellRequest,
    window_class: Option<*const Class>,
) -> Id;
pub unsafe fn present_shared_shell_panel(window: Id);
```

Even smaller first patch if churn must be minimal:

- Add `onboarding_shell_panel_policy(fixed_size)` beside
  `settings_shell_panel_policy`.
- Add `create_shared_shell_window(frame, title, policy, window_class)` so
  Settings, Onboarding, and Agent Chat stop duplicating allocation/policy
  application.
- Leave `VoiceChatOverlayWindow` in `app/ui/voice_chat/handlers.rs`, but pass it
  through `create_shared_shell_window`.

This keeps runtime chat features intact while reducing shell policy drift.

## Swift Tafla Option

The healthier product cut is not "rewrite CodeScribe." It is: rewrite only the
chat shell/content surface as a thin Swift tafla while preserving the Rust/Vista
runtime boundary.

Recommended split:

- Keep Rust/FFI for audio, VAD, STT, transcription persistence, agent session
  events, provider calls, and existing `app/controller/helpers.rs` stream
  fallback behavior.
- Move window controller, focus, scroll, input, markdown blocks, resize, and
  rendering to Swift.
- Keep the old Rust chat overlay behind a fallback flag until the Swift slice
  handles: open, type, stream deltas, final-only fallback stream, render tables,
  resize, copy, attach, and close.

This is justified because the runtime path has already shown it can keep
working while the AppKit overlay hangs. The rotten layer is the Rust/AppKit
window/content stack, not transcription.

## Implementation Guidance

Phase 1: shared shell cleanup inside current CodeScribe Rust UI.

- `app/ui/shared/helpers.rs`: add `SharedShellKind`,
  `onboarding_shell_panel_policy`, and `create_shared_shell_window`.
- `app/ui/settings/mod.rs:864`: replace direct `NSWindow` allocation with
  `create_shared_shell_window`; preserve toolbar setup and fixed size.
- `app/ui/onboarding/mod.rs:576`: replace hand-built policy setters with
  `onboarding_shell_panel_policy` and `present_shared_shell_panel` or an
  explicit `SharedShellPresentation::OrderFrontOnly` if activation during
  onboarding is undesirable.
- `app/ui/voice_chat/mod.rs:209`: replace direct custom-window allocation with
  `create_shared_shell_window(..., Some(overlay_window_class()))`; keep content
  building unchanged.
- Add tests beside `agent_chat_shell_policy_caps_to_visible_frame` for settings
  and onboarding policy invariants.

Phase 2: reduce chat hot-path lock pressure without deleting runtime features.

- `app/ui/voice_chat/state.rs`: split raw window refs from chat message model,
  or introduce a main-thread-only view model adapter so renderer updates do not
  hold the same mutex as state mutation.
- `app/ui/voice_chat/api.rs:1086`: keep in-place streaming as the primary path;
  make full rebuild rare and observable.
- `app/ui/voice_chat/api.rs:1808`: avoid clearing/recreating all bubbles for a
  single streaming delta.

Phase 3: Swift tafla vertical slice if the operator chooses the cleaner cut.

- Keep public Rust functions in `app/ui/voice_chat/mod.rs:13` as the adapter
  surface initially.
- Replace their implementation with FFI/event forwarding to the Swift host.
- Keep `app/controller/helpers.rs:345` final-only local stream behavior until
  Swift proves native streaming preserves Unicode order and pacing.
- Keep `app/ui/shared/helpers.rs:2309` table bypass until Swift has a real block
  Markdown renderer; do not regress table readability during migration.

## Risks To Guard

- Focus: `present_shared_shell_panel` activates the app and makes the window key
  (`app/ui/shared/helpers.rs:1592`). Chat also relies on key/main overrides in
  `VoiceChatOverlayWindow`. A shared shell must not break typing into Agent
  input, nor steal focus during passive transcript display.
- Window level: Agent chat intentionally uses floating level and all-Spaces
  behavior. Settings intentionally uses normal level and fixed preferences
  behavior. Onboarding should not accidentally become floating or all-Spaces
  unless the product wants that.
- Scroll: `update_chat_view_with_state` scrolls to bottom after rebuild. A new
  renderer must preserve bottom pinning for active streams while not fighting a
  user who manually scrolls upward.
- Markdown rendering: AppKit native Markdown in `NSTextField` is not enough for
  tables. Any Swift/AppKit/SwiftUI renderer must prove tables remain readable
  before removing the bypass.
- Resizing: Chat is resizable and currently reflows with `try_lock` at
  `app/ui/voice_chat/api.rs:727`. Settings and Onboarding are fixed-size. Shared
  shell must encode those differences instead of flattening them.

## Validation Commands

Run these commands separately:

```bash
cargo fmt --check
cargo test --lib native_markdown_is_bypassed_for_tables -- --nocapture
cargo test --lib local_chat_stream_chunks_preserve_unicode_and_order -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings
```

Observed result in this pass: all four commands passed.

Note: the combined test command
`cargo test native_markdown_is_bypassed_for_tables local_chat_stream_chunks_preserve_unicode_and_order -- --nocapture`
is invalid Cargo syntax for these targeted tests. Run them separately.

## Boundary

This report intentionally stays out of STT/transcription provider changes,
packaging, and release. If the mission expands into Swift tafla implementation,
the next plan should start in the Swift/FFI checkout and define the exact event
contract before touching runtime transcription.
