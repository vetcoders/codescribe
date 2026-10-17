/**
 * Codescribe for OMP (oh-my-pi): identity, `/codescribe NAME CHANNEL [VOICE]`
 * and the idle wakeup, inside the current OMP conversation.
 *
 * OMP loads this file as an extension module from the marketplace plugin's
 * `hooks/pre/` directory (Claude Code and Junie never read it; their wake is
 * `hooks/claude-code.json`). Reading, ACK and voice stay with the `codescribe_*`
 * MCP tools served by `cs-bus --mcp` from `.mcp.json`. This module only:
 *
 * 1. tells the model the conversation's real identity (`provider=omp`,
 *    the OMP session id) once per session start;
 * 2. holds `cs-bus --omp-wake` while the conversation is idle and, when the
 *    helper exits 75 with one announced delivery, injects that sentence as a
 *    turn-starting message, exactly like the Claude Code asyncRewake hook;
 * 3. re-arms after every agent turn and stops the waiter on shutdown.
 *
 * The waiter never reads, acknowledges or replies.
 */
import type { ExtensionAPI, ExtensionContext } from "@oh-my-pi/pi-coding-agent";

const PROVIDER = "omp";
/** `WAKE_EXIT` in scripts/bus-mcp.py: one unannounced delivery, problem or idle receipt. */
const WAKE_EXIT = 75;
/** Above `WAKE_SECONDS` (6 h) in scripts/bus-mcp.py, like the Claude Code hook timeout. */
const WAKE_TIMEOUT_MS = 21_660_000;
/** Same launcher as `.mcp.json`: the verified helper, with the managed PATH first. */
const WAKE_SCRIPT =
  'export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"; ' +
  'exec "$HOME/.local/bin/cs-bus" --omp-wake --provider omp --session "$1" --host-pid "$2"';

function identityContext(session: string): string {
  return (
    `Codescribe identity for THIS OMP conversation: provider=${PROVIDER} session_id=${session}. ` +
    "For /codescribe NAME CHANNEL call codescribe_connect with exactly this provider and session_id. " +
    "After connecting and after each drained message, end the turn: the Codescribe OMP extension wakes this " +
    "conversation when the next message arrives. Read every message completely and ACK it before work."
  );
}

function connectRequest(
  session: string,
  name: string,
  channel: string,
  voice: string | undefined
): string {
  const voiceClause = voice ? ` voice=${voice}` : "";
  return (
    `/codescribe ${name} ${channel}${
      voice ? ` ${voice}` : ""
    }: call codescribe_connect with provider=${PROVIDER} ` +
    `session_id=${session} name=${name} channel=${channel}${voiceClause}. Use exactly this session id; never invent ` +
    "another. An occupied channel is not yours to take over. Report the attach receipt, then end the turn: " +
    "the Codescribe OMP extension wakes this conversation when a message arrives. Attachment is " +
    "attached_unverified until a fresh message is read, ACKed and answered."
  );
}

export default function codescribeOmp(pi: ExtensionAPI) {
  pi.setLabel("Codescribe");

  let session: string | undefined;
  let waiter: AbortController | undefined;

  function identify(ctx: ExtensionContext): string | undefined {
    const current = ctx.sessionManager.getSessionId();
    if (typeof current !== "string" || !current.trim()) {
      pi.logger.warn(
        "codescribe: OMP supplied no session id; not connecting and not arming the wake"
      );
      session = undefined;
      return undefined;
    }
    if (current !== session) {
      session = current;
      pi.sendMessage(
        {
          customType: "codescribe_identity",
          content: identityContext(current),
          display: false,
        },
        { deliverAs: "nextTurn" }
      );
    }
    return current;
  }

  function disarm(): void {
    waiter?.abort();
    waiter = undefined;
  }

  async function arm(): Promise<void> {
    const owned = session;
    if (!owned || waiter) return;
    const controller = new AbortController();
    waiter = controller;
    try {
      const result = await pi.exec(
        "/bin/sh",
        ["-c", WAKE_SCRIPT, "cs-bus", owned, String(process.pid)],
        {
          signal: controller.signal,
          timeout: WAKE_TIMEOUT_MS,
        }
      );
      if (waiter === controller) waiter = undefined;
      if (controller.signal.aborted || result.killed || owned !== session)
        return;
      if (result.code !== WAKE_EXIT) {
        // 0: this session owns no channel, another waiter holds the lock, or
        // the host pid is gone. 3: helper refused. Both stay silent until the
        // next turn end re-arms; a connect during that turn makes the next
        // arm bind.
        if (result.code !== 0) {
          pi.logger.warn("codescribe: wake helper refused", {
            code: result.code,
            stderr: result.stderr.trim(),
          });
        }
        return;
      }
      const notice =
        result.stderr.trim().split("\n").filter(Boolean).at(-1) ?? "";
      if (notice.startsWith("Codescribe: no message ")) {
        // Idle receipt after WAKE_SECONDS: re-arm without a model turn.
        void arm();
        return;
      }
      pi.sendMessage(
        {
          customType: "codescribe_mailbox_ready",
          content: notice,
          display: true,
        },
        { deliverAs: "followUp", triggerTurn: true }
      );
    } catch (error) {
      if (waiter === controller) waiter = undefined;
      if (!controller.signal.aborted) {
        pi.logger.warn("codescribe: wake helper failed", {
          error: String(error),
        });
      }
    }
  }

  pi.on("session_start", async (_event, ctx) => {
    disarm();
    if (identify(ctx)) void arm();
  });

  pi.on("session_switch", async (_event, ctx) => {
    disarm();
    if (identify(ctx)) void arm();
  });

  pi.on("agent_end", async (_event, ctx) => {
    if (identify(ctx)) void arm();
  });

  pi.on("session_shutdown", async () => {
    disarm();
  });

  pi.registerCommand("codescribe", {
    description:
      "Connect this OMP conversation to a Codescribe channel: /codescribe NAME CHANNEL [VOICE]",
    handler: async (args, ctx) => {
      const [name, channel, voice] = args.trim().split(/\s+/).filter(Boolean);
      if (!name || !channel) {
        ctx.ui.notify(
          "Usage: /codescribe NAME CHANNEL [VOICE] (channel 1-9)",
          "warning"
        );
        return;
      }
      if (!/^[1-9]$/.test(channel)) {
        ctx.ui.notify(
          `Codescribe channel must be a single digit 1-9, not ${channel}`,
          "warning"
        );
        return;
      }
      const owned = identify(ctx);
      if (!owned) {
        ctx.ui.notify(
          "Codescribe: this OMP session has no session id; cannot connect",
          "error"
        );
        return;
      }
      pi.sendUserMessage(connectRequest(owned, name, channel, voice), {
        attribution: "agent",
      });
    },
  });
}
