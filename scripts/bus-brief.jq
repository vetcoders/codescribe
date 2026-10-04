# One-line projection of Transcript Bus events for a human eye.
#
#   tail -F ~/.codescribe/transcript-events.jsonl | jq -rn --unbuffered -f scripts/bus-brief.jq
#
# Every reducer revision fans out one Bus line per document entry, each carrying
# the whole rendered document. By default only the first line of such a burst
# is printed (BUS_ALL=1 prints every line). Sample counts are converted with the
# rate carried in the receipt's calibration id, never a hard-coded rate.

def hz:
  (.acoustic_receipts // [] | .[0].evidence_calibration_version // "")
  | (capture("@(?<hz>[0-9]+)hz")? // {hz: "88200"}).hz | tonumber;

def secs($hz): if . == null then "-" else ((. / $hz * 10 | round) / 10 | tostring) end;

def tail_text:
  (.rendered_text // "") | gsub("[\\n\\t]"; " ")
  | if length > 60 then "…" + .[-60:] else . end;

def coverage:
  hz as $hz
  | .seal_coverage
  | if . == null then ""
    else "cov=\(.status) \((.coverage_ratio * 100) | round)% gap=\(.max_uncovered_samples | secs($hz))s holes=\(.uncovered_speech_ranges | length)"
    end;

def line:
  if .schema == "codescribe.transcript.v1" then
    "\(.emitted_at)\tBUS\t\(.status)\t\(.end_reason // "")\tsession=\(.session_id[0:8])"
  else
    "\(.emitted_at)\tBUS\t\(.reducer_action)\trev=\(.reducer_revision) seq=\(.sequence) \(coverage)\t\(tail_text)"
  end;

def burst_key:
  if .schema == "codescribe.transcript.v1" then "lifecycle/\(.sequence)"
  else "\(.reducer_revision)/\(.reducer_action)" end;

foreach inputs as $e ({key: null, prev: null};
  .prev = .key | .key = ($e | burst_key);
  select(($ENV.BUS_ALL // "") == "1" or .key != .prev) | $e | line)
