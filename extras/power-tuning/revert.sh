#!/usr/bin/env bash
# Undo apply.sh — back to power-profiles-daemon.
set -euo pipefail
[ "$(id -u)" -ne 0 ] || { echo "Run as your normal user."; exit 1; }

read -rp "Remove TLP tuning and restore power-profiles-daemon? [y/N] " a
[[ ${a,,} == y ]] || exit 0

sudo systemctl disable --now tlp.service 2>/dev/null || true
sudo rm -f /etc/tlp.d/01-omarchy-macbookair.conf
sudo rm -f /etc/sudoers.d/battery-plus
sudo rm -f /usr/local/bin/battery-plus-priv
sudo rm -f /etc/udev/rules.d/99-rapl-readable.rules
sudo udevadm control --reload || true

sudo systemctl unmask power-profiles-daemon.service 2>/dev/null || true
sudo systemctl enable --now power-profiles-daemon.service 2>/dev/null || true

echo "Reverted. 'tlp' and 'powertop' packages left installed — remove with:"
echo "  sudo pacman -Rns tlp powertop"
echo "Restart the shell:  omarchy restart shell"
