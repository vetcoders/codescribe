# Codescribe: kontekst projektu dla kolejnej sesji

Stan źródeł i inwentarza: 2026-10-28, baseline `d75e87dd4cdef9003b26e6e982b73e722a0d19d2`. To migawka, nie dowód aktualnego stanu aplikacji. Przed decyzją odśwież `git status -sb`, HEAD, artefakt, proces i logi. Relacje oznaczone poniżej nie są diagnozą przyczyny.

## Czym jest i gdzie jest właściciel

Codescribe to aplikacja macOS do dyktowania, z overlayem i kanałami agentów. Rust w `core/` obsługuje audio, rozpoznawanie i ledger; `app/` prowadzi kontroler, prezentację i dostarczanie; `bridge/` wystawia UniFFI, a `macos/` jest powłoką SwiftUI (`macos/Codescribe/App.swift:176`). Generowane bindings: `macos/Codescribe/Bridge/codescribe_ffi.swift`; zmiany API bridge wymagają `make app-bindings`, nie ręcznej edycji.

Ścieżka dyktowania, potwierdzona w źródłach baseline:

1. `RecordingController` (`app/controller/mod.rs:1854`) posiada mikrofon; `StreamingRecorder` (`core/audio/streaming_recorder.rs:693`) uruchamia sesję (`:1270`) i zamyka capture (`:1735`). Start/stop dostępny przez bridge (`bridge/src/hotkeys.rs:1195`, `:1320`).
2. Silero VAD pracuje na 16 kHz (`core/vad/mod.rs:1`, `:20`, `:43`); energia PCM i doliny VAD służą kwalifikacji wystąpień.
3. Wspólna sesja `core/pipeline/streaming/apple_live_session.rs:1` prowadzi zegar capture, opcjonalny progresywny Apple Live i obserwacje ledgera. `local_power` uzbraja lokalny Whisper jako Layer 1 / tail patch, zależnie od decyzji i ustawień (`:17`, `core/asr_session/mod.rs:219`). Apple-only nie ładuje Whisper automatycznie, a Apple może być wyłączony (`apple_live_session.rs:32`). Historyczne twierdzenie, że Apple Live jest standardem od miesięcy: **relacja; niezweryfikowane historycznie**.
4. `AcousticLedger` (`core/pipeline/acoustic_ledger.rs:886`) jest właścicielem tożsamości akustycznych wystąpień; obserwacje i zamknięcia stają się zdarzeniami dla `TranscriptReducer` (`app/presentation/emitter.rs:1110`), który jest właścicielem dokumentu. Overlay jest projekcją; bus obserwuje zdarzenia, nie ustanawia drugiej prawdy (`app/presentation/transcript_bus.rs`).
5. Dostarczenie zależy od jawnego celu: `DeliveryRoute` i `resolve_delivery_route` (`app/controller/delivery_route.rs:43`, `:258`), następnie paste/archiwizacja według decyzji. Nie zakładaj, że sam focus OS wybiera cel.

Kontrakty: [AGENTS.md](../AGENTS.md), [STT](STT_CONTRACT.md), [bus](TRANSCRIPT_BUS.md), [hotkeys](HOTKEYS_CONTRACT.md), [delivery](DELIVERY_ROUTE.md), [rejestr środowiska](ENV_REGISTRY.toml), [lokalizacja](LOCALIZATION.md). Konfiguracja przechodzi z `settings.json` przez `core/config/loader.rs` do snapshotu runtime.

## Mapa i ryzyko

`loct context --full --markdown` na czystym baseline: 1210 plików, 8 celów w ograniczonym packu; brak pełnego pokrycia. `loct repo-view` opisuje 3299 wewnętrznych krawędzi. Najważniejsze huby według Loctree (liczby wykrytych importerów, przybliżenie analizatora, nie kompletne dowody zależności):

| Plik                                 | Importerzy |
| ------------------------------------ | ---------: |
| `core/config/mod.rs`                 |         73 |
| `core/pipeline/contracts.rs`         |         51 |
| `core/stt/tail_provider.rs`          |         37 |
| `core/pipeline/acoustic_ledger.rs`   |         32 |
| `core/audio/capture_receipt.rs`      |         25 |
| `core/llm/provider.rs`               |         25 |
| `core/vad/mod.rs`                    |         15 |
| `app/presentation/transcript_bus.rs` |         13 |
| `core/pipeline/trail.rs`             |         10 |

`app/controller/mod.rs`, `core/pipeline/streaming/` i `app/presentation/emitter.rs` są dużymi powierzchniami współdzielonej ścieżki. Loctree zgłasza cykle i kandydatów na duplikaty/martwy kod, nie dowód że można je usuwać. Ma 116 nierozwiązanych re-eksportów w snapshotcie; przed refaktorem sprawdź `loct slice`, `impact`, realny runtime i testy.

## Wydania i identyfikacja

`make release-nightly` ustawia kanał nightly (`Makefile:1495`). `macos/project.yml:48` definiuje `CSBuildNumber`, `:49` `CSBuildCommit`; są to pola zbudowanego Info.plist. `diagnostic.log` odnotował uruchomienia `d75e87dd4` 2026-10-27/28, ale nie dowodzi aktualnie załadowanego procesu ani dystrybucji.

**Relacja, stan 2026-10-28:** build 2740 = `d75e87dd4`; poprzedni znany dobry 2723 = `6d95cdbda`. Dokładnego przypisania numerów do artefaktów oraz określenia „znany dobry” niezależnie nie potwierdzono. Git potwierdza w zakresie `6d95cdbda..d75e87dd4` zmiany continuous voice (`0958fc149`), Option hotkeys (`f799f564b`), composer mic sync (`879c20001`) i test first-Stop formattera (`5fd24ed3f`); to nie dowodzi akceptacji zainstalowanego runtime.

## Żywe śledztwa — relacja, stan 2026-10-28

- **Zwis na 2740:** dwa epizody około `00:23:52Z` i `01:12:26Z`; według relacji log urywa się po `whisper_encoder_invocation_finished` w `local-stt`, wątki milkną, brak crash-reportu, aplikacja pozostaje zombie. W logu potwierdzono wystąpienie tego zdarzenia o `01:12:26.680985Z`; całości obserwacji wątków, braku raportu i przyczyny nie potwierdzono. Koniec wpisów nie dowodzi, że Whisper spowodował zwis.
- **Apple Live off:** relacja, że przełącznik nie dociera do rdzenia i w logach nie zaobserwowano take'a bez Apple. Należy prześledzić ustawienie → snapshot → decyzję sesji i porównać świeży take; przyczyna nieustalona.
- **`take_live`:** relacja, że semafor potrafi blokować playback odpowiedzi po śmierci procesu appki. Zweryfikować właściciela, cykl życia i receipt przed jakąkolwiek naprawą; przyczyna nieustalona.

## Prawda runtime i bramki

Na tej maszynie `~/.codescribe/data.noindex/logs/codescribe.log` zawiera tracing rdzenia, `diagnostic.log` diagnostykę Swift. Take'i: `~/.codescribe/takes/`, sesje: `~/.codescribe/sessions/`, voice lab: `~/.codescribe/voice-lab/` — lokalizacje jako relacja, sprawdź konfigurację i istnienie przed użyciem. Dane mogą być prywatne. Zawsze rozdziel źródła, zbudowany artefakt, zainstalowaną aplikację, uruchomiony proces i faktyczne zachowanie UI.

Bramki repo: `make check` (format, Clippy, Semgrep, rejestr env, ledger), `make verify`, `make test-swift`, `make app-bindings`, `make verify-l10n-catalog`. Bezpośredni `cargo clippy --workspace --all-targets -- -D warnings` wymaga wspólnego absolutnego `CARGO_TARGET_DIR` i `CARGO_BUILD_BUILD_DIR` oraz uprzedniego `scripts/target-source-guard.sh "$CARGO_TARGET_DIR"`; patrz AGENTS.md. Testy i build nie dowodzą żywej ścieżki mikrofon → Stop → widoczne dostarczenie.

## Inwentarz integracyjny

Migawka poniżej pochodzi z lokalnego Git, 2026-10-28, względem `d75e87dd4`. Worktree to checkout, a `branch --no-merged` oznacza wyłącznie brak ancestry tipa względem baseline; nie oznacza automatycznie wartościowego niezintegrowanego kodu. Są też detached/prunable, historyczne, identyczne i już zawarte tipy. Przed decyzją zbadaj diff, ancestry, patch-equivalence i stan docelowego drzewa. Niczego nie usuwaj automatycznie.

Wbrew początkowej hipotezie, `fix/pr184-review-threads-261025` wskazuje `7d8cf2cc3` i **jest przodkiem** baseline (`git merge-base --is-ancestor` sukces); jego `perf(trail)` (#188) jest obecny w historii baseline. Nie traktuj go jako brakującej integracji. Lista lokalna nie dowodzi stanu zdalnych PR ani release.

Poniżej pełne wyniki komend; `+` przy branch oznacza checkout w worktree, nie status integracji.

### `git worktree list`

```text
/Volumes/vc-workspace/vetcoders/codescribe                                                                                                d75e87dd4 [integrate/fleet-6x9-local]
/private/tmp/claude-501/-Volumes-vc-workspace-vetcoders-codescribe/2818dc5d-e6e2-4726-beca-71203d508ab3/scratchpad/integ-gates/release-wt 262c987a2 (detached HEAD) prunable
/Users/polyversai/.vibecrafted/artifacts/vetcoders/codescribe/2026_1004/stack-117/release-source                                          d183dced3 (detached HEAD)
/Users/polyversai/.vibecrafted/artifacts/vetcoders/codescribe/2026_1005/reports/chat-native-blur/system-glass/verification-source         0eb26753f [verify/lena-system-glass-261005]
/Users/polyversai/.vibecrafted/artifacts/vetcoders/codescribe/2026_1014/lena-div0-app-1004-recovery/source-520c90b1                       9a71dd6b1 [test/lena-recorded-4804-prefix-20261014]
/Users/polyversai/.vibecrafted/worktrees/codescribe-pr-189                                                                                30410d9a0 [fix/pr184-review-threads-20261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/agy-work-261020-151652-27508                                      d5bd051b2 [cut/agy-work-261020-151652-27508]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/agy-work-261020-153856-37113                                      d5bd051b2 [cut/agy-work-261020-153856-37113]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/claude-work-261020-151539-30692                                   d5bd051b2 [cut/claude-work-261020-151539-30692]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/codex-audi-261020-183059-20133                                    a10f577fc [cut/codex-audi-261020-183059-20133]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/codex-work-261020-151557-98749                                    f1b675b6b [cut/codex-work-261020-151557-98749]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/cursor-work-261020-151710-12625                                   5d48c1bbc [cut/cursor-work-261020-151710-12625]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/cursor-work-261020-151729-10556                                   9f12d97b1 [cut/cursor-work-261020-151729-10556]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/grok-work-261020-151749-41304                                     cf944a7f4 [cut/grok-work-261020-151749-41304]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/klaudiusz-lab-pins-top                                            f55a7ab4e [cut/klaudiusz-lab-pins-top-261020]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/lena-admit-klaudiusz-261020                                       f70c47c28 [cut/lena-throne-rail-contract-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/lena-integrate-ledger-speech-truth                                642648c04 [integrate/lena-ledger-speech-truth-261020]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/omp-work-261020-151615-81976                                      d5bd051b2 [cut/omp-work-261020-151615-81976]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/omp-work-261020-151633-70153                                      9017e7821 [cut/omp-work-261020-151633-70153]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1020/omp-work-261020-153842-19457                                      d5bd051b2 [cut/omp-work-261020-153842-19457]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/claude-work-261021-235338-57225                                   a97d338cd [cut/claude-work-261021-235338-57225]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/cursor-work-261021-235224-47203                                   28e8869c6 [cut/cursor-work-261021-235224-47203]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/grok-just-261021-134426-83669                                     5ae1b9b0e [cut/grok-just-261021-134426-83669]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/klaudiusz-absence-proof                                           d09152c6a [cut/klaudiusz-emitter-evidence-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/klaudiusz-dictionary-recent-takes                                 3a95a38ee [cut/klaudiusz-dictionary-recent-takes-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/klaudiusz-explorek-docs                                           2c3fd5df4 [cut/klaudiusz-explorek-docs-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/klaudiusz-monika-roster-terminal                                  8b61e1d94 [integrate/klaudiusz-monika-roster-terminal-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/klaudiusz-recent-takes-truth                                      6c5f22ed3 [cut/klaudiusz-recent-takes-truth-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-admit-emitter-evidence                                       1ee219df0 [integrate/lena-emitter-evidence-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-bisect-core                                                  ac8e4d0c4 [cut/lena-silero-amplitude-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-c-on-absence                                                 ce3683bf5 [measure/lena-c-on-absence-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-complete-decode-pays                                         bbc05929f [cut/lena-complete-decode-pays-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-edge-word-waits                                              3239ad324 [cut/lena-edge-word-waits-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-guard-state-tmp                                              44cbb79bc [cut/lena-guard-state-tmp-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-integrate-absence-c                                          2f61940ba [integrate/lena-pcm-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-keychain-off-executor                                        9269f3978 [cut/lena-keychain-off-executor-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-monika-settings                                              005d7ca99 [cut/lena-install-stt-probe-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-preseries-51b40d31                                           51b40d31e (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-replay-sink-wait                                             0a701e87b [cut/lena-replay-sink-wait-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-retokenize                                                   3239ad324 [cut/lena-retokenize-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1021/lena-target-source-guard                                          4b2865c76 [cut/lena-target-source-guard-261021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/admit-cursor-grok                                                 d1664e928 [integrate/klaudiusz-cursor-grok-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/astra-transcript-seven-slots                                      ddb65cbf3 [cut/astra-transcript-seven-slots-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/bon-base                                                          d5bd051b2 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/bon-codex                                                         f1b675b6b (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/bon-cursor                                                        0428e994f (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/codex-work-261022-161033-40687                                    d3f0b0f50 [cut/codex-overlay-mouse-cost-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/codex-work-261022-202614-36698                                    d7d39558b [cut/codex-work-261022-202614-36698]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/klaudiusz-catalogue                                               1ee219df0 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/klaudiusz-monika-guard                                            4b3cfe7ce [integrate/klaudiusz-monika-guard-or-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/klaudiusz-witness-by-range                                        1ee219df0 [cut/klaudiusz-witness-by-range-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/lena-dt-lock-on-range                                             d2202f5cb [cut/lena-dt-lock-on-range-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/lena-horizon-park                                                 c49c7bc2e [cut/lena-horizon-park-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/lena-replay-bridge-tcc                                            6aea21f70 [cut/lena-replay-bridge-tcc-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/lena-seal-frontier                                                db8c44e3b [cut/lena-seal-frontier-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1022/lena-witness-lock                                                 73b47fe70 [cut/lena-witness-lock-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/codex-work-261023-132010-59694                                    dc963bec9 [cut/codex-work-261023-132010-59694]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/codex-work-261023-141656-46408                                    f486e2a98 [cut/codex-work-261023-141656-46408]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/codex-work-261023-151645-16880                                    535897d86 [cut/codex-work-261023-151645-16880]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/codex-work-261023-153505-59754                                    ec90f5a6d [cut/codex-work-261023-153505-59754]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-coarse-owner                                                 96976075a [cut/lena-coarse-owner-261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-composer-mic                                                 1f96644ea [cut/lena-composer-mic-261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-dt-after-tank                                                708c4d13b [cut/lena-dt-after-tank-261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-dt-base-126e73fe0                                            126e73fe0 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-dt-live-witness                                              8aee14fc5 [cut/lena-dt-live-witness-261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-dt-stop-witness                                              9e98197b8 [cut/lena-dt-stop-witness-root-261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1023/lena-formatter-store                                              34b5394a7 [cut/lena-formatter-store-261023]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1024/codex-rele-261024-133014-78699                                    8c92c3089 [cut/codex-rele-261024-133014-78699]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1024/lena-integrate-all                                                4442bb8d6 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1024/W-A1-l10n-gate                                                    b82dc5a47 [cut/W-A1-l10n-gate]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1024/W-C1-gates                                                        617d7d8d4 [cut/W-C1-gates]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1025/omp-pr184-threads                                                 0215d152c [chore/autopilot-setup-p1-p3]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-audi-261027-163513-10567                                      6d95cdbda [cut/agy-audi-261027-163513-10567]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152401-71223                                      6d95cdbda [cut/agy-work-261027-152401-71223]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152437-72760                                      6d95cdbda [cut/agy-work-261027-152437-72760]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152533-01526                                      6c7bf6994 [cut/agy-work-261027-152533-01526]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152604-72315                                      6d95cdbda [cut/agy-work-261027-152604-72315]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152724-45790                                      6d95cdbda [cut/agy-work-261027-152724-45790]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152801-81764                                      6d95cdbda [cut/agy-work-261027-152801-81764]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152842-51899                                      6d95cdbda [cut/agy-work-261027-152842-51899]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-152923-34926                                      6d95cdbda [cut/agy-work-261027-152923-34926]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153001-29481                                      6d95cdbda [cut/agy-work-261027-153001-29481]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153050-98743                                      6d95cdbda [cut/agy-work-261027-153050-98743]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153146-14192                                      6d95cdbda [cut/agy-work-261027-153146-14192]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153240-94835                                      6d95cdbda [cut/agy-work-261027-153240-94835]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153311-03997                                      6d95cdbda [cut/agy-work-261027-153311-03997]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153327-56054                                      6d95cdbda [cut/agy-work-261027-153327-56054]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153341-74045                                      6d95cdbda [cut/agy-work-261027-153341-74045]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153357-80517                                      28d2be35e [cut/agy-work-261027-153357-80517]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153413-88671                                      6d95cdbda [cut/agy-work-261027-153413-88671]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/agy-work-261027-153430-52158                                      6d95cdbda [cut/agy-work-261027-153430-52158]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/baseline-probe                                                    6d95cdbda (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-audi-261027-163351-43752                                   6d95cdbda [cut/claude-audi-261027-163351-43752]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-just-261027-195702-36889                                   ba001763d [cut/claude-just-261027-195702-36889]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-just-261027-195721-66461                                   ba001763d [cut/claude-just-261027-195721-66461]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-145830-99072                                   b7c41c6ee [cut/claude-work-261027-145830-99072]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-145840-98764                                   495395785 [cut/claude-work-261027-145840-98764]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-145851-03330                                   f32808a6c [cut/claude-work-261027-145851-03330]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-145901-94598                                   e157ea77f [cut/claude-work-261027-145901-94598]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151508-17898                                   33acab218 [cut/claude-work-261027-151508-17898]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151519-60222                                   6d95cdbda [cut/claude-work-261027-151519-60222]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151528-39780                                   6d95cdbda [cut/claude-work-261027-151528-39780]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151538-83989                                   515ec9a41 [cut/claude-work-261027-151538-83989]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151547-09240                                   c70d57e7c [cut/claude-work-261027-151547-09240]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151556-85486                                   6d95cdbda [cut/claude-work-261027-151556-85486]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151606-97502                                   711638288 [cut/claude-work-261027-151606-97502]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151616-66412                                   79a5a6cac [cut/claude-work-261027-151616-66412]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151625-04679                                   458987676 [cut/claude-work-261027-151625-04679]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151635-82021                                   b0e4ffef1 [cut/claude-work-261027-151635-82021]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151644-27551                                   affab3599 [cut/claude-work-261027-151644-27551]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151652-48809                                   49935da80 [cut/claude-work-261027-151652-48809]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151702-92570                                   6b6716329 [cut/claude-work-261027-151702-92570]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151711-16165                                   f4030cc92 [cut/claude-work-261027-151711-16165]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/claude-work-261027-151718-72221                                   f9a1636be [cut/claude-work-261027-151718-72221]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-audi-261027-163211-72834                                    6d95cdbda [cut/codex-audi-261027-163211-72834]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-just-261027-200048-63899                                    ba001763d [cut/codex-just-261027-200048-63899]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-just-261027-200114-41312                                    ba001763d [cut/codex-just-261027-200114-41312]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-just-261027-200129-85511                                    ba001763d [cut/codex-just-261027-200129-85511]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151733-04880                                    503f8b1f2 [cut/codex-work-261027-151733-04880]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151747-86707                                    6d95cdbda [cut/codex-work-261027-151747-86707]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151802-41733                                    6d95cdbda [cut/codex-work-261027-151802-41733]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151814-68690                                    6d95cdbda [cut/codex-work-261027-151814-68690]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151829-26864                                    6d95cdbda [cut/codex-work-261027-151829-26864]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151842-50603                                    6d95cdbda [cut/codex-work-261027-151842-50603]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151856-74263                                    fd8f6fa6d [cut/codex-work-261027-151856-74263]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151916-20467                                    6d95cdbda [cut/codex-work-261027-151916-20467]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151931-28406                                    75d07dc97 [cut/codex-work-261027-151931-28406]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-151948-95446                                    b0bc1fbaa [cut/codex-work-261027-151948-95446]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152003-61557                                    6d95cdbda [cut/codex-work-261027-152003-61557]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152022-07533                                    6d95cdbda [cut/codex-work-261027-152022-07533]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152054-90816                                    a30f6c65d [cut/codex-work-261027-152054-90816]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152131-83331                                    6d95cdbda [cut/codex-work-261027-152131-83331]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152155-15507                                    43f6b1c05 [cut/codex-work-261027-152155-15507]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152218-65114                                    5fd24ed3f [cut/codex-work-261027-152218-65114]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152239-67761                                    6d95cdbda [cut/codex-work-261027-152239-67761]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/codex-work-261027-152309-34234                                    6d95cdbda [cut/codex-work-261027-152309-34234]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1027/integrator-verify                                                 ba001763d (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1028/codex-just-261028-080947-56235                                    d75e87dd4 [cut/codex-just-261028-080947-56235]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1028/codex-just-261028-081038-66779                                    d75e87dd4 [cut/codex-just-261028-081038-66779]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/2026_1028/codex-just-261028-081050-31641                                    d75e87dd4 [cut/codex-just-261028-081050-31641]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-baseline-40f232f                                                       40f232f82 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-coarse-apple                                                           04fa8e1cc [cut/lena-coarse-apple-placeholder-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-format-probe                                                           ba11603fb [cut/lena-format-probe-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-integrate                                                              04fa8e1cc [cut/lena-integrate-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-measure-zero                                                           66a0e7b15 [cut/lena-measure-zero-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-playback-guard                                                         b5964b365 [cut/lena-playback-guard-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-reopen-order                                                           7cf535980 [cut/lena-reopen-order-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-wer-04fa8e1cc                                                          04fa8e1cc (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-wer-40f232f82                                                          40f232f82 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-wer-a17a66c8c                                                          a17a66c8c (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-wer-b7dd405a5                                                          b7dd405a5 (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-wer-eb5303e0a                                                          eb5303e0a (detached HEAD)
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-whisper-delay                                                          04fa8e1cc [cut/lena-replay-whisper-delay-261022]
/Users/polyversai/.vibecrafted/worktrees/vetcoders/codescribe/lena-window-position                                                        496893881 [cut/lena-window-position-261022]
/Volumes/vc-workspace/_integration/codescribe-wave2                                                                                       1fa17deeb [release/0.15.2-dragon]
/Volumes/vc-workspace/copilot-worktrees/codescribe/div0-space-curly-eureka                                                                663e63dbf [div0-space-pr-189-repairs]
/Volumes/vc-workspace/vetcoders/codescribe-fusion                                                                                         8f211a789 [fix/p0-p2-agy]
/Volumes/vc-workspace/vetcoders/codescribe-olek-live-stt-20261016                                                                         9fa2cd314 [fix/olek-live-stt-20261016]
/Volumes/vc-workspace/vetcoders/codescribe-release-88e1554d                                                                               88e1554dd (detached HEAD)
/Volumes/vc-workspace/vetcoders/codescribe-release-975ae4a2                                                                               975ae4a24 (detached HEAD)
/Volumes/vc-workspace/vetcoders/codescribe/.claude/worktrees/agent-a3238f5f2ce661e0e                                                      d76b7b963 [cut/klaudiusz-ledger-speech-truth-tests-261020]
/Volumes/vc-workspace/vetcoders/codescribe/.claude/worktrees/agent-abb922184fa78a667                                                      9f7ba2a6f [cut/klaudiusz-auto-fold-menu-261020]
/Volumes/vc-workspace/vetcoders/codescribe/.claude/worktrees/agent-af8d0cb1f0e10f295                                                      e5249166c [cut/klaudiusz-mini-cs-megaphone-261020]
```

### `git branch --no-merged HEAD`

```text
+ cut/claude-work-261027-145901-94598
+ cut/codex-work-261027-152155-15507
+ cut/claude-work-261027-145851-03330
+ cut/codex-work-261027-151931-28406
+ cut/claude-work-261027-151652-48809
+ cut/claude-work-261027-151616-66412
+ cut/claude-work-261027-151644-27551
+ cut/codex-work-261027-151948-95446
+ cut/claude-work-261027-151625-04679
+ cut/agy-work-261027-153357-80517
+ cut/codex-work-261027-151733-04880
+ cut/claude-work-261027-151508-17898
+ cut/claude-work-261027-151718-72221
+ cut/codex-work-261027-151856-74263
+ cut/claude-work-261027-151538-83989
+ cut/codex-work-261027-152054-90816
+ cut/claude-work-261027-151702-92570
+ cut/claude-work-261027-151711-16165
+ cut/claude-work-261027-151547-09240
+ cut/agy-work-261027-152533-01526
+ cut/claude-work-261027-145840-98764
+ chore/autopilot-setup-p1-p3
+ cut/codex-rele-261024-133014-78699
+ cut/lena-dt-stop-witness-root-261023
+ cut/lena-coarse-owner-261023
+ cut/lena-dt-live-witness-261023
  cut/lena-dt-stop-witness-261023
+ cut/codex-work-261023-153505-59754
+ cut/codex-work-261023-151645-16880
+ cut/codex-work-261023-141656-46408
  release/0.18.0-cohort-plus-moni
+ cut/codex-work-261022-202614-36698
+ cut/lena-reopen-order-261022
+ cut/lena-playback-guard-261022
+ cut/lena-format-probe-261022
+ cut/astra-transcript-seven-slots-261022
+ cut/codex-overlay-mouse-cost-261022
+ cut/lena-coarse-apple-placeholder-261022
+ cut/lena-integrate-261022
+ cut/lena-replay-whisper-delay-261022
  embargo/astra-transcript-seven-slots-261022
+ cut/lena-measure-zero-261022
+ cut/lena-window-position-261022
+ integrate/klaudiusz-cursor-grok-261022
+ cut/lena-dt-lock-on-range-261022
+ cut/lena-horizon-park-261022
+ cut/lena-replay-bridge-tcc-261022
+ cut/lena-witness-lock-261022
+ integrate/klaudiusz-monika-guard-or-261022
  integrate/klaudiusz-monika-mini-261022
  integrate/klaudiusz-monika-guard-261022
+ cut/cursor-work-261021-235224-47203
  cut/klaudiusz-guard-room-floor-261022
+ cut/lena-seal-frontier-261022
+ cut/claude-work-261021-235338-57225
+ cut/klaudiusz-witness-by-range-261022
+ integrate/lena-emitter-evidence-261021
+ cut/lena-guard-state-tmp-261021
+ cut/klaudiusz-emitter-evidence-261021
+ cut/lena-replay-sink-wait-261021
+ integrate/lena-pcm-261021
  cut/klaudiusz-disjoint-neighbours-261021
  integrate/lena-absence-c-261021
  cut/klaudiusz-cross-owner-overlap-261021
  cut/klaudiusz-mutual-majority-261021
  cut/klaudiusz-continuation-261021
  cut/klaudiusz-absence-proof-261021
+ cut/lena-keychain-off-executor-261021
+ cut/lena-edge-word-waits-261021
+ cut/lena-retokenize-261021
+ cut/grok-just-261021-134426-83669
+ measure/lena-c-on-absence-261021
+ cut/lena-target-source-guard-261021
+ cut/lena-complete-decode-pays-261021
+ cut/klaudiusz-explorek-docs-261021
+ cut/klaudiusz-recent-takes-truth-261021
+ integrate/klaudiusz-monika-roster-terminal-261021
  cut/lena-rpath-guard-261021
+ cut/lena-silero-amplitude-261021
  cut/lena-pcm-admission-261021
+ cut/lena-install-stt-probe-261021
+ cut/lena-throne-rail-contract-261021
+ cut/klaudiusz-dictionary-recent-takes-261021
  integrate/lena-admit-klaudiusz-261020
+ cut/klaudiusz-lab-pins-top-261020
  cut/grok-work-261020-200236-61372
+ integrate/lena-ledger-speech-truth-261020
  cut/klaudiusz-vc-frame-tab-identity-261020
+ cut/cursor-work-261020-151729-10556
+ cut/codex-work-261020-151557-98749
+ cut/grok-work-261020-151749-41304
+ cut/cursor-work-261020-151710-12625
+ cut/omp-work-261020-151633-70153
+ cut/klaudiusz-ledger-speech-truth-tests-261020
+ cut/codex-audi-261020-183059-20133
+ cut/klaudiusz-mini-cs-megaphone-261020
+ cut/agy-work-261020-151652-27508
+ cut/agy-work-261020-153856-37113
+ cut/claude-work-261020-151539-30692
+ cut/omp-work-261020-151615-81976
+ cut/omp-work-261020-153842-19457
+ cut/klaudiusz-auto-fold-menu-261020
  worktree-agent-af8d0cb1f0e10f295
  release/0.17.2
  release/0.17.0
+ fix/olek-live-stt-20261016
+ test/lena-recorded-4804-prefix-20261014
  release/astra-standard-017-20261011
+ verify/lena-system-glass-261005
  release/0.15.3-agent-surfaces
+ release/0.15.2-dragon
  release/0.15.2-dragon-roman
  release/0.15.2-dragon-old
+ fix/p0-p2-agy
```
