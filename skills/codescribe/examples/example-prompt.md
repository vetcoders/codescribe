# codescribe — example trigger

## Trigger phrase

> Hej James, wpinasz się w bus. Jak chcesz się nazywać?

## Expected agent behavior

1. Check Codescribe.app is up and `~/.codescribe/transcript-events.jsonl` exists.
   If not: _Stary, odpal apkę i licencję._ Fail loud after retry.
2. Start the installed helper with the client token, stable provider session id,
   `--become --drafts --follow`; preserve and poll its handle.
3. Answer the name question (e.g. James). Greet in **this** chat.
4. Bind the same provider session with `--name james --drafts --follow`.
5. On Hold Fn, reply in the ~5 s gap when the utterance addresses James.
   Side effects only after `transcript_sealed`.

## Acceptance evidence

- A greeting in the agent chat, not in the overlay
- `bus-demux` stdout line with `"audience": "james"` on a named seal
- Attach receipt with provider/session/lease and `resumed: true` after a recovery probe
- No Voice Lab, no second recorder, no `vibecrafted codescribe`

## Notes

Saying **James** (or the bound stem) at the start of an utterance is the mailbox
stamp — one word, not a litany. First _Hej James_ in a hold may bind the rest
of that hold until Fn up.
