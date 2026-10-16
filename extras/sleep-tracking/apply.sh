#!/usr/bin/env bash
# Battery+ — install sleep-drain tracking.
# Run as your normal user:   bash extras/sleep-tracking/apply.sh
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
HOOK=/usr/lib/systemd/system-sleep/battery-plus-sleep
LOG=/var/log/battery-plus-sleep.log

[ "$(id -u)" -ne 0 ] || { echo "Run as your normal user, not root — it calls sudo itself."; exit 1; }

cat <<EOF
This will:
  • install   $HOOK
                (records battery capacity/charge/voltage before and after
                every suspend, so the panel can show real sleep power draw)
  • create    $LOG   (world-readable, so the panel can read it without root)

Undo anytime with ./revert.sh
EOF
read -rp $'\nProceed? [y/N] ' a
[[ ${a,,} == y ]] || { echo "aborted"; exit 0; }

sudo install -Dm755 -o root -g root "$HERE/battery-plus-sleep" "$HOOK"

# Fix ownership/permissions explicitly rather than touch+chmod, which does
# neither on a file that already exists: touch never chowns, and chmod
# never touches ownership either. A pre-existing $LOG under an unexpected
# owner would otherwise keep write access forever after, despite living at
# a path that now looks like a root-owned plugin log. Preserve content if
# the log is already a real log from a previous install; refuse outright
# if it's anything other than a regular file (e.g. a symlink).
if [ -L "$LOG" ]; then
  echo "Refusing to use $LOG: it's a symlink." >&2
  exit 1
elif [ -e "$LOG" ]; then
  if [ ! -f "$LOG" ]; then
    echo "Refusing to use $LOG: it exists but is not a regular file." >&2
    exit 1
  fi
  sudo chown root:root "$LOG"
  sudo chmod 644 "$LOG"
else
  sudo install -Dm644 -o root -g root /dev/null "$LOG"
fi

cat <<EOF

Done. The next time this laptop actually sleeps for a while (lid close,
idle timeout, or manual suspend) one line will be appended to $LOG.
Open the Battery+ panel afterward to see it under "Sleep".
EOF
