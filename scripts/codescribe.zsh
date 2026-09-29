# codescribe — zsh line-editor integration.
#
# Puts the last thing you dictated into the command line you are typing.
#
#   source /path/to/codescribe/scripts/codescribe.zsh
#
# Then dictate as usual and press Ctrl-X Ctrl-V. To bind another key, call
# `bindkey <key> codescribe-insert-last` after sourcing.
#
# WHY A WIDGET AS WELL AS UI INSERT. Overlay Insert may restore a positively
# latched terminal and use one borrowed-clipboard Cmd+V. This widget is the
# explicit no-synthetic-event path: a key the human presses reads the same
# committed Bus, needs no Accessibility grant, and works inside tmux, zellij,
# and a bare tty without becoming a second transcript authority.

codescribe-insert-last() {
  local text receipt rc
  # stderr carries the receipt (session, chars, bus path). Inside a zle
  # widget it cannot go straight to the terminal without garbling the line
  # editor, so capture it and surface it through the zle message area when
  # the insert fails instead of discarding the diagnostic.
  receipt="$(mktemp)"
  text=$(command codescribe transcribe last 2>"$receipt")
  rc=$?
  if (( rc != 0 )) || [[ -z $text ]]; then
    local detail
    detail="$(tail -n 1 -- "$receipt" 2>/dev/null)"
    rm -f -- "$receipt"
    zle -M "codescribe: ${detail:-nic do wklejenia — bus nie ma zamkniętej wypowiedzi}"
    return 1
  fi
  rm -f -- "$receipt"
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
