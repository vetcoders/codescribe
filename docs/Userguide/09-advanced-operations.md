# 09 - Advanced Operations

This section covers command-line tools, batch processing, and power-user features for CodeScribe.

## Command Line Interface

CodeScribe provides a CLI for automation and scripting.

### Basic Commands

```bash
# Start the tray app (daemon mode)
codescribe

# Open config file in your default editor
codescribe --config

# Transcribe a single audio file
codescribe transcribe recording.wav

# Transcribe with language hint
codescribe transcribe meeting.mp3 --language pl

# Transcribe with AI formatting
codescribe transcribe interview.m4a --format

# Transcribe with specific LLM model
codescribe transcribe lecture.wav --format --llm qwen2.5:7b
```

### Piping Output

The transcribe command outputs text to stdout, making it easy to pipe:

```bash
# Save to file
codescribe transcribe audio.wav > transcript.txt

# Pipe to clipboard (macOS)
codescribe transcribe audio.wav | pbcopy

# Pipe to another tool
codescribe transcribe audio.wav | wc -w
```

## Environment Variables

Enable stream logging for debugging:

```bash
CODESCRIBE_STREAM_LOG=1 codescribe
```

The log file is written to `~/.codescribe/stream.log`.

## Quality Reports

Generate accuracy reports comparing your transcriptions against reference text.

```bash
# Generate quality report for recent transcriptions
codescribe-quality --date 2026-01-17 --limit 5

# Full report without cloud comparison
codescribe-quality --skip-cloud --limit 10

# Report with custom input/output directories
codescribe-quality --input ~/recordings --out ~/reports/jan
```

### Quality Report Options

| Flag | Description |
|------|-------------|
| `--date` | Filter by date folder (e.g., 2026-01-17) |
| `--limit` | Process last N pairs (default: 3) |
| `--language` | Force language code (pl, en, etc.) |
| `--skip-cloud` | Skip cloud reference transcription |
| `--skip-formatting` | Skip AI formatting step |
| `--debug` | Show references immediately in HTML |
| `--copy-audio` | Copy audio files instead of symlinks |

## Self-Improving Quality Loop

The quality loop analyzes transcription accuracy and automatically tunes settings.

```bash
# Run quality loop with tuning suggestions
codescribe-loop --date 2026-01-17

# Apply tuning updates automatically
codescribe-loop --date 2026-01-17 --apply

# Compare against a baseline report
codescribe-loop --baseline ~/.codescribe/reports/quality_20260115/report.json
```

### Loop Tuning Options

| Flag | Description |
|------|-------------|
| `--apply` | Apply lexicon/prompt/gate updates |
| `--baseline` | Previous report for regression comparison |
| `--regression-threshold` | Delta threshold for WER/CER (default: 0.02) |
| `--lexicon-max` | Max lexicon entries to add (default: 50) |
| `--no-lexicon` | Skip lexicon auto-updates |
| `--no-gate` | Skip gate threshold tuning |
| `--no-prompt` | Skip prompt tuning |
| `--metrics-reference` | Source for metrics: corpus or cloud |
| `--lexicon-source` | Source for lexicon: corpus or cloud |

## Custom Lexicon

CodeScribe maintains a custom vocabulary to improve recognition of domain-specific terms.

### Lexicon Location

Lexicon files are stored in `~/.CodeScribe/lexicon/` as JSONL files organized by topic.

### Lexicon Entry Format

Each entry contains:
- `term` - The correct spelling
- `category` - Topic category (e.g., "medical", "programming")
- `phonetic` - Optional pronunciation hint
- `examples` - Usage examples

### Auto-Generated Lexicon

The quality loop generates `~/.codescribe/lexicon.custom.jsonl` with detected mispronunciations:

```json
{"term":"Kielce","mispronunciations":["kielce","kjelce","kieltse"]}
{"term":"Tauri","mispronunciations":["taury","tori","towri"]}
```

## Prompts Customization

Custom prompts control AI formatting behavior.

### Editing Prompts

Access via the app: **Settings > Edit AI Prompt** or **Settings > Open Prompts Folder**.

Prompts directory: `~/.codescribe/prompts/`

### Auto-Tuning

The quality loop may generate `formatting_tuning.txt` with additional instructions when AI formatting causes regressions.

## History Migration

Migrate old transcript filenames to the new ASCII + suffix naming scheme:

```bash
# Preview changes (dry run)
codescribe migrate-history --dry-run

# Apply migration
codescribe migrate-history

# Assume a specific kind for files without suffix
codescribe migrate-history --assume-kind ai
```

## Scripting Examples

### Batch Transcribe Directory

```bash
#!/bin/bash
for file in ~/recordings/*.wav; do
    name=$(basename "$file" .wav)
    codescribe transcribe "$file" --format > ~/transcripts/"$name".txt
done
```

### Daily Quality Check

```bash
#!/bin/bash
DATE=$(date +%Y-%m-%d)
codescribe-loop --date "$DATE" --apply --limit 10
```

### Integration with Other Tools

```bash
# Send transcription via curl
codescribe transcribe audio.wav | curl -X POST -d @- https://api.example.com/notes

# Process with jq (if JSON output)
codescribe-quality --out - 2>/dev/null | jq '.summary'
```

## Tips for Power Users

1. **Use language hints** - Specifying `--language` improves accuracy for mixed-language content
2. **Run quality loop weekly** - Regular tuning improves recognition over time
3. **Review lexicon suggestions** - Check auto-generated entries before trusting them
4. **Keep baseline reports** - Compare against known-good reports to detect regressions
5. **Stream logging** - Enable `CODESCRIBE_STREAM_LOG=1` when debugging postprocessing issues

---

*Created by M&K (c)2026 VetCoders*
