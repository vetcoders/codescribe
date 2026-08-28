# codescribe — zsh line-editor integration.
#
# Puts the last thing you dictated into the command line you are typing.
#
#   source /path/to/codescribe/scripts/codescribe.zsh
#
# Then dictate as usual and press Ctrl-X Ctrl-V. To bind another key, call
# `bindkey <key> codescribe-insert-last` after sourcing.
#
# WHY A WIDGET AND NOT A PASTE. A synthetic Cmd+V targets the frontmost
# application, which is the terminal running the command — the app's delivery
# throne already refuses that case as `refuse_paste_into_self`, and it would
# need an Accessibility grant to attempt it at all. A ZLE widget inserts under
# a key the operator presses: no permission, no synthetic event, and it works
# the same inside tmux, zellij, and a bare tty.

codescribe-insert-last() {
  local text
  # stderr carries the receipt (session, chars, bus path); the prompt only
  # wants the words, so let the receipt go to the terminal and keep stdout.
  text=$(command codescribe transcribe last 2>/dev/null)
  if (( $? != 0 )) || [[ -z $text ]]; then
    zle -M "codescribe: nic do wklejenia — bus nie ma zamkniętej wypowiedzi"
    return 1
  fi
  LBUFFER+="$text"
}
zle -N codescribe-insert-last
bindkey '^X^V' codescribe-insert-last

# Same text, for a pane that is not the one you are typing in.
#   codescribe-send-to <pane>   e.g. codescribe-send-to %3
codescribe-send-to() {
  local target=${1:?usage: codescribe-send-to <tmux-pane>}
  local text
  text=$(command codescribe transcribe last) || return 1
  # -l sends the text literally: no key names are interpreted, and no Enter is
  # appended, so the target pane keeps an editable line.
  tmux send-keys -t "$target" -l -- "$text"
}

# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
