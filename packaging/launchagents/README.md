# LaunchAgents

Legacy sample plists lived here, but CodeScribe now generates per-user LaunchAgents
with `packaging/scripts/install_backend.command`. Use that helper so the generated
`com.CodeScribe.backend.plist` contains the correct paths, Whisper model location,
and shared settings file.

If you do need to craft a custom LaunchAgent, start from the one produced by the
installer (~/Library/LaunchAgents/com.CodeScribe.backend.plist) and update only the
fields you care about. Do **not** commit machine-specific plists to the repo.
