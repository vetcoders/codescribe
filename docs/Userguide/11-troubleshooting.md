# 11 - Troubleshooting

This guide covers common problems and their solutions for CodeScribe on macOS.

## App Won't Start

**No menu bar icon appears:**
- Check if CodeScribe is already running in Activity Monitor
- If running, force quit and restart the app
- Verify the app bundle is in /Applications folder
- Check Console.app for crash logs (filter by "CodeScribe")

**App crashes immediately:**
- Delete `~/.codescribe/` folder to reset configuration
- Reinstall CodeScribe from a fresh download
- Check macOS version compatibility (requires macOS 13+)

## Microphone Not Working

**No audio captured:**
1. Open System Settings > Privacy & Security > Microphone
2. Ensure CodeScribe is listed and enabled
3. If not listed, remove and reinstall the app

**Wrong microphone selected:**
- Open CodeScribe Settings and check the Input Device dropdown
- Select the correct microphone from the list
- Test by speaking and watching the audio level indicator

**Audio too quiet or distorted:**
- Adjust system input volume in System Settings > Sound > Input
- Check the SILENCE_DB setting in `~/.codescribe/.env` (default: -45.0)
- Lower values (e.g., -50) are more sensitive to quiet sounds

## Hotkeys Not Responding

**Hold-to-talk not working:**
1. Open System Settings > Privacy & Security > Accessibility
2. Find CodeScribe.app and enable it
3. If already enabled, toggle it off and on again
4. Restart CodeScribe after granting permission

**Double-tap toggle not working:**
- Verify Toggle Trigger is enabled in Settings (not set to "None")
- Check for conflicts with other apps using Option key
- Increase double-tap timing if needed (DOUBLE_TAP_INTERVAL_MS in .env)

**Hotkey conflicts with other apps:**
- Change Hold Modifiers in Settings (Ctrl, Ctrl+Alt, Ctrl+Shift, Ctrl+Cmd)
- Disable conflicting shortcuts in other applications
- Set TOGGLE_TRIGGER=None to disable toggle mode entirely

## Transcription Quality Issues

**Whisper producing gibberish or hallucinations:**
- Recording may be too short (minimum ~0.5 seconds needed)
- Background noise is too high - find a quieter environment
- Check that correct language is selected in Settings

**Repeated words or phrases ("Wielki, Wielki, Wielki..."):**
- This is a known Whisper behavior with short audio
- Enable AI Formatting to automatically clean repetitions
- Speak in complete sentences rather than short fragments

**Wrong language detected:**
- Set explicit language in Settings instead of "Auto"
- Supported: Polish (pl), English (en), German (de), Spanish (es), French (fr)

**Transcription is slow:**
- First transcription loads the model (10-30 seconds)
- Subsequent transcriptions should be faster
- Check Activity Monitor for CPU/memory usage
- Try a smaller Whisper model variant if available

## AI Formatting Failures

**"LLM endpoint is required" error:**
- Set LLM_ENDPOINT in `~/.codescribe/.env`
- Example: `LLM_ENDPOINT=https://api.openai.com/v1/responses`

**"LLM API key is required" error:**
- Set LLM_API_KEY in `~/.codescribe/.env`
- For OpenAI: use your API key from platform.openai.com
- For local Ollama: API key is not required

**AI formatting returns raw text unchanged:**
- Check API key validity and quota
- Verify endpoint URL is correct
- Try with a different model (LLM_MODEL setting)
- Check network connectivity to the LLM provider

**Timeout errors during formatting:**
- Increase timeout: `CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS=5000`
- Check if LLM provider is experiencing issues
- Try a faster model or local Ollama

## Backend Connection Issues

**"Backend server not found" error:**
- The Python backend may not be running
- Check if ports 8237, 8238, 7237, 6237 are available
- Run `lsof -i :8237` to check what's using the port

**Transcription times out:**
- Backend may be overloaded - wait and retry
- Check backend logs for errors
- Restart the backend service

## How to Check Logs

**View application logs:**
```bash
# Open Console.app and filter by "CodeScribe"
open -a Console
```

**View transcription history:**
```bash
# History is stored in dated folders
ls -la ~/.codescribe/history/
```

**Enable debug logging:**
```bash
# Add to ~/.codescribe/.env
RUST_LOG=debug
```

**Open history folder in Finder:**
- Use the tray menu: Settings > Open History Folder
- Or run: `open ~/.codescribe/history`

## How to Reset to Defaults

**Reset all settings:**
```bash
# Backup current config
cp ~/.codescribe/.env ~/.codescribe/.env.backup

# Remove config to regenerate defaults
rm ~/.codescribe/.env

# Restart CodeScribe
```

**Clear transcription history:**
```bash
rm -rf ~/.codescribe/history/*
```

**Complete reset (nuclear option):**
```bash
# Remove all CodeScribe data
rm -rf ~/.codescribe/

# Restart CodeScribe to regenerate defaults
```

## Permission Checklist

If CodeScribe isn't working correctly, verify these permissions:

| Permission | Location | Required For |
|------------|----------|--------------|
| Microphone | Privacy & Security > Microphone | Audio recording |
| Accessibility | Privacy & Security > Accessibility | Global hotkeys |
| Input Monitoring | Privacy & Security > Input Monitoring | Keyboard events |

## Still Stuck?

1. Quit and restart CodeScribe
2. Check the Settings panel for obvious misconfigurations
3. Review logs in Console.app
4. Reset to defaults (see above)
5. Check GitHub Issues for known problems
6. Open a new issue with logs and steps to reproduce

---

*Created by M&K (c)2026 VetCoders*
