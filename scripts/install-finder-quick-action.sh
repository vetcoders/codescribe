#!/bin/zsh
# Install the "Transcribe with Codescribe" Finder Quick Action.
#
# Right-click any selection of audio/video files in Finder → Quick Actions →
# Transcribe with Codescribe. Each file goes through the SAME product pipeline
# as `codescribe transcribe` (final-pass Whisper verdict), with `--no-bus` so a
# batch of old recordings never displaces "Copy last transcript". The result
# lands as a .txt next to each source file and the combined text is copied to
# the clipboard.
#
# This is a distribution surface for an existing corridor — zero new decode
# code. Re-run after CLI changes only if this script itself changed; the
# workflow always calls the installed binary.
set -euo pipefail

SERVICE_DIR="${1:-$HOME/Library/Services}"
WORKFLOW="$SERVICE_DIR/Transcribe with Codescribe.workflow"
CONTENTS="$WORKFLOW/Contents"

SHELL_COMMAND=$(cat <<'SCRIPT'
CODESCRIBE="${CODESCRIBE_BIN:-$HOME/.local/bin/codescribe}"
if [ ! -x "$CODESCRIBE" ]; then
  CODESCRIBE="$HOME/.cargo/bin/codescribe"
fi
if [ ! -x "$CODESCRIBE" ]; then
  osascript -e 'display notification "Install the Codescribe CLI first" with title "Codescribe"'
  exit 1
fi
combined=""
failed=0
for f in "$@"; do
  out="${f%.*}.txt"
  if [ -e "$out" ] || [ -L "$out" ]; then
    print -ru2 -- "Refusing to overwrite: $out"
    failed=$((failed + 1))
    continue
  fi
  tmp=$(mktemp "${out}.XXXXXX") || { failed=$((failed + 1)); continue; }
  if "$CODESCRIBE" transcribe --no-bus "$f" > "$tmp" && ln "$tmp" "$out"; then
    combined="$combined$(cat "$out")"$'\n\n'
  else
    print -ru2 -- "Transcription failed: $f"
    failed=$((failed + 1))
  fi
  rm -f "$tmp"
done
if [ -n "$combined" ]; then
  printf '%s' "$combined" | pbcopy
fi
count=$#
osascript -e "display notification \"$((count - failed))/$count transcribed · text copied for successful files · existing files kept\" with title \"Codescribe\""
[ "$failed" -eq 0 ]
SCRIPT
)

mkdir -p "$CONTENTS"

# Escape the shell command for XML embedding.
XML_COMMAND=$(printf '%s' "$SHELL_COMMAND" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSBackgroundColorName</key>
			<string>background</string>
			<key>NSIconName</key>
			<string>NSTouchBarAudioInputTemplate</string>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>Transcribe with Codescribe</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.audio</string>
				<string>public.movie</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
PLIST

cat > "$CONTENTS/document.wflow" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>528</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>${XML_COMMAND}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/zsh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>6E2E4F9A-0000-0000-0000-000000000001</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
					<string>Command</string>
					<string>Run</string>
					<string>Unix</string>
				</array>
				<key>OutputUUID</key>
				<string>6E2E4F9A-0000-0000-0000-000000000002</string>
				<key>UUID</key>
				<string>6E2E4F9A-0000-0000-0000-000000000003</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict>
					<key>0</key>
					<dict>
						<key>default value</key>
						<integer>0</integer>
						<key>name</key>
						<string>inputMethod</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>0</string>
					</dict>
				</dict>
				<key>isViewVisible</key>
				<integer>1</integer>
				<key>location</key>
				<string>309.000000:253.000000</string>
				<key>nibPath</key>
				<string>/System/Library/Automator/Run Shell Script.action/Contents/Resources/Base.lproj/main.nib</string>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleID</key>
		<string>com.apple.finder</string>
		<key>applicationBundleIDsByPath</key>
		<dict>
			<key>/System/Library/CoreServices/Finder.app</key>
			<string>com.apple.finder</string>
		</dict>
		<key>applicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>applicationPaths</key>
		<array>
			<string>/System/Library/CoreServices/Finder.app</string>
		</array>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<integer>0</integer>
		<key>serviceApplicationBundleID</key>
		<string>com.apple.finder</string>
		<key>serviceApplicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
PLIST

# Ask the pasteboard server to pick up the new service without a re-login.
if (( $# == 0 )); then
  /System/Library/CoreServices/pbs -update >/dev/null 2>&1 || true
fi

echo "installed: $WORKFLOW"
echo "Finder → right-click audio/video selection → Quick Actions → Transcribe with Codescribe"
