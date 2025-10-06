# Dev Quick Start — Useful One‑Liners

Below are copy‑pasteable one‑liners for the most common developer and power‑user tasks. All paths are anonymized and use `$HOME` or repo‑relative locations.

## Install / Launch

```bash
curl -fsSL https://raw.githubusercontent.com/LibraxisAI/VistaScribe/develop/packaging/scripts/install.sh | zsh -s -- --url "https://example.com/VistaScribe-0.1.0.dmg" --login
```

```bash
GH_TOKEN=__YOUR_GH_TOKEN__ curl -H "Authorization: token $GH_TOKEN" -fsSL https://raw.githubusercontent.com/LibraxisAI/VistaScribe/develop/packaging/scripts/install.sh | zsh -s -- --repo LibraxisAI/VistaScribe --latest --login
```

```bash
open -a "/Applications/VistaScribe.app" && tail -n +1 -f "$HOME/Library/Logs/VistaScribe.app.log"
```

```bash
launchctl unload -w "$HOME/Library/LaunchAgents/com.vistascribe.tray.plist" 2>/dev/null || true && launchctl load -w "$HOME/Library/LaunchAgents/com.vistascribe.tray.plist"
```

## Terminal aliases (zsh)

```bash
echo 'alias VistaScribe="open -a \"/Applications/VistaScribe.app\""' >> "$HOME/.zshrc" && echo 'alias VistaScribe-stop="cd \"/Applications/VistaScribe.app/Contents/Resources/Repo\" && ./scripts/quickstart_mac.sh --stop-all"' >> "$HOME/.zshrc" && exec $SHELL -l
```

## Run from source (repo root)

```bash
./scripts/quickstart_mac.sh --mode both && tail -n +1 -f logs/VistaScribe.log
```

```bash
./scripts/quickstart_mac.sh --stop-all && rm -f .pids/*.pid
```

```bash
grep -H . .pids/*.pid 2>/dev/null || echo "no pids" && ps aux | rg "(main|backend)\.py|VistaScribe\.app|quickstart_mac\.sh" -n || true
```

## Models — download / switch

```bash
uv run python scripts/get_models.py --whisper large-v3 --method auto
```

```bash
HF_TOKEN=__YOUR_HF_TOKEN__ uv run python scripts/get_models.py --whisper large-v3 --method hf --hf-token "$HF_TOKEN"
```

```bash
uv run python scripts/get_models.py --whisper small-mlx --method git
```

```bash
python -c 'from config import update_env_vars; update_env_vars({"WHISPER_VARIANT":"large-v3"})' && ./scripts/quickstart_mac.sh --stop-all && ./scripts/quickstart_mac.sh --mode both
```

## Logs / diagnostics

```bash
tail -n 200 "$HOME/Library/Logs/VistaScribe.app.log" || tail -n 200 logs/VistaScribe.log
```

```bash
codesign --verify --deep --strict --verbose=2 "/Applications/VistaScribe.app" && spctl --assess --type execute -vvvv "/Applications/VistaScribe.app"
```

```bash
xattr -dr com.apple.quarantine "/Applications/VistaScribe.app" || true
```

## Build (dev)

```bash
packaging/appwrap/build_wrapper_app.sh && packaging/dmg/build_dmg.sh
```

```bash
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" NOTARY_PROFILE="VSNotary" packaging/release.sh
```

```bash
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/release.sh
```

## Notary (advanced)

```bash
xcrun notarytool store-credentials VSNotary --apple-id "your@appleid.com" --team-id "TEAMID" --password "APP-SPECIFIC-PASS"
```

```bash
xcrun notarytool submit "packaging/dmg/Your_DMG.dmg" --keychain-profile "VSNotary" --wait && xcrun stapler staple "packaging/dmg/Your_DMG.dmg"
```

```bash
ditto -c -k --keepParent "packaging/dist/VistaScribe.app" VistaScribe.app.zip && xcrun notarytool submit VistaScribe.app.zip --keychain-profile "VSNotary" --wait && xcrun stapler staple "packaging/dist/VistaScribe.app" && rm -f VistaScribe.app.zip
```

## Config tweaks (env)

```bash
python -c 'from config import update_env_vars; update_env_vars({"HOLD_MODS":"ctrl+alt","HOLD_EXCLUSIVE":"1","RESTORE_CLIPBOARD":"1"})'
```

```bash
python -c 'from config import update_env_vars; update_env_vars({"BEEP_ON_START":"0","SOUND_NAME":"Tink","SOUND_VOLUME":"0.2"})'
```

```bash
python -c 'from config import update_env_vars; update_env_vars({"WHISPER_LANGUAGE":"pl"})' && ./scripts/quickstart_mac.sh --stop-all && ./scripts/quickstart_mac.sh --mode both
```

## Tray helpers

```bash
osascript -e 'tell application "System Settings" to activate' || true
```

```bash
open "$HOME/Library/LaunchAgents" && open "$HOME/Library/Logs"
```

