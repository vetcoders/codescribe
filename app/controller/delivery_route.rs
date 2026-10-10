//! Delivery throne: one session, one destination, chosen by intent — never by
//! whoever happens to be frontmost at stop.
//!
//! The mic, the transcript, and the agent chain stay other thrones. This module
//! is only the destination axis (operator diagnosis 2026-08-15: "walka o tron").
//!
//! W1-D part set (destination only):
//! - [`DeliveryRoute`] — sole typed destination owner.
//! - [`DeliveryIntent`] — operator intent frozen at session start / overlay click.
//! - [`DeliveryDecision`] — destination-decision part: selected route plus a
//!   recoverable-failure reason token. Route never chooses transcript text;
//!   automatic label authorship lives in the formatter module, not here.
//!
//! Removed competitors that must not return: `assistive_delivery`,
//! `overlay_paste`, `quality_delivery` destination construction, and any
//! second route owner beside [`resolve_delivery_route`].
//!
//! Law:
//! - `DeliveryIntent` is frozen at session start (or at an explicit overlay
//!   click). It is not re-derived from OS focus.
//! - `resolve_delivery_route` is the only function allowed to pick a
//!   [`DeliveryRoute`]. Auto-paste, overlay Insert, and To Agent consult it;
//!   they do not invent a second destination.
//! - The overlay canvas is never a legal Cmd+V target (caret in our panel).
//!   The Agent window, Notes, and every other caret are legal ambulances for
//!   an explicit Insert. Assistive still delivers as a first-class Agent
//!   message — that is a different intent, not a paste ban.
//! - Automatic (Orient) paste obeys one persisted [`PasteMode`] (Founder
//!   2026-09-25: safe / comfort / off). Its gate only ever downgrades
//!   `ClipboardPaste` to [`DeliveryRoute::ClipboardHold`]; terminals get Cmd+V
//!   only when [`looks_executable`] is false, password fields never.
//!
//! # Intended W2 consumers
//! - `app/controller/mod.rs` stop / overlay Insert / To Agent paths that
//!   already import this module. Clipboard, Agent composer, and canvas execute
//!   a decided route; they do not invent one.

use crate::os::hold_badge::FocusedInputField;
use codescribe_core::config::PasteMode;

/// Where a finished transcript is allowed to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRoute {
    /// Spoken intent goes to the Agent composer as a first-class message.
    /// Never a clipboard paste into whatever is focused.
    AgentComposer,
    /// Auto-paste / overlay Insert into the *latched session target*.
    /// Focus at stop time is not the authority.
    ClipboardPaste,
    /// Armed for a later explicit Paste Here. Constructed when the overlay
    /// Insert / defer click refuses a synthetic paste into Codescribe.
    DeferredInsert,
    /// History / notes / RAW only — no user-visible delivery.
    ArchiveOnly,
    /// An armed automatic paste the paste-mode gate or the executable-content
    /// guard stopped. Execution arms the existing Deferred Paste slot without
    /// changing the system clipboard. The configured deferred shortcut inserts
    /// it later; no synthetic Cmd+V is posted by the hold.
    ClipboardHold,
}

/// Transport result for an explicit overlay delivery action. Destination
/// selection still belongs exclusively to [`resolve_delivery_route`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPasteDelivery {
    /// Keyboard events were posted; the recipient has not acknowledged insertion.
    PasteRequested,
    CopiedToClipboard,
    AccessibilityPermissionNeeded,
    DeferredInsertArmed,
    Noop,
}

/// Operator-visible outcome after executing an already selected overlay route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPasteResult {
    pub delivery: OverlayPasteDelivery,
    pub target_app_name: Option<String>,
    pub frontmost_app_name: Option<String>,
    pub deferred_insert_shortcut: Option<String>,
    pub deferred_insert_failure: Option<String>,
}

impl OverlayPasteResult {
    /// No transport ran. Destination selection stays in [`resolve_delivery_route`].
    pub(crate) fn noop() -> Self {
        Self {
            delivery: OverlayPasteDelivery::Noop,
            target_app_name: None,
            frontmost_app_name: None,
            deferred_insert_shortcut: None,
            deferred_insert_failure: None,
        }
    }
}

impl DeliveryRoute {
    /// Stable telemetry label (snake_case, one token).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentComposer => "agent_composer",
            Self::ClipboardPaste => "clipboard_paste",
            Self::DeferredInsert => "deferred_insert",
            Self::ArchiveOnly => "archive_only",
            Self::ClipboardHold => "clipboard_hold",
        }
    }
}

/// Operator delivery intent, frozen at session start (Orient, AgentVoice,
/// NotesOnly) or at the explicit overlay click that declares it (overlay
/// intents). OS focus at stop time is never an input.
///
/// The stop path lost its intents in the W0 authority demolition
/// (`ac6d399b3`, 2026-08-24) and with them auto-paste. Restored 2026-09-08:
/// Auto Paste is the product's basic verb (Founder C02 2026-07-20, one
/// persisted setting shared by Hold and Double Left Option).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryIntent {
    /// Assistive hold / Double Right Option: the transcript is a first-class
    /// Agent message. Never a focus-derived paste.
    AgentVoice,
    /// Hold Fn / Globe or toggle dictation: auto-paste into the latched target.
    OrientDictation,
    /// Double Left Option: formatted dictation. Same destination as dictation.
    OrientFormat,
    /// Save-only Quick Notes: history only, no user-visible delivery.
    NotesOnly,
    /// Explicit overlay "To Agent" after any session.
    OverlayToAgent,
    /// Explicit overlay Insert / Paste Here. Frozen at the click, not at stop.
    OverlayInsert,
}

impl DeliveryIntent {
    /// Stable telemetry label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentVoice => "agent_voice",
            Self::OrientDictation => "orient_dictation",
            Self::OrientFormat => "orient_format",
            Self::NotesOnly => "notes_only",
            Self::OverlayToAgent => "overlay_to_agent",
            Self::OverlayInsert => "overlay_insert",
        }
    }
}

/// Freeze the stop-path intent from the flags the session started with.
///
/// Save-only notes outrank everything (nothing may leave history). Assistive
/// outranks formatting: an assistive hold is an Agent message even when AI
/// formatting is on. Everything else is Orient dictation.
pub fn delivery_intent_from_session(
    assistive: bool,
    force_ai: bool,
    notes_save_only: bool,
) -> DeliveryIntent {
    if notes_save_only {
        DeliveryIntent::NotesOnly
    } else if assistive {
        DeliveryIntent::AgentVoice
    } else if force_ai {
        DeliveryIntent::OrientFormat
    } else {
        DeliveryIntent::OrientDictation
    }
}

/// The caret an automatic paste would land in, observed at stop. It can only
/// hold an armed paste; it never names a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteTarget {
    /// The receiving app is a terminal emulator ([`is_terminal_app`]).
    pub terminal: bool,
    /// Text-input shape of the focused element.
    pub field: FocusedInputField,
}

impl PasteTarget {
    /// Nothing observed: not a terminal, field unreadable.
    pub const UNOBSERVED: Self = Self {
        terminal: false,
        field: FocusedInputField::Unobserved,
    };
}

/// Facts the destination function is allowed to read. Focus-at-stop is not a
/// destination input; [`PasteTarget`] may only hold an armed paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryFacts {
    pub has_text: bool,
    pub no_speech: bool,
    /// Automatic paste policy. Only Orient intents read it.
    pub paste_mode: PasteMode,
    /// Caret facts for the paste-mode gate. Only Orient intents read it.
    pub paste_target: PasteTarget,
    /// [`looks_executable`] on the committed (untagged) text.
    pub executable_payload: bool,
    pub overlay_enabled: bool,
    pub live_stream_session: bool,
    pub commit_required: bool,
    /// True only when the overlay canvas holds the caret. The Codescribe
    /// **app** (Agent window, Settings) is not this flag — those are legal
    /// Cmd+V sinks. Swift `defer_text_from_overlay` is the constructor.
    pub latched_target_is_self: bool,
}

/// Typed destination-decision part: operator intent resolved to a
/// [`DeliveryRoute`] plus a stable reason token.
///
/// Success and recoverable failure share this shape. Failure still names the
/// parked route (`DeferredInsert`, `ArchiveOnly`, …) so the
/// stop path can recover without inventing a second destination owner or
/// choosing transcript text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryDecision {
    pub route: DeliveryRoute,
    pub reason: &'static str,
}

/// Read-only projection of which overlay actions are legal for one immutable
/// take snapshot. It does not execute delivery or choose transcript text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TranscriptProjectionAvailability {
    pub can_paste: bool,
    pub can_insert: bool,
    pub can_copy: bool,
    pub can_retranscribe: bool,
    pub can_format: bool,
    pub can_send_to_agent: bool,
}

/// Facts an overlay Insert / defer click may feed the throne.
///
/// Focus-at-click is not an input. `latched_target_is_self` is true only when
/// Swift already knows the overlay canvas holds the caret
/// (`defer_text_from_overlay`). A latched Codescribe **app** name is the
/// Agent window, not this flag.
pub fn overlay_insert_facts(has_text: bool, latched_target_is_self: bool) -> DeliveryFacts {
    DeliveryFacts {
        has_text,
        no_speech: false,
        paste_mode: PasteMode::Off,
        paste_target: PasteTarget::UNOBSERVED,
        executable_payload: false,
        overlay_enabled: true,
        live_stream_session: false,
        commit_required: false,
        latched_target_is_self,
    }
}

/// Single destination function. Advisors (quality gate, overlay flag, paste
/// mode, executable-content guard) may veto or hold a paste; they may not pick
/// a different throne.
pub fn resolve_delivery_route(intent: DeliveryIntent, facts: DeliveryFacts) -> DeliveryDecision {
    if !facts.has_text || facts.no_speech {
        return DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "empty_or_no_speech",
        };
    }

    match intent {
        DeliveryIntent::AgentVoice => DeliveryDecision {
            route: DeliveryRoute::AgentComposer,
            reason: "assistive_first_class",
        },
        DeliveryIntent::NotesOnly => DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "notes_save_only",
        },
        DeliveryIntent::OrientDictation | DeliveryIntent::OrientFormat => orient_route(facts),
        DeliveryIntent::OverlayToAgent => DeliveryDecision {
            route: DeliveryRoute::AgentComposer,
            reason: "explicit_to_agent",
        },
        DeliveryIntent::OverlayInsert => overlay_insert_route(facts),
    }
}

/// Stop-path Orient (Hold Fn / Globe, Double Left Option, toggle Finish).
///
/// [`PasteMode`] is one persisted setting shared by every Orient gesture. The
/// vetoes that keep Orient off the paste gun, in order: mode Off, a
/// live-stream consumer that already owns the text, a pending quality commit,
/// and the overlay canvas holding the caret. The doc table's `OrientCanvas` is
/// `ArchiveOnly` here: the canvas already shows the committed document, so
/// nothing else moves. What survives goes through [`paste_gate`].
fn orient_route(facts: DeliveryFacts) -> DeliveryDecision {
    if facts.paste_mode == PasteMode::Off {
        return DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "paste_mode_off",
        };
    }
    if facts.live_stream_session {
        return DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "live_stream_session",
        };
    }
    if facts.commit_required {
        return DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "quality_commit_pending",
        };
    }
    if facts.latched_target_is_self {
        return DeliveryDecision {
            route: DeliveryRoute::DeferredInsert,
            reason: "refuse_paste_into_self",
        };
    }
    paste_gate(
        facts.paste_mode,
        facts.paste_target,
        facts.executable_payload,
    )
}

/// May an armed automatic paste fire at this caret?
///
/// - A password field (or a prompt holding secure input) holds in every mode:
///   dictation never lands in a secret field (Founder s03-028).
/// - A terminal gets Cmd+V only when the text does not look executable; a
///   command-shaped take is held for the user's own ⌘V (Founder s04-036).
/// - Both automatic modes require an observed editable input. App focus or a
///   text-shaped role alone never authorizes a keyboard shortcut.
///
/// The gate can only answer `ClipboardPaste` or `ClipboardHold`.
fn paste_gate(mode: PasteMode, target: PasteTarget, executable: bool) -> DeliveryDecision {
    let hold = |reason| DeliveryDecision {
        route: DeliveryRoute::ClipboardHold,
        reason,
    };
    if target.field == FocusedInputField::Secure {
        return hold("hold_secure_field");
    }
    if target.terminal && executable {
        return hold("hold_executable");
    }
    match target.field {
        FocusedInputField::Text | FocusedInputField::Secure => {}
        FocusedInputField::NotText => return hold("hold_no_text_field"),
        FocusedInputField::Unobserved => return hold("hold_field_unobserved"),
    }
    DeliveryDecision {
        route: DeliveryRoute::ClipboardPaste,
        reason: if mode == PasteMode::Comfort {
            "paste_comfort"
        } else {
            "paste_safe"
        },
    }
}

/// User-facing words for a held paste, keyed by the gate's reason token.
/// `None` for every decision that is not a hold.
pub fn paste_hold_notice(decision: DeliveryDecision) -> Option<&'static str> {
    if decision.route != DeliveryRoute::ClipboardHold {
        return None;
    }
    Some(match decision.reason {
        "hold_executable" => {
            "Held: this looks like a shell command. It is waiting in Deferred Paste."
        }
        "hold_secure_field" => "Held: a password field has focus. It is waiting in Deferred Paste.",
        "hold_no_text_field" => "Held: no text field had focus. It is waiting in Deferred Paste.",
        _ => "Held: Codescribe could not confirm a text field. It is waiting in Deferred Paste.",
    })
}

/// Terminal emulators, matched case-insensitively against the app name the
/// latch and the frontmost probe report (`NSRunningApplication.localizedName`).
/// Zellij and tmux run inside one of these, so they are covered by the host.
pub const TERMINAL_APPS: &[&str] = &[
    "Terminal",
    "iTerm2",
    "iTerm",
    "Ghostty",
    "Alacritty",
    "kitty",
    "WezTerm",
    "Warp",
    "Hyper",
    "Tabby",
    "Rio",
    "Wave",
    "vc-terminal",
];

/// Whether `app_name` is a terminal emulator from [`TERMINAL_APPS`].
pub fn is_terminal_app(app_name: &str) -> bool {
    let name = app_name.trim();
    TERMINAL_APPS
        .iter()
        .any(|terminal| terminal.eq_ignore_ascii_case(name))
}

/// First words that make a line a shell command on their own.
const COMMAND_WORDS: &[&str] = &[
    "sudo",
    "su",
    "doas",
    "rm",
    "rmdir",
    "mv",
    "cp",
    "ln",
    "chmod",
    "chown",
    "chgrp",
    "dd",
    "mkfs",
    "diskutil",
    "launchctl",
    "killall",
    "pkill",
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "git",
    "gh",
    "brew",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "node",
    "pip",
    "pip3",
    "pipx",
    "uv",
    "uvx",
    "python",
    "python3",
    "ruby",
    "perl",
    "cargo",
    "rustup",
    "rustc",
    "docker",
    "podman",
    "kubectl",
    "helm",
    "terraform",
    "bash",
    "sh",
    "zsh",
    "fish",
    "eval",
    "exec",
    "osascript",
    "xattr",
    "codesign",
    "spctl",
    "csrutil",
    "nvram",
    "sqlite3",
    "psql",
    "mysql",
    "systemctl",
    "apt",
    "apt-get",
    "dnf",
    "yum",
    "tmux",
    "zellij",
    "ls",
    "cd",
    "mkdir",
    "grep",
    "rg",
    "sed",
    "awk",
    "tar",
    "unzip",
    "crontab",
    "shred",
    "xargs",
    "nohup",
    "chsh",
    "pbcopy",
    "pbpaste",
    "ps",
    "du",
    "df",
];

/// Binaries that are also everyday English words. They count only when the
/// next token looks like a CLI argument (`make sure` is prose, `kill -9` not).
const AMBIGUOUS_COMMAND_WORDS: &[&str] = &[
    "make", "find", "open", "kill", "cat", "echo", "export", "source", "set", "unset", "test",
    "touch", "top", "less", "more", "head", "tail", "sort", "cut", "date", "which", "man", "say",
    "type", "go", "defaults", "alias", "history", "clear", "exit", "time", "watch", "tee", "diff",
    "file", "who",
];

/// Prompt glyphs a pasted line may carry from a copied shell session.
const PROMPT_MARKERS: &[&str] = &["$ ", "% ", "❯ ", "➜ "];

/// Case-insensitive: dictation capitalizes the first word ("Sudo rm …") and
/// the default macOS volume resolves `Git` to `git` all the same.
fn word_in(list: &[&str], token: &str) -> bool {
    list.iter().any(|word| word.eq_ignore_ascii_case(token))
}

fn is_command_word(token: &str) -> bool {
    word_in(COMMAND_WORDS, token) || word_in(AMBIGUOUS_COMMAND_WORDS, token)
}

/// `-9`, `/tmp`, `~/x`, `./run`, `$HOME`, `a=b`, `*.log`, `notes.txt`, `4242`.
fn looks_like_argument(token: &str) -> bool {
    let inner = token.trim_end_matches(['.', ',', '!', '?', ':', ';']);
    token.starts_with(['-', '/', '~', '.', '$', '"', '\''])
        || token.contains(['=', '*', '/'])
        || inner.contains('.')
        || (!token.is_empty() && token.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
            && !name.starts_with(|ch: char| ch.is_ascii_digit())
    })
}

/// A command-shaped word sequence: optional `VAR=value` prefixes, then a
/// command word (ambiguous ones need an argument-looking next token).
fn starts_with_command(tokens: &[&str]) -> bool {
    let mut rest = tokens
        .iter()
        .copied()
        .skip_while(|token| is_env_assignment(token));
    let Some(first) = rest.next() else {
        return false;
    };
    if word_in(COMMAND_WORDS, first) {
        return true;
    }
    word_in(AMBIGUOUS_COMMAND_WORDS, first) && rest.next().is_some_and(looks_like_argument)
}

/// The word right after `separator` names a command (`| sh`, `; rm`).
fn command_follows(line: &str, separator: char) -> bool {
    line.split(separator)
        .skip(1)
        .any(|tail| tail.split_whitespace().next().is_some_and(is_command_word))
}

/// Shell constructs that execute or chain even inside prose.
fn has_shell_construct(line: &str) -> bool {
    if ["$(", "${", "&&", "||", "<(", ">>", "2>&1"]
        .iter()
        .any(|construct| line.contains(construct))
    {
        return true;
    }
    // Backticks run their content in a shell; only a command inside counts,
    // so `parse_mode` in a sentence stays prose.
    let quoted_command = line.split('`').skip(1).step_by(2).any(|inside| {
        let tokens: Vec<&str> = inside.split_whitespace().collect();
        starts_with_command(&tokens)
    });
    if quoted_command || command_follows(line, '|') || command_follows(line, ';') {
        return true;
    }
    // Redirection into a path (`> /etc/hosts`, `>~/out`), not `2 > 1`.
    line.match_indices('>')
        .any(|(index, _)| line[index + 1..].trim_start().starts_with(['/', '~', '.']))
}

/// Executable-content guard for terminal targets (Founder s04-036): does this
/// text look like something a shell would run?
///
/// Deliberately a heuristic that leans toward holding: a false positive costs
/// one ⌘V, a false negative can run a command. Checked per line: a copied
/// prompt (`$ git status`), a leading command word (`sudo`, `rm`, `git`,
/// `curl`, …), or a chaining / substitution construct (`$(`, backticks around
/// a command, `&&`, `| sh`, `; rm`, `> /path`).
pub fn looks_executable(text: &str) -> bool {
    text.lines().map(str::trim).any(|line| {
        if line.is_empty() {
            return false;
        }
        if let Some(body) = PROMPT_MARKERS
            .iter()
            .find_map(|marker| line.strip_prefix(marker))
        {
            return !body.trim().is_empty();
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        starts_with_command(&tokens) || has_shell_construct(line)
    })
}

/// Derive canvas availability through the delivery throne plus immutable book,
/// audio, and lifecycle facts. A caller may paint these bits but must not
/// reconstruct them independently.
pub(crate) fn resolve_transcript_projection_availability(
    has_text: bool,
    take_in_progress: bool,
    session_wav_exists: bool,
    has_latched_target: bool,
    latched_target_is_self: bool,
) -> TranscriptProjectionAvailability {
    let insert = resolve_delivery_route(
        DeliveryIntent::OverlayInsert,
        overlay_insert_facts(has_text, latched_target_is_self),
    );
    let insert_route_is_legal = matches!(
        insert.route,
        DeliveryRoute::ClipboardPaste
            | DeliveryRoute::ClipboardHold
            | DeliveryRoute::DeferredInsert
    );

    // A historical projection cannot observe today's caret. Keep the explicit
    // action available so its executor can inspect the retained target; this
    // flag never authorizes posting keyboard events.
    TranscriptProjectionAvailability {
        can_paste: !take_in_progress
            && has_latched_target
            && matches!(
                insert.route,
                DeliveryRoute::ClipboardPaste | DeliveryRoute::ClipboardHold
            ),
        can_insert: !take_in_progress && insert_route_is_legal,
        can_copy: has_text,
        can_retranscribe: !take_in_progress && session_wav_exists,
        can_format: !take_in_progress && has_text,
        can_send_to_agent: !take_in_progress
            && matches!(
                resolve_delivery_route(
                    DeliveryIntent::OverlayToAgent,
                    overlay_insert_facts(has_text, latched_target_is_self),
                )
                .route,
                DeliveryRoute::AgentComposer
            ),
    }
}

/// Explicit overlay click. Orient vetoes (live stream, quality commit) do not
/// apply — the user asked to insert *now*. Overlay **caret** still refuses
/// Cmd+V into the canvas (`latched_target_is_self` from Swift). A latched
/// Codescribe **app** name is the Agent window, not the canvas.
fn overlay_insert_route(facts: DeliveryFacts) -> DeliveryDecision {
    if facts.latched_target_is_self {
        return DeliveryDecision {
            route: DeliveryRoute::DeferredInsert,
            reason: "refuse_paste_into_self",
        };
    }
    let gate = paste_gate(
        PasteMode::Safe,
        facts.paste_target,
        facts.executable_payload,
    );
    if gate.route == DeliveryRoute::ClipboardPaste {
        DeliveryDecision {
            route: DeliveryRoute::ClipboardPaste,
            reason: "explicit_insert",
        }
    } else {
        gate
    }
}

/// Transport law for an already decided `ClipboardPaste`: may the synthetic
/// Cmd+V be posted right now?
///
/// - A **latched** target must have confirmed focus (bounded wait) or be
///   observed frontmost afterwards. It never yields to whoever happens to be
///   frontmost — that fallback re-admitted "focus at stop" through the back
///   door (`2fb2bd8ec`, 2026-09-08) and was the canary finding P1-01
///   "auto-paste accepts unconfirmed activation" (2026-08-24).
/// - With **no** latch (an overlay Insert started from Codescribe itself, so
///   the latch never stored a target) the external frontmost app is the only
///   caret the user can mean; Codescribe's own windows are still refused.
///
/// Event-tap permission is checked by the caller; this is destination law only.
#[must_use]
pub const fn clipboard_paste_may_post(
    target_latched: bool,
    focus_confirmed: bool,
    target_observed_frontmost: bool,
    frontmost_is_external: bool,
) -> bool {
    if target_latched {
        focus_confirmed || target_observed_frontmost
    } else {
        frontmost_is_external
    }
}

/// One INFO line: route, reason, intent, latched target. The stop-path budget
/// already has a `delivery_secs` phase; this names *where* those seconds went.
pub fn format_delivery_route_line(
    intent: DeliveryIntent,
    decision: DeliveryDecision,
    latched_target: Option<&str>,
) -> String {
    format!(
        "delivery_route: intent={intent} route={route} reason={reason} target={target}",
        intent = intent.as_str(),
        route = decision.route.as_str(),
        reason = decision.reason,
        target = latched_target.unwrap_or("-"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::selection::is_codescribe_app;

    const TEXT_FIELD: PasteTarget = PasteTarget {
        terminal: false,
        field: FocusedInputField::Text,
    };

    fn facts(overrides: impl FnOnce(&mut DeliveryFacts)) -> DeliveryFacts {
        let mut f = DeliveryFacts {
            has_text: true,
            no_speech: false,
            paste_mode: PasteMode::Safe,
            paste_target: TEXT_FIELD,
            executable_payload: false,
            overlay_enabled: true,
            live_stream_session: false,
            commit_required: false,
            latched_target_is_self: false,
        };
        overrides(&mut f);
        f
    }

    #[test]
    fn empty_or_no_speech_archives_regardless_of_intent() {
        for intent in [
            DeliveryIntent::AgentVoice,
            DeliveryIntent::OrientDictation,
            DeliveryIntent::OrientFormat,
            DeliveryIntent::NotesOnly,
            DeliveryIntent::OverlayToAgent,
            DeliveryIntent::OverlayInsert,
        ] {
            let empty = resolve_delivery_route(
                intent,
                facts(|f| {
                    f.has_text = false;
                }),
            );
            assert_eq!(empty.route, DeliveryRoute::ArchiveOnly, "{intent:?}");
            assert_eq!(empty.reason, "empty_or_no_speech");

            let silent = resolve_delivery_route(
                intent,
                facts(|f| {
                    f.no_speech = true;
                }),
            );
            assert_eq!(silent.route, DeliveryRoute::ArchiveOnly, "{intent:?}");
        }
    }

    #[test]
    fn orient_pastes_into_the_latched_target_when_safe_sees_a_field() {
        for intent in [
            DeliveryIntent::OrientDictation,
            DeliveryIntent::OrientFormat,
        ] {
            let decision = resolve_delivery_route(intent, facts(|_| {}));
            assert_eq!(decision.route, DeliveryRoute::ClipboardPaste, "{intent:?}");
            assert_eq!(decision.reason, "paste_safe");
        }
    }

    #[test]
    fn paste_mode_off_archives_only_whatever_the_caret() {
        for target in [
            TEXT_FIELD,
            PasteTarget::UNOBSERVED,
            PasteTarget {
                terminal: true,
                field: FocusedInputField::Unobserved,
            },
        ] {
            let decision = resolve_delivery_route(
                DeliveryIntent::OrientDictation,
                facts(|f| {
                    f.paste_mode = PasteMode::Off;
                    f.paste_target = target;
                    f.executable_payload = true;
                }),
            );
            assert_eq!(decision.route, DeliveryRoute::ArchiveOnly, "{target:?}");
            assert_eq!(decision.reason, "paste_mode_off");
        }
    }

    /// (mode, terminal, field, executable) → (route, reason). The gate only
    /// ever answers ClipboardPaste or ClipboardHold.
    #[test]
    fn paste_gate_matrix_for_safe_and_comfort() {
        use DeliveryRoute::{ClipboardHold as Hold, ClipboardPaste as Paste};
        use FocusedInputField::{NotText, Secure, Text, Unobserved};
        use PasteMode::{Comfort, Safe};
        let cases = [
            (Safe, false, Text, false, Paste, "paste_safe"),
            (Safe, false, NotText, false, Hold, "hold_no_text_field"),
            (
                Safe,
                false,
                Unobserved,
                false,
                Hold,
                "hold_field_unobserved",
            ),
            (Safe, false, Secure, false, Hold, "hold_secure_field"),
            // Executable text into a non-terminal field is not a shell.
            (Safe, false, Text, true, Paste, "paste_safe"),
            // Unobserved terminal input is retained, even for ordinary prose.
            (Safe, true, Unobserved, false, Hold, "hold_field_unobserved"),
            (Safe, true, Text, true, Hold, "hold_executable"),
            (Safe, true, Secure, false, Hold, "hold_secure_field"),
            (Comfort, false, Text, false, Paste, "paste_comfort"),
            (Comfort, false, NotText, false, Hold, "hold_no_text_field"),
            (
                Comfort,
                false,
                Unobserved,
                false,
                Hold,
                "hold_field_unobserved",
            ),
            (Comfort, false, Secure, false, Hold, "hold_secure_field"),
            (
                Comfort,
                true,
                Unobserved,
                false,
                Hold,
                "hold_field_unobserved",
            ),
            (Comfort, true, Unobserved, true, Hold, "hold_executable"),
            (Comfort, true, Secure, true, Hold, "hold_secure_field"),
        ];
        for (mode, terminal, field, executable, route, reason) in cases {
            let decision = resolve_delivery_route(
                DeliveryIntent::OrientFormat,
                facts(|f| {
                    f.paste_mode = mode;
                    f.paste_target = PasteTarget { terminal, field };
                    f.executable_payload = executable;
                }),
            );
            let case = format!("{mode:?} terminal={terminal} {field:?} exec={executable}");
            assert_eq!(decision.route, route, "{case}");
            assert_eq!(decision.reason, reason, "{case}");
        }
    }

    #[test]
    fn orient_vetoes_outrank_the_paste_gate() {
        let into_self = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.latched_target_is_self = true;
                f.paste_target.field = FocusedInputField::Secure;
            }),
        );
        assert_eq!(into_self.route, DeliveryRoute::DeferredInsert);
        let live = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.live_stream_session = true;
                f.paste_target.terminal = true;
                f.executable_payload = true;
            }),
        );
        assert_eq!(live.route, DeliveryRoute::ArchiveOnly);
    }

    /// Explicit Insert bypasses automatic mode Off, but still needs a writable
    /// non-secret field and refuses executable text in a terminal.
    #[test]
    fn explicit_insert_bypasses_mode_off_but_keeps_input_and_terminal_guards() {
        let confirmed_input = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| f.paste_mode = PasteMode::Off),
        );
        assert_eq!(confirmed_input.route, DeliveryRoute::ClipboardPaste);
        assert_eq!(confirmed_input.reason, "explicit_insert");

        let decision = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| {
                f.paste_mode = PasteMode::Off;
                f.paste_target = PasteTarget {
                    terminal: true,
                    field: FocusedInputField::Secure,
                };
                f.executable_payload = true;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::ClipboardHold);
        assert_eq!(decision.reason, "hold_secure_field");

        let executable = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| {
                f.paste_mode = PasteMode::Off;
                f.paste_target.terminal = true;
                f.executable_payload = true;
            }),
        );
        assert_eq!(executable.route, DeliveryRoute::ClipboardHold);
        assert_eq!(executable.reason, "hold_executable");
    }

    #[test]
    fn hold_notice_speaks_only_for_holds() {
        let paste = DeliveryDecision {
            route: DeliveryRoute::ClipboardPaste,
            reason: "paste_safe",
        };
        assert_eq!(paste_hold_notice(paste), None);
        for reason in [
            "hold_executable",
            "hold_secure_field",
            "hold_no_text_field",
            "hold_field_unobserved",
        ] {
            let notice = paste_hold_notice(DeliveryDecision {
                route: DeliveryRoute::ClipboardHold,
                reason,
            })
            .expect("a hold always explains itself");
            assert!(notice.contains("Deferred Paste"), "{reason}: {notice}");
        }
        assert!(
            paste_hold_notice(DeliveryDecision {
                route: DeliveryRoute::ClipboardHold,
                reason: "hold_executable",
            })
            .is_some_and(|notice| notice.contains("shell command"))
        );
    }

    #[test]
    fn terminal_apps_match_case_insensitively() {
        for name in [
            "Terminal",
            "iTerm2",
            "Ghostty",
            " alacritty ",
            "kitty",
            "WezTerm",
            "Warp",
            "vc-terminal",
        ] {
            assert!(is_terminal_app(name), "{name}");
        }
        for name in [
            "Notes",
            "Codescribe",
            "Cursor",
            "Safari",
            "Terminal Tips",
            "",
        ] {
            assert!(!is_terminal_app(name), "{name}");
        }
    }

    #[test]
    fn guard_flags_command_shaped_text() {
        for text in [
            "sudo rm -rf /",
            "Sudo rm -rf /tmp/build.",
            "rm -rf ~/scratch",
            "git push --force origin main",
            "Git push",
            "curl -fsSL https://example.com/install.sh | sh",
            "wget https://example.com/x",
            "echo $(whoami)",
            "cd build && make",
            "run tests || exit",
            "$ brew install ripgrep",
            "% ls -la",
            "❯ cargo test",
            "please run `rm -rf target` now",
            "kill -9 4242",
            "open ~/Downloads",
            "cat notes.txt",
            "FOO=1 cargo build",
            "print it; rm the file",
            "save it > ~/out.txt",
            "Zrób podsumowanie dnia\n$ git status",
            "ls",
        ] {
            assert!(looks_executable(text), "should hold: {text:?}");
        }
    }

    #[test]
    fn guard_leaves_prose_alone() {
        for text in [
            "",
            "   ",
            "Make sure the tests pass.",
            "make sure the tests pass",
            "Find the bug in the parser",
            "Open the file and read it to me",
            "Zrób mi podsumowanie dnia i wyślij je do zespołu",
            "Fix the `parse_mode` function please",
            "Let's meet at five; bring snacks",
            "I think 2 > 1 is true",
            "The cat is on the table",
            "echo chamber is a real problem",
            "A -> B is the arrow",
            "50% of the time it works",
            "Kill the lights when you leave",
        ] {
            assert!(!looks_executable(text), "should paste: {text:?}");
        }
    }

    #[test]
    fn orient_honours_live_stream_and_commit_vetoes() {
        let live = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.live_stream_session = true;
            }),
        );
        assert_eq!(live.route, DeliveryRoute::ArchiveOnly);
        assert_eq!(live.reason, "live_stream_session");

        let commit = resolve_delivery_route(
            DeliveryIntent::OrientFormat,
            facts(|f| {
                f.commit_required = true;
            }),
        );
        assert_eq!(commit.route, DeliveryRoute::ArchiveOnly);
        assert_eq!(commit.reason, "quality_commit_pending");
    }

    #[test]
    fn orient_into_self_parks_paste_here() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.latched_target_is_self = true;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::DeferredInsert);
        assert_eq!(decision.reason, "refuse_paste_into_self");
    }

    #[test]
    fn agent_voice_never_auto_pastes() {
        let decision = resolve_delivery_route(DeliveryIntent::AgentVoice, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::AgentComposer);
        assert_eq!(decision.reason, "assistive_first_class");
    }

    #[test]
    fn notes_only_archives_even_with_paste_armed() {
        let decision = resolve_delivery_route(DeliveryIntent::NotesOnly, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::ArchiveOnly);
        assert_eq!(decision.reason, "notes_save_only");
    }

    #[test]
    fn session_intent_precedence_is_notes_then_assistive_then_format() {
        assert_eq!(
            delivery_intent_from_session(true, true, true),
            DeliveryIntent::NotesOnly
        );
        assert_eq!(
            delivery_intent_from_session(true, true, false),
            DeliveryIntent::AgentVoice
        );
        assert_eq!(
            delivery_intent_from_session(false, true, false),
            DeliveryIntent::OrientFormat
        );
        assert_eq!(
            delivery_intent_from_session(false, false, false),
            DeliveryIntent::OrientDictation
        );
    }

    #[test]
    fn overlay_to_agent_is_first_class_not_focus_paste() {
        let decision = resolve_delivery_route(DeliveryIntent::OverlayToAgent, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::AgentComposer);
        assert_eq!(decision.reason, "explicit_to_agent");
    }

    #[test]
    fn overlay_insert_to_foreign_app_is_clipboard_paste() {
        let decision = resolve_delivery_route(DeliveryIntent::OverlayInsert, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::ClipboardPaste);
        assert_eq!(decision.reason, "explicit_insert");
    }

    #[test]
    fn explicit_insert_requires_editable_input_in_every_app() {
        for terminal in [false, true] {
            for field in [
                FocusedInputField::NotText,
                FocusedInputField::Unobserved,
                FocusedInputField::Secure,
            ] {
                let decision = resolve_delivery_route(
                    DeliveryIntent::OverlayInsert,
                    facts(|f| {
                        f.paste_target = PasteTarget { terminal, field };
                    }),
                );
                assert_eq!(decision.route, DeliveryRoute::ClipboardHold);
            }
        }
    }

    #[test]
    fn overlay_insert_into_self_is_deferred() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| {
                f.latched_target_is_self = true;
                f.paste_mode = PasteMode::Comfort;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::DeferredInsert);
        assert_eq!(decision.reason, "refuse_paste_into_self");
    }

    #[test]
    fn overlay_insert_ignores_live_stream_and_commit_vetoes() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| {
                f.live_stream_session = true;
                f.commit_required = true;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::ClipboardPaste);
        assert_eq!(decision.reason, "explicit_insert");
    }

    #[test]
    fn overlay_insert_facts_are_the_click_constructor() {
        let click = overlay_insert_facts(true, true);
        assert_eq!(click.paste_mode, PasteMode::Off);
        assert_eq!(click.paste_target, PasteTarget::UNOBSERVED);
        assert!(!click.executable_payload);
        assert!(click.overlay_enabled);
        assert!(click.latched_target_is_self);
        let decision = resolve_delivery_route(DeliveryIntent::OverlayInsert, click);
        assert_eq!(decision.route, DeliveryRoute::DeferredInsert);
    }

    #[test]
    fn projection_offers_explicit_target_check_without_authorizing_a_shortcut() {
        let projection = resolve_transcript_projection_availability(true, false, true, true, false);
        assert!(projection.can_insert);
        assert!(projection.can_paste);
        let unobserved = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            overlay_insert_facts(true, false),
        );
        assert_eq!(unobserved.route, DeliveryRoute::ClipboardHold);
        assert_eq!(unobserved.reason, "hold_field_unobserved");
        let no_target = resolve_transcript_projection_availability(true, false, true, false, false);
        assert!(no_target.can_insert);
        assert!(!no_target.can_paste);
    }

    #[test]
    fn projection_availability_follows_book_lifecycle_audio_and_delivery_table() {
        let cases = [
            (
                "listening",
                (true, true, false, true, false),
                TranscriptProjectionAvailability {
                    can_paste: false,
                    can_insert: false,
                    can_copy: true,
                    can_retranscribe: false,
                    can_format: false,
                    can_send_to_agent: false,
                },
            ),
            (
                "formatted_foreign_target",
                (true, false, true, true, false),
                TranscriptProjectionAvailability {
                    can_paste: true,
                    can_insert: true,
                    can_copy: true,
                    can_retranscribe: true,
                    can_format: true,
                    can_send_to_agent: true,
                },
            ),
            (
                "formatted_self_target",
                (true, false, true, true, true),
                TranscriptProjectionAvailability {
                    can_paste: false,
                    can_insert: true,
                    can_copy: true,
                    can_retranscribe: true,
                    can_format: true,
                    can_send_to_agent: true,
                },
            ),
            (
                "no_speech",
                (false, false, true, true, false),
                TranscriptProjectionAvailability {
                    can_paste: false,
                    can_insert: false,
                    can_copy: false,
                    can_retranscribe: true,
                    can_format: false,
                    can_send_to_agent: false,
                },
            ),
        ];

        for (name, (has_text, in_progress, wav, has_target, target_is_self), expected) in cases {
            assert_eq!(
                resolve_transcript_projection_availability(
                    has_text,
                    in_progress,
                    wav,
                    has_target,
                    target_is_self,
                ),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn codescribe_is_self_case_insensitive() {
        assert!(is_codescribe_app("Codescribe"));
        assert!(is_codescribe_app(" codescribe "));
        assert!(!is_codescribe_app("Ghostty"));
        assert!(!is_codescribe_app(""));
    }

    #[test]
    fn latched_target_unconfirmed_never_follows_foreign_frontmost() {
        // Today's take: target latched, activation unconfirmed, a foreign app frontmost.
        assert!(!clipboard_paste_may_post(true, false, false, true));
        assert!(!clipboard_paste_may_post(true, false, false, false));
    }

    #[test]
    fn latched_target_posts_when_confirmed_or_observed() {
        assert!(clipboard_paste_may_post(true, true, false, false));
        assert!(clipboard_paste_may_post(true, false, true, false));
    }

    #[test]
    fn unlatched_insert_follows_external_frontmost_only() {
        assert!(clipboard_paste_may_post(false, false, false, true));
        assert!(!clipboard_paste_may_post(false, false, false, false));
    }

    #[test]
    fn budget_line_names_the_throne() {
        let line = format_delivery_route_line(
            DeliveryIntent::OverlayInsert,
            DeliveryDecision {
                route: DeliveryRoute::ClipboardPaste,
                reason: "explicit_insert",
            },
            Some("Ghostty"),
        );
        assert_eq!(
            line,
            "delivery_route: intent=overlay_insert route=clipboard_paste reason=explicit_insert target=Ghostty"
        );
    }
}
