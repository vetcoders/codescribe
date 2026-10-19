#!/usr/bin/env zsh
# Replay acceptance for "no speech detected" on a take every observer heard as
# silence, paired with a take that had speech and must never read as silence.
#
# Before the measured-silence cut the silent take ended in coverage_refused,
# which the overlay showed as "Transcription failed". It now ends as observed
# silence: Silero heard nothing and each Whisper window's VAD selected nothing.
#
#   silent take  09ededb1-7884-4b71-81f8-8cd0fc12a475 (19 X 09:36 CEST, 18 s)
#                capture_energy found 10-11 crumbs of -70 dB noise; tail VAD
#                selected 0 samples in all 5 windows; Apple segments 0.
#                Expect: no words, and no terminal_seal_coverage_incomplete.
#   speech take  agent-channel-0-24153f32-c233-4787-b8df-5eedc61f5050
#                ("Jest.", 19 X 09:55 CEST, 8.8 s, 16 spoken words).
#                Expect: words, whatever the coverage receipt says.
#
# The takes are the Founder's recordings and stay in his data directory; the
# script copies only their cards into a scratch directory and links the WAVs.
# Usage: no_speech_heard.sh [codescribe binary]   (default: codescribe on PATH)
set -u
BIN=${1:-codescribe}
SRC=${CODESCRIBE_SOURCE_DATA:-$HOME/.codescribe}
SILENT=09ededb1-7884-4b71-81f8-8cd0fc12a475
SPEECH=agent-channel-0-24153f32-c233-4787-b8df-5eedc61f5050
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/no-speech-heard.XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT

for take in $SILENT $SPEECH; do
  for ext in wav replay-card.json; do
    if [[ ! -e $SRC/sessions/$take.$ext ]]; then
      echo "SKIP: $SRC/sessions/$take.$ext is not on this host"
      exit 0
    fi
  done
done

mkdir -p $SCRATCH/data/sessions
for x in models data_assets assets prompts lexicon.custom.jsonl; do
  [[ -e $SRC/$x ]] && ln -s $SRC/$x $SCRATCH/data/$x
done
for take in $SILENT $SPEECH; do
  ln -s $SRC/sessions/$take.wav $SCRATCH/data/sessions/$take.wav
  cp $SRC/sessions/$take.replay-card.json $SCRATCH/data/sessions/
done
: > $SCRATCH/empty.env

replay() {
  CODESCRIBE_DATA_DIR=$SCRATCH/data CODESCRIBE_ENV_PATH=$SCRATCH/empty.env \
    $BIN replay $1 --json 2> $SCRATCH/$1.err
}

replay $SILENT > $SCRATCH/silent.json || { echo "FAIL: replay of $SILENT exited $?"; exit 1; }
replay $SPEECH > $SCRATCH/speech.json || { echo "FAIL: replay of $SPEECH exited $?"; exit 1; }

python3 - $SCRATCH/silent.json $SCRATCH/speech.json <<'PY'
import json, sys

silent = json.load(open(sys.argv[1]))["runs"][0]
speech = json.load(open(sys.argv[2]))["runs"][0]
failures = []
if silent["text"].strip():
    failures.append(f"silent take produced words: {silent['text']!r}")
if "terminal_seal_coverage_incomplete" in silent["warning_codes"]:
    failures.append("silent take still ends in refused coverage "
                    "(overlay: Transcription failed), expected measured silence")
if not speech["text"].strip():
    failures.append("speech take lost every word; it must never read as silence")
for line in failures:
    print("FAIL:", line)
print("silent:", silent["warning_codes"])
print("speech:", len(speech["text"].split()), "words:", speech["text"])
sys.exit(1 if failures else 0)
PY
