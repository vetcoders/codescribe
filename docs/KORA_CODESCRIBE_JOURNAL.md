# Kora Codescribe source journal

Status: append-only extraction journal.

Primary source: `/Users/maciejgad/Downloads/Kora_codescribe.md` (2,789 lines, read sequentially in 400-line blocks on 2026-08-21).

Purpose: preserve decisions, corrections, runtime evidence, superseded claims, and open risks before distilling them into `KORA_CODESCRIBE_CONTRACT.md`. This journal is evidence, not executable product authority. Later statements and explicit operator corrections supersede earlier interpretations.

Extraction labels:

- `OPERATOR` — explicit product intent or correction from Maciej.
- `FACT` — observation grounded in code, runtime, logs, or artifacts in the source conversation.
- `DECISION` — an adopted implementation or configuration choice.
- `SUPERSEDED` — a statement later corrected or invalidated.
- `RISK` — unresolved failure mode or uncertainty.
- `FOLLOW-UP` — work promised or required after the source block.

## Source lines 1–400

- `FACT` The initial heat investigation was explicitly read-only. A 15.9-second sample did not include a running Codescribe process, so it could not attribute heat to Codescribe.
- `FACT` The dominant sampled process was `voc --view observe`, averaging about 41.8% of one CPU core; its stack repeatedly rebuilt control-plane state and spawned PID-liveness checks.
- `RISK` Physical heat was not measured directly; CPU samples do not establish chassis temperature, fan speed, GPU use, or causality.
- `SUPERSEDED` Kora first inferred that the user had returned to an ordinary application text field. A direct terminal command proved the session was a forked terminal Codex session. The corrected interpretation supersedes the UI inference.
- `OPERATOR` The CodeScribe priority was runtime truth around two Whisper cache entries: retired Q8 versus intended large-v3-turbo FP16/F32 runtime, with Q8 dequantization suspected as a large cold-start cost.
- `FACT` Cache presence is not runtime selection. The resolver, bundle completeness, tensor format, conversion path, and engine lifetime determine cost.
- `FACT` The intended FP16 snapshot contained `config.json` and `weights.safetensors` but lacked `tokenizer.json` and `mel_filters.npz`; the old Q8 repository still supplied companion assets and retained a complete orphaned Q8 snapshot.
- `FACT` The then-current resolver scanned snapshots and could select the complete Q8 snapshot when the FP16 snapshot was incomplete.
- `FACT` A runtime log proved this was not theoretical: a Q8 load took 9.53 seconds, of which 9.28 seconds was `dequantize_q8`.
- `FACT` The installed application was older than the repository commit intended to forbid local weights outside Local Power.
- `FACT` At that time, normal Apple/cloud Hold Fn runs did not load Whisper. The Q8 risk applied to local Layer 1 or explicit HQ/Retranscribe paths.
- `DECISION` The correct model bundle shape was identified as FP16 config/weights plus trustworthy tokenizer and mel assets in one complete composed directory.
- `DECISION` Resolver and runtime gates must never select Q8 weights once the product decision retires Q8.
- `FOLLOW-UP` Add a fixture for incomplete FP16 + companion assets + orphaned complete Q8, expecting FP16 composition/selection and zero Q8 fallback.
- `FOLLOW-UP` Runtime evidence must show the intended model identity and `dequantize_q8 = 0`.
- `RISK` The exact claimed 7.5x whole cold-start advantage was not found in the inspected repo evidence; the direction was supported, while exact ratios depended on which phase was compared.
- `FOLLOW-UP` For `voc`, measure refresh phases and helper spawn counts, eliminate subprocess-per-PID checks, cache terminal runs and unchanged derived views, add backoff/event-driven refresh, and target idle observe below 2% of one core.

## Source lines 401–800

- `FACT` Three unrelated mechanisms had overlapping names: stop-path `FINAL_PASS_MODE=smart`, LLM Smart formatting, and planned Layer 4 Final BAM. They must never be treated as one feature.
- `FACT` Hated Full Final Pass (`always`) redecoded the whole saved WAV after release and could replace the whole transcript. Its product failures were latency, cold-load cost, authority without span provenance, and the ability to destroy a good live result.
- `FACT` Smart stop routing classified live output as complete, shape-deficient, or incomplete; it could skip, transplant punctuation/capitalization, or fill only a missing tail. It was not Layered live refinement.
- `DECISION` `off` means no Whisper inference on the normal stop path. It does not disable Apple live, lexicon, Layered live refinement, formatting, or explicit Retranscribe.
- `FACT` The source observed a macOS UI that forced final pass to `off` and retained old routing tokens for compatibility/tests.
- `CURRENT-CODE` On HEAD `361ece40`, `FinalPassRoutingMode` still contains `Always`, `Smart`, and `Off`; `Always` alone permits whole-file repass, `Smart` is documented as per-utterance tail gap-fill, and `Off` forbids Whisper on stop paths. `SettingsViewModel.finalPassModeId` still returns literal `off`.
- `DECISION` Normal capture must finish from the evolving live transcript. Whole-file inference belongs only to an explicit Retranscribe/HQ action.
- `OPERATOR` Layered is the core product advantage: several imperfect observers update one time-grounded canvas without granting any observer authority to rewrite the session from scratch.
- `DECISION` Intended layers: Apple fast hypotheses; Whisper/cloud recall and correction; lexicon/small LLM domain shaping; paralingual events; bounded Final BAM.
- `FACT` The source distinguished two different Layer 1 implementations: legacy local Whisper tail patch and a provider/cloud PCM fan-out lane. Naming them both Layer 1 hid incompatible semantics.
- `FACT` Legacy local tail patch was gated by `CODESCRIBE_LAYERED_TRANSCRIPTION=phase1` and Local Power. It transcribed an utterance PCM window, compared Apple and Whisper, and attempted bounded replacement or gap insertion.
- `FACT` The provider/cloud lane had bounded queues, no capture backpressure, volatile partials, ordering/idempotence tracking, typed degradation, and bounded stop drain, but its merge policy conservatively preserved Apple on ordinary disagreement.
- `SUPERSEDED` The source temporarily treated Apple-only as a complete product baseline and Layered `off` as the safe default. This was explicitly contingent on defects listed later, not the target product contract.
- `RISK` The old patcher sometimes compared transcripts produced from different PCM ranges because of VAD shifts, inaccurate clocks, mid-phrase cuts, or committed-text/audio mismatch.
- `RISK` Apple word timestamps could be lost before the reducer; seals lacked complete `[start,end)` word payloads; some mutation paths bypassed the intended rewrite fence.
- `RISK` Text-similarity safety gates rejected precisely the high-value cases where Whisper recovered more speech. Historical evidence included 116 skipped/0 applied and 295 change-ratio rejections, many with longer Whisper candidates.
- `DECISION` The remedy is stronger audio/span identity, not merely a looser textual change-ratio threshold.
- `RISK` Text-based gap dedup can either duplicate later Apple delivery or delete intentional repetitions. Content equality is not span identity.
- `RISK` Layer 1 work could be abandoned after the Apple seal worker closed. A completed product must bounded-drain admitted work or emit explicit degradation.
- `CURRENT-CODE` HEAD still counts `abandoned_tail_patch_jobs`, emits a warning, reports drain degradation, and abandons queued/in-flight work after the Apple worker closes. This remains an explicit product degradation, not a completed patch guarantee.
- `CURRENT-CODE` HEAD still defaults Layered to absent/off through `layered_phase_from_raw(None) -> None`; the local patch lane is therefore opt-in despite the operator's target architecture.

## Source lines 801–1200

- `DECISION` Turning off hated whole-file Final Pass is a consequence of Layered, not a retreat from it. Final BAM must apply bounded ledger-aware corrections, never rebuild the transcript without provenance.
- `DECISION` The temporary `off` may be removed only when word/span identity, exact PCM range, one rewrite fence, structural idempotence, intentional repetition preservation, bounded stop drain, provider-semantic parity, and operator-visible receipts are real.
- `OPERATOR` Audio and its timeline are truth. Apple produces fast imperfect hypotheses pinned to time. Whisper observes overlapping windows of about four seconds with about one second of overlap. The canvas is a live projection, not immutable Apple text.
- `OPERATOR` Apple output such as `stwierdzić` is a temporal/phonetic pin, not a protected token. Whisper may replace it with `stwierdził` when the observation belongs to the same span identity.
- `DECISION` The safety law is: no canvas mutation without proof of the audio span to which the new observation belongs. It is not: never change an Apple token.
- `ANTI-MUTATION` Apple temporal pins must not become an immutable floor.
- `ANTI-MUTATION` Preserve speech content must not become preserve literal overlay characters.
- `ANTI-MUTATION` Correct the proper span must not become accept only a small textual diff.
- `ANTI-MUTATION` Audio identity must not be replaced by token similarity.
- `ANTI-MUTATION` Preserve intentional repetitions as separate spans must not become global no-delete or text deduplication.
- `ANTI-MUTATION` Overlapping Whisper windows must not become isolated whole-utterance string comparison.
- `ANTI-MUTATION` A living canvas must not become immutable after seal.
- `ANTI-MUTATION` Layer 0 as first observer must not become semantic authority.
- `OPERATOR` Agent-authored heuristics must never be relabeled as `operator law` or `operator decision`. Tests and comments need evidence provenance and an explicit supersession trail.
- `DECISION` True invariants are timeline continuity, audio-span identity, observation provenance, no uncovered audio gaps, no cross-span text mixing, replay identity distinct from intentional repetition, and reconstructable mutation receipts.
- `FACT` The source's final preflight identified a settings dual-brain: canonical Application Support settings lacked STT choices while `.env` and an older config carried them.
- `OPERATOR` Q8 was explicitly forbidden from runtime, including explicit custom paths. This was a product decision, not merely a resolver preference.
- `DECISION` FP16 must be a complete verified bundle. Tokenizer must come from official `openai/whisper-large-v3-turbo`; mel filters from a pinned `openai/whisper` revision; no runtime assets may depend on the retired Q8 repository.
- `CURRENT-CODE` HEAD contains `request_identity` and `span_map` throughout the Apple progressive tail-patch path, and no `dequantize_q8` or `LEGACY_WHISPER_REPO` implementation remains.
- `CURRENT-CODE` HEAD does not contain the literal phrase `immutable floor`, but textual conservatism must be audited semantically; deleting the phrase alone does not restore the product model.
- `CURRENT-CODE` The implementation and docs still admit incomplete closure: `core/pipeline/streaming/session.rs` reports that the VAD/scheduler path has no exact pending-span rewrite fence and preserves primary text; comments/docs describe exact PCM identity and one rewrite fence only for Apple progressive.
- `CURRENT-CODE` `docs/ENV_REGISTRY.toml` still says to keep Layered off until PCM/span identity, one rewrite fence, structural idempotence, and bounded-drain evidence cover every accepted patch. That condition is not satisfied across every live path on current HEAD.
- `CORRECTION` Therefore the later exact-identity cut materially advanced the Apple progressive path but did not, by itself, justify claiming the entire Layered product contract complete or universally safe-by-default.

## Source lines 1201–1600

- `DECISION` Kora persisted the then-current Silver profile as Apple live + cloud ASR, Layered off, Final off, Polish, and explicit `whisper-large-v3-turbo`. This was a host-specific safe profile before the later Layered repair, not the timeless product target.
- `FACT` The composed FP16 bundle was exercised through a real loader and a real transcription, producing 2,432 characters; a real Q8 snapshot was rejected before tensor load/dequantization.
- `DECISION` Q8 refusal must exist at config validation, safetensors header validation, resolver selection, direct engine load, and embedded payload selection. Dead Q8 dequantization code must be removed to avoid future resurrection.
- `FACT` Installation verification distinguished signed files on disk from the actually running process. A relaunch was necessary to prove the new binary, not merely the replaced app bundle.
- `DECISION` A rescue push may preserve committed work even when a PR gate is red; mergeability and publication remain separately gated.
- `FACT` A many-month rescue branch produced 37 conflicts against current `develop`. The safe solution was a fresh branch from current `develop` plus the focused verified commit, not merging all of `develop` into the historical branch.
- `FACT` PR #81 eventually reached green CI, zero unresolved threads, and mergeability after focused fixes, while remaining a draft and unmerged.
- `FACT` Review correctly found residual bundle-integrity problems: platform-specific checksum tooling, corrupt mel retained as apparently complete, unconditional mel re-fetch, missing direct loader fixture, stale diagnostics, and ambiguous artifact errors.
- `RISK` Green discovery tests do not prove direct engine refusal; both discovery and direct loader paths require negative fixtures.
- `RISK` A checksum verifier that only returns an error can leave a corrupt destination. Download/repair must use temporary files and promote only after validation, or explicitly quarantine/remove invalid final files.
- `RISK` File-name presence is never model completeness. Config, tokenizer, pinned mel checksum, weights structure, dtype allowlist, offsets, payload length, and metadata schema belong to one validator.
- `CURRENT-CODE` HEAD now has a single `validate_whisper_model_bundle` that parses config and tokenizer, checks the pinned mel SHA-256, resolves a valid weights file, and validates safetensors.
- `CURRENT-CODE` `resolve_runtime_whisper_model_path` checks the complete validator for explicit path, configured model, default local bundle, and HF snapshots; its terminal diagnostic explicitly refuses quantized Q8.
- `CURRENT-CODE` `verify_sha256` still only reports mismatch; safety depends on callers using `.partial`/repair discipline. This must be checked at download call sites rather than inferred from the helper alone.
- `CURRENT-CODE` No runtime `dequantize_q8` implementation remains on HEAD. The earlier source note that dead dequantizer code remained is superseded by later PR work and current code.
- `RISK` A host build must not be installed merely to satisfy a ritual if its base would downgrade unrelated product work. Code/loader smoke and installed-app truth must be reported separately.

## Source lines 1601–2000

- `DECISION` Before merge, model download needed cross-platform `shasum`/`sha256sum`, invalid-destination cleanup, reuse of valid mel, direct loader rejection tests, corrected FP16 terminology, and host evidence for real FP16 load/Q8 refusal.
- `DECISION` A self-attack must name falsifiers. Example: corrupt nonempty mel plus otherwise valid files must not make the next download return success without repair.
- `FACT` A second review broadened the corruption finding: invalid config, tokenizer, mel, or weights could persist behind filename-only completeness and skip-if-nonempty behavior.
- `DECISION` `fp16 only` must be a positive validator, not a blacklist of known Q8 signals. It requires at least one tensor, dtype allowlisting, metadata-schema validation, consistent shapes, byte sizes, offsets, no gaps/overlaps, and exact payload length.
- `DECISION` The official model's `alignment_heads:I64` is a narrow named exception; arbitrary integer tensors remain forbidden.
- `DECISION` All downloaded/copied artifacts use `.partial`, validate before promotion, repair/quarantine invalid destinations, and reuse valid existing files offline.
- `FACT` The focused implementation added one bundle validator shared by discovery/status/download, parsed tokenizer, pinned mel SHA, structural safetensors checks, loader-level U32/I32 refusal, corrupt-cache repair, and offline reuse tests.
- `FACT` Runtime verification included real official FP16 load and real Q8 refusal before tensor load; hermetic tests covered U32, I32, metadata-only, truncation, corrupt mel, retry, and reuse.
- `RISK` Structural validation cannot detect a same-length payload bit flip without a pinned full-weights hash. Supporting custom model paths makes a universal official-blob hash a separate product decision.
- `DECISION` A failure caused by a newer CI Clippy is still a real gate failure when introduced by the diff. Fix the source compatibility and rerun; never call it flaky merely because local Clippy is older.
- `FACT` Separate follow-up findings existed outside PR #81: auto-paste could send Cmd+V without positive target confirmation; bus demux diverged from the canonical bus-path resolver; loose greeting regex could assign accidental agent names; UTF-8 split across polls could produce a negative offset.
- `DECISION` These follow-up findings must not be mixed into model-bundle work merely because they share a branch. Scope ownership remains explicit.
- `CURRENT-CODE` HEAD's model validator matches the positive-validation shape: config/tokenizer/pinned mel plus structural weights validation. Current resolver uses it across explicit, configured, default, and HF candidates.
- `CURRENT-CODE` Current `verify_sha256` is only a validator; correct repair behavior must remain enforced and tested in downloader/copy callers.
- `RISK` Repo-wide `make check` can be red because its scope includes unchanged historical artifacts. Changed-file formatting and full hermetic tests must be reported distinctly; neither erases the repo-wide red gate.
- `DECISION` Loctree build/version drift must be reported and direct code checked when structural snapshot authority is stale; tool drift does not justify skipping structural mapping.

## Source lines 2001–2400

- `FACT` After the repair, PR #81 reported one positive bundle validator, direct loader refusal, `.partial` promotion, invalid-cache repair, valid-mel reuse, cross-platform checksum tooling, corrected diagnostics, and green local/remote gates.
- `DECISION` A pull request is not done merely because CI is green. It must also be mergeable, conflict-free, and have every review thread addressed with code/evidence or a justified rejection.
- `DECISION` Do not merge merely because a PR was flipped from draft to Ready. Bot/reviewer waves are part of the expected lifecycle.
- `FACT` Later bot review found additional plausible defects even after green CI: invalid first weights file shadowing a valid alternate, newest invalid HF snapshot shadowing an older valid one, shell preflight using filename-only completeness, missing safetensors metadata validation, and documentation/runtime default drift.
- `RISK` Suppressed suggestions can contain real product failures and require falsification, not automatic dismissal: zero-element tensors, local paths mistaken for HF IDs, shallow test resolvers, and ambiguous downloader stdout.
- `DECISION` One review thread should normally map to one focused commit unless multiple threads share the same owner/root cause. Replies must cite the fixing SHA after verification.
- `DECISION` Cross-platform differences must be reasoned about before push whenever process, filesystem, signal, locking, or timing behavior changes.
- `DECISION` CI failure must be diagnosed from the failing step/log before retry. Platform-specific failure is evidence, not inconvenience.
- `CURRENT-CODE` Current resolver calls `is_complete_whisper_model_dir` for candidates and current bundle validation is deep. The later HF and dual-weights selection behavior still needs direct inspection before claiming every bot thread remains closed on this branch.
- `RISK` Documentation default drift is particularly dangerous here because `Final Pass off` and `Layered off` have different meanings, while old `smart` tokens can survive in persisted settings. UI, settings serialization, env registry, and runtime logs must agree.
- `DECISION` GitHub presentation is part of product truth: PR bodies must contain real Markdown newlines, not escaped `\\n`, and state changes must be re-read from the API.

## Source lines 2401–2789

- `DECISION` Review replies are short, cite a verified SHA, explain the exact fix, and resolve the thread only after the code and gates support the claim.
- `FACT` Read-only audit confirmed three strong resolver defects: first-existing weights shadowing a valid alternate; newest invalid HF snapshot shadowing older valid snapshots; malformed safetensors `__metadata__` passing discovery but failing the upstream loader.
- `DECISION` Weights selection must choose the first validated candidate in deterministic name priority. Discovery and engine must share that resolver.
- `DECISION` HF resolution preserves cache-root priority, then checks snapshots newest-to-oldest until a full validator accepts one. Invalid candidates must not shadow valid candidates.
- `DECISION` Safetensors metadata must match the upstream schema: absent or accepted null, or an object with string values; numeric/array/non-string metadata is invalid.
- `FACT` Additional confirmed defects included zero-element tensors, shallow shell early exits, local paths mistaken for HF repository IDs, tokenizer-only E2E resolution, and dead resolver helpers.
- `DECISION` Shell/release/setup paths must invoke the same Rust validator as runtime; duplicating file lists in Bash recreates split-brain completeness.
- `RISK` Direct Cargo embedding could still bypass canonical Make/release preflight through a shallower `core/build.rs` filename-only check. Canonical targets being safe does not make the bypass correct.
- `DECISION` `final_pass_mode=smart` persisted as a migration/runtime token in the audited branch, while the normal product stop path remained hard-off. Documentation must state both facts without implying that Smart whole-file inference runs after every take.
- `FACT` The source reports focused commits for the resolver, HF predicate, metadata schema, zero-element rejection, shell validation, and E2E cleanup, followed by full gates and push.
- `CURRENT-CODE` Current HEAD does not contain those final resolver shapes: `resolve_weights_path` still chooses the first existing file without validating alternates.
- `CURRENT-CODE` Current HEAD has no `find_snapshot_with_any_matching` symbol, so the predicate-aware HF traversal described in the source is absent or named/implemented differently and requires direct follow-up.
- `CURRENT-CODE` Current HEAD still filters `__metadata__` out of tensor iteration without an evident schema check; the source's later metadata fix is not present in this checkout.
- `CURRENT-CODE` Therefore historical green PR state must not be projected onto the current branch. The current code is authoritative and presently re-exposes at least the alternate-weights and metadata risks.
- `FOLLOW-UP` Before claiming PR #81 follow-up is integrated, compare current HEAD against `7b85f718` (or the final remote PR head), then port/verify missing validated-weights, predicate-aware HF, metadata-schema, zero-element, and shell preflight fixes.
- `FOLLOW-UP` After any port, rerun targeted bundle/loader fixtures, `make verify`, Clippy, Semgrep, changed-doc formatting, real FP16 load, real Q8 refusal, and current-branch runtime smoke.

## Journal synthesis

- The source conversation contains intentional evolution: early safe-off statements were later narrowed by explicit operator correction.
- The stable product north star is time-grounded multi-observer transcription, not immutable Apple text.
- The stable model policy is verified FP16 and absolute Q8 refusal.
- The stable normal-stop policy is no hidden whole-file repass; explicit Retranscribe owns whole-file inference.
- The stable engineering policy is one validator/owner per truth, runtime receipts, real artifact smoke, and no historical-green projection onto a living branch.
- The largest current gap is not lack of intent. It is incomplete convergence between that intent, the current Layered runtime, current settings truth, and current branch integration of the final PR #81 follow-ups.
