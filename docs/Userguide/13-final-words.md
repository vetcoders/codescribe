# 13 - Final Words

Congratulations on completing this user guide. You now have all the knowledge needed to
use CodeScribe effectively in your daily workflow.

## What You Have Learned

Throughout this guide, you have discovered how to:

- Install and configure CodeScribe on your Mac
- Use global hotkeys to capture audio seamlessly
- Control recording modes (hold-to-talk and toggle)
- Enable and customize AI formatting for polished transcripts
- Configure language settings and LLM providers
- Manage your transcript history and audio logs
- Troubleshoot common issues with permissions and audio devices
- Use the CLI tools for batch processing and quality assessment

CodeScribe transforms your voice into text with privacy-first, on-device processing.
No audio leaves your machine unless you explicitly enable cloud-based AI formatting.

## Getting Help

If you encounter issues or have questions:

- **GitHub Issues**: https://github.com/VetCoders/CodeScribe/issues
  Open an issue with details about your problem. Include your macOS version,
  CodeScribe version (`codescribe --version`), and relevant log output.

- **Documentation**: Check `docs/WHISPER_LIVE.md` and `docs/ARCHITECTURE.md`
  for deeper technical details.

- **Help Menu**: Use the Help menu in the tray app for quick access to resources.

## Contributing

CodeScribe is open source under the Apache 2.0 license. Contributions are welcome:

1. Fork the repository on GitHub
2. Create a feature branch from `develop`
3. Make your changes and add tests where appropriate
4. Run `make check` to ensure code quality
5. Submit a pull request with a clear description

We appreciate bug reports, documentation improvements, and new features alike.

## Acknowledgments

CodeScribe stands on the shoulders of remarkable open-source projects:

- **OpenAI Whisper** - The speech recognition model that powers transcription
- **Candle** (Hugging Face) - Rust ML framework enabling local Whisper inference
- **Tauri** - Framework for building the native GUI application
- **Metal** (Apple) - GPU acceleration for fast on-device inference
- **cpal** - Cross-platform audio library for recording
- **tray-icon / muda / tao** - System tray and menu bar integration
- **reqwest** - HTTP client for API communication
- **fastembed** - Local embedding utilities
- **tokio** - Async runtime powering the application

Special thanks to the Rust community for building such a solid ecosystem,
and to Apple for Metal GPU acceleration on Apple Silicon.

## Credits

CodeScribe was created by the VetCoders team:

- **Maciej Gad** (@Szowesgad) - Founder and lead developer
- **Monika Szymanska** (@m-szymanska) - Product Owner Vista
- **Klaudiusz** (AI) - AI development partner
- **Junie** (AI) - AI assistant

## VetCoders Mission

VetCoders started as an experiment: can a veterinarian with zero coding experience
build production software with AI assistance? The answer is yes.

Our mission is to prove that domain expertise combined with AI partnership can
create tools that solve real problems. CodeScribe exists because voice-to-text
should be private, fast, and work offline. No cloud dependency, no subscription,
no data collection - just your voice turned into text on your own machine.

We believe in building tools that respect user privacy and work reliably.

## Thank You

Thank you for choosing CodeScribe. We hope it serves you well in your work,
whether you are writing code, taking notes, composing messages, or capturing ideas.

Your feedback makes this tool better. If CodeScribe helps you, consider starring
the repository or sharing it with others who might benefit.

Happy transcribing.

---

*Created by M&K (c)2026 VetCoders*
