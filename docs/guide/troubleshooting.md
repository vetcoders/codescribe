# Troubleshooting

Common issues and solutions for CodeScribe.

---

## Quick Fixes

**80% of issues are solved by:**

1. ✅ Check Accessibility permission is enabled
2. ✅ Check Microphone permission is enabled
3. ✅ Restart CodeScribe
4. ✅ Check menu bar icon is visible (not hidden by notch)

---

## Startup Issues

### CodeScribe doesn't launch

**Symptoms**: Nothing happens when opening the app.

**Solutions**:

1. Check Activity Monitor for running `codescribe` process
2. Kill any zombie processes: `pkill -9 codescribe`
3. Try launching from Terminal: `codescribe -v`
4. Check Console.app for crash logs

### Menu bar icon not visible

**Symptoms**: App launches but no icon appears.

**Solutions**:

1. Check if icon is hidden behind notch (MacBook Pro)
2. Use Bartender or similar to reveal hidden icons
3. Run `codescribe` from Terminal to confirm it's running

### "Model loading" takes forever

**Symptoms**: Icon stays gray, no response to hotkeys.

**Solutions**:

1. First launch takes 5-10 seconds - wait
2. Check RAM usage (Whisper needs ~2GB)
3. Restart if stuck more than 30 seconds

---

## Permission Issues

### Hotkeys don't work

**Symptoms**: Pressing Ctrl does nothing.

**Solutions**:

1. **Check Accessibility**:

   - System Settings → Privacy & Security → Accessibility
   - Ensure CodeScribe is in the list and enabled
   - If present but not working, remove and re-add

2. **Check Input Monitoring**:

   - System Settings → Privacy & Security → Input Monitoring
   - Enable CodeScribe

3. **Restart after permission change**:
   ```bash
   pkill -9 codescribe
   open -a CodeScribe
   ```

### Microphone not recording

**Symptoms**: Recording starts but no audio captured.

**Solutions**:

1. **Check Microphone permission**:

   - System Settings → Privacy & Security → Microphone
   - Enable CodeScribe

2. **Check audio input**:

   - System Settings → Sound → Input
   - Verify correct microphone selected
   - Speak and check input level meter

3. **Test microphone**:
   ```bash
   # Record 5 seconds
   codescribe transcribe --record 5
   ```

### Text doesn't paste

**Symptoms**: Transcription works but text doesn't appear.

**Solutions**:

1. **Check Accessibility** (required for simulating keyboard)
2. **Check target app accepts text input**
3. **Try manual paste**: Text is in clipboard, use Cmd+V

---

## Transcription Quality

### Empty transcripts

**Symptoms**: Recording completes but transcript is blank.

**Causes**:

- Microphone not picking up audio
- Audio too quiet
- Wrong input device

**Solutions**:

1. Speak louder/closer to mic
2. Check System Settings → Sound → Input level
3. Test with: `codescribe transcribe --record 5 -v`

### Poor accuracy

**Symptoms**: Many wrong words, missed phrases.

**Solutions**:

1. **Environment**:

   - Reduce background noise
   - Use headset microphone
   - Speak at moderate pace

2. **Language setting**:

   - Set correct language: `WHISPER_LANGUAGE=en`
   - Whisper auto-detects but explicit is better

3. **Repetition loops** ("word, word, word..."):
   - This is a known Whisper issue
   - CodeScribe auto-detects and cleans these
   - If persistent, try shorter recordings

### Mixed language issues

**Symptoms**: Code-switching or multilingual speech garbled.

**Solutions**:

- Set primary language explicitly
- Avoid switching languages mid-sentence
- Use AI formatting to fix mixed content

---

## AI Formatting Issues

### "AI Failed" error

**Symptoms**: Recording works but AI formatting fails.

**Solutions**:

1. **Check API configuration**:

   ```bash
   cat ~/.codescribe/.env | grep LLM
   ```

   Verify `LLM_ENDPOINT`, `LLM_API_KEY`, `LLM_MODEL` are set.

2. **Test API connectivity**:

   ```bash
   curl $LLM_ENDPOINT/models -H "Authorization: Bearer $LLM_API_KEY"
   ```

3. **Check API key validity**:
   - OpenAI: Check usage/billing at platform.openai.com
   - Anthropic: Check at console.anthropic.com
   - Local: Ensure Ollama is running

### Slow AI responses

**Symptoms**: Long delay before AI response appears.

**Solutions**:

1. Enable streaming: `LLM_USE_STREAMING=1`
2. Use faster model (gpt-4o-mini vs gpt-4)
3. Check network connection
4. For local: ensure Ollama has GPU acceleration

### AI not activated

**Symptoms**: Getting raw text even with AI enabled.

**Solutions**:

1. Verify provider settings in **Settings → AI & Prompts** (Formatting AI / Assistive AI)
2. If you're using **Dictation** mode: enable **Settings → Audio & Input → AI Formatting** (optional)
3. If you want AI every time: use **Formatting** (double‑tap `Left Option`) or **Assistive (Agent)** (double‑tap `Right Option`)

---

## Performance Issues

### High CPU usage

**Symptoms**: Mac gets hot, fans spin up.

**Causes**: Whisper transcription is GPU-intensive.

**Solutions**:

1. This is normal during transcription
2. Should drop after recording stops
3. If persistent, restart CodeScribe

### Memory usage

**Symptoms**: CodeScribe using several GB of RAM.

**Normal**: Whisper model needs ~2GB.

**If excessive (>4GB)**:

1. Restart CodeScribe
2. Check for memory leaks with Activity Monitor
3. Report issue with `codescribe -v` logs

---

## Log Files

### View logs

```bash
# Recent logs
tail -100 ~/.codescribe/logs/codescribe.log

# Live logs
tail -f ~/.codescribe/logs/codescribe.log

# Verbose mode (run in foreground)
codescribe -v
```

### Important log messages

| Message                      | Meaning                     |
| ---------------------------- | --------------------------- |
| `Whisper engine initialized` | Model loaded successfully   |
| `Recording started`          | Microphone activated        |
| `Transcription complete`     | Whisper finished processing |
| `AI formatting applied`      | AI cleaned up text          |
| `Text pasted successfully`   | Clipboard + paste worked    |

### Error patterns

| Error                          | Cause               | Fix                      |
| ------------------------------ | ------------------- | ------------------------ |
| `Failed to initialize Whisper` | Model not found     | Reinstall CodeScribe     |
| `Microphone access denied`     | Permission missing  | Grant in System Settings |
| `Backend unavailable`          | Health check failed | Check LLM configuration  |
| `Empty transcript`             | No audio captured   | Check microphone         |

---

## Reset Everything

If all else fails, complete reset:

```bash
# Stop CodeScribe
pkill -9 codescribe

# Backup config
cp -r ~/.codescribe ~/.codescribe.backup

# Reset config
rm ~/.codescribe/.env
codescribe --config

# Clear caches
rm -rf ~/.codescribe/logs/*

# Restart
open -a CodeScribe
```

---

## Getting Help

If troubleshooting doesn't solve your issue:

1. **Collect logs**: `codescribe -v 2>&1 | tee debug.log`
2. **Note your setup**: macOS version, chip type, CodeScribe version
3. **Open issue**: [GitHub Issues](https://github.com/VetCoders/CodeScribe/issues)

Include in your report:

- macOS version (System Settings → General → About)
- Chip (M1/M2/M3/Intel)
- CodeScribe version (`codescribe --version`)
- Relevant log output
- Steps to reproduce

---

_Created by M&K (c)2026 VetCoders_
