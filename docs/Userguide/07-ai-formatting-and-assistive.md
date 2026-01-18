# 07 - AI Formatting and Assistive Mode

CodeScribe offers two AI-powered modes that transform raw speech-to-text output into polished, usable text.

## Overview

Raw transcription from Whisper often lacks punctuation, proper capitalization, and paragraph structure. AI formatting fixes these issues automatically. Assistive mode goes further, enhancing and structuring your message for specific use cases.

## Two Modes Explained

### Formatting Mode (Default)

Formatting mode cleans up transcription without changing meaning:

- Adds punctuation (periods, commas, question marks)
- Fixes capitalization (sentence starts, proper nouns)
- Structures text with paragraphs and bullet points
- Removes Whisper repetition artifacts ("Wielki, Wielki, Wielki" becomes "Wielki")

**Before:** `cześć jak się masz mam pytanie pytanie pytanie do ciebie`

**After:** `Cześć, jak się masz? Mam pytanie do ciebie.`

### Assistive Mode

Assistive mode acts as a "courier" that enhances your message while preserving your intent. It augments and passes your words forward rather than responding to them.

**Before:** `chcę zrobić dark mode w aplikacji`

**After:** `Chcę zrobić dark mode w aplikacji. Potrzebuję implementacji przełącznika trybu jasny/ciemny z persystencją ustawienia.`

## Keyboard Controls

| Action | Trigger | Result |
|--------|---------|--------|
| Raw transcription | Hold Ctrl | No AI processing, direct paste |
| Assistive transcription | Hold Ctrl + Shift | AI enhances your message |
| Normal with AI formatting | Double tap LEFT Option | Formatting mode |
| Assistive hands-off | Double tap RIGHT Option | Assistive mode toggle |

## Enabling AI Formatting

AI formatting requires a configured AI provider and API key. Without configuration, you get raw transcription.

Toggle via menu bar: **Settings > AI Formatting**

### Environment Variables

Configure your AI provider in `.env`:

```bash
# Shared defaults (used if mode-specific vars not set)
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4o-mini
LLM_API_KEY=sk-your-api-key

# Optional: separate providers for each mode
LLM_FORMATTING_ENDPOINT=http://localhost:11434
LLM_FORMATTING_MODEL=llama3.2
LLM_ASSISTIVE_ENDPOINT=https://api.openai.com/v1/responses
LLM_ASSISTIVE_MODEL=gpt-4o
```

## Customizing Prompts

Prompts are stored in `~/.config/codescribe/prompts/`:

| File | Purpose |
|------|---------|
| `formatting.txt` | Main formatting mode instructions |
| `formatting_tuning.txt` | Optional additions to formatting prompt |
| `assistive.txt` | Main assistive mode instructions |
| `assistive_tuning.txt` | Optional additions to assistive prompt |

Edit via GUI (menu bar option) or terminal: `open ~/.config/codescribe/prompts/formatting.txt`

The `_tuning.txt` files let you add custom instructions without modifying base prompts. Use them for domain-specific terminology or language-specific rules.

To reset prompts, delete the files (they regenerate on next launch) or use the menu bar reset option.

## AI Provider Options

### Cloud Providers (OpenRouter, OpenAI)

```bash
LLM_ENDPOINT=https://openrouter.ai/api/v1/responses
LLM_MODEL=anthropic/claude-3-haiku
LLM_API_KEY=sk-or-your-key
```

### Local with Ollama

Run AI completely offline:

```bash
LLM_ENDPOINT=http://localhost:11434
LLM_MODEL=llama3.2
# No API key needed for local Ollama
```

Ollama is detected automatically when the endpoint points to localhost without `/v1/` in the path.

## Privacy Considerations

**Cloud providers:** Your transcribed text is sent to external servers. Check your provider's privacy policy.

**Local Ollama:** All processing happens on your machine. No data leaves your computer.

**Conversation memory:** Assistive mode maintains short-term memory (up to 4000 characters) for context continuity. This memory is local and clears when CodeScribe restarts.

## Troubleshooting

**AI formatting not working:**
1. Check that `AI_FORMATTING_ENABLED=true` in your `.env`
2. Verify your API key is valid
3. Check the endpoint URL is correct

**Formatting returns raw text:** The AI may return unformatted text if input is very short (under 10 characters), all retry attempts failed, or the provider returned an error. CodeScribe retries once after 5 seconds if the first attempt returns unchanged text.

**Repetition loops:** Whisper sometimes produces artifacts like "test test test test". CodeScribe detects and removes these automatically using pattern matching and semantic similarity analysis.
