#!/bin/sh
# install.sh: install the thepia CLI (commands: thepia, the, th) from this release bundle.
#
# It copies the binary into ~/.thepia/bin (no privileges needed), then tries to link the commands
# onto your PATH via a standard bin directory (/usr/local/bin, /opt/homebrew/bin, ~/.local/bin). If
# that needs permissions you have not granted, it degrades gracefully: the binary is still installed
# under ~/.thepia/bin and it prints exactly how to finish (add that dir to PATH, or link with sudo).
#
# Re-running is safe (idempotent). Override the install dir with THEPIA_PREFIX=/path.
set -eu

PREFIX="${THEPIA_PREFIX:-$HOME/.thepia}"
BIN="$PREFIX/bin"
SRC="$(cd "$(dirname "$0")" && pwd)"
REAL="thepia"            # the real binary; `the` and `th` dispatch on argv[0]
NAMES="thepia the th"

say()  { printf '%s\n' "$*"; }
warn() { printf '%s\n' "$*" >&2; }

# --- 1. Install the binary under ~/.thepia/bin (aliases are symlinks to the one binary). ----------
if [ ! -f "$SRC/$REAL" ]; then
  warn "error: '$REAL' not found next to this script ($SRC)."
  warn "Run install.sh from inside the extracted release bundle."
  exit 1
fi
mkdir -p "$BIN"
cp "$SRC/$REAL" "$BIN/$REAL.tmp.$$"
chmod 0755 "$BIN/$REAL.tmp.$$"
mv -f "$BIN/$REAL.tmp.$$" "$BIN/$REAL"   # atomic replace, so re-install never hits ETXTBSY
for n in the th; do
  ln -sf "$REAL" "$BIN/$n"               # relative symlink inside ~/.thepia/bin
done

# macOS: a downloaded binary may be quarantined; clear it so it runs without a Gatekeeper prompt.
if [ "$(uname)" = "Darwin" ]; then
  xattr -dr com.apple.quarantine "$BIN" 2>/dev/null || true
fi

# Verify it actually runs on this machine (catches a wrong-arch/OS bundle early).
if ! VERSION="$("$BIN/$REAL" --version 2>/dev/null)"; then
  warn "error: installed '$BIN/$REAL' does not run on this machine."
  warn "You likely downloaded the wrong platform bundle; pick the one matching your OS/CPU."
  exit 1
fi
say "Installed $VERSION to $BIN"

# --- 2. Put the commands on PATH. ----------------------------------------------------------------
case ":${PATH:-}:" in
  *":$BIN:"*)
    say "$BIN is already on your PATH. Try: thepia skills lint"
    exit 0 ;;
esac

# Link into the first standard bin dir that is on PATH and writable without escalation.
link_into() {
  dir="$1"
  [ -d "$dir" ] || return 1
  case ":${PATH:-}:" in *":$dir:"*) : ;; *) return 1 ;; esac   # must already be on PATH
  [ -w "$dir" ] || return 1                                     # no sudo: graceful skip
  for n in $NAMES; do ln -sf "$BIN/$n" "$dir/$n"; done
  say "Linked thepia, the, th into $dir. Try: thepia skills lint"
  return 0
}

for d in "$HOME/.local/bin" /usr/local/bin /opt/homebrew/bin; do
  if link_into "$d"; then exit 0; fi
done

# --- 3. Graceful fallback: nothing on PATH was writable. Tell the user how to add it to PATH. -----
# Collapse $HOME to a literal $HOME so the rc line is portable, and name the right file for the shell.
disp="$BIN"
case "$BIN" in "$HOME"/*) disp="\$HOME${BIN#"$HOME"}" ;; esac
case "${SHELL:-}" in
  */fish) rc="$HOME/.config/fish/config.fish"; addline="fish_add_path $disp" ;;
  */zsh)  rc="$HOME/.zshrc";   addline="export PATH=\"$disp:\$PATH\"" ;;
  */bash) rc="$HOME/.bashrc";  addline="export PATH=\"$disp:\$PATH\"" ;;
  *)      rc="$HOME/.profile"; addline="export PATH=\"$disp:\$PATH\"" ;;
esac

say ""
say "Installed, but $BIN is not on your PATH and no standard bin directory was writable."
say "To use the 'thepia', 'the', and 'th' commands, add that directory to your PATH:"
say ""
say "  echo '$addline' >> $rc"
say "  exec \$SHELL -l          # reload your shell (or open a new terminal)"
say ""
say "Or, to link into a system directory instead (prompts for your password):"
say ""
say "  sudo ln -sf $BIN/thepia /usr/local/bin/thepia"
say "  sudo ln -sf $BIN/the    /usr/local/bin/the"
say "  sudo ln -sf $BIN/th     /usr/local/bin/th"
say ""
exit 0
