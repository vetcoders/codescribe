#!/usr/bin/env bash

# Stage a WAV without allocating a second payload on the same filesystem.
stage_fixture_audio() {
  local source="$1" destination="$2" source_device destination_device
  source_device="$(stat -f %d "$source")" || return
  destination_device="$(stat -f %d "$(dirname "$destination")")" || return
  if [[ "$source_device" == "$destination_device" ]]; then
    ln "$source" "$destination"
  else
    cp -c "$source" "$destination" || cp -p "$source" "$destination"
  fi
}
