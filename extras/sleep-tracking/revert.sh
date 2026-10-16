#!/usr/bin/env bash
# Battery+ — remove sleep-drain tracking. Run as your normal user.
set -euo pipefail

sudo rm -f /usr/lib/systemd/system-sleep/battery-plus-sleep

echo "Removed the sleep-tracking hook."
echo "Recorded history is kept at /var/log/battery-plus-sleep.log — remove it by hand if you don't want it."
