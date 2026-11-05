#!/usr/bin/env bash
# Measures real battery draw in repeated windows and prints the conditions that
# could explain a difference. Run on battery, idle, at the same brightness.
#   scripts/measure-draw.sh [label] [windows=5] [seconds=60]
set -uo pipefail
label=${1:-run} windows=${2:-5} seconds=${3:-60}
bat=$(ls -d /sys/class/power_supply/BAT* | head -1)
gpu=/sys/bus/pci/devices/0000:c2:00.0
out=${XDG_STATE_HOME:-$HOME/.local/state}/omarchy-power-hub/measure-draw.log
mkdir -p "$(dirname "$out")"

[ "$(cat "$bat/status")" = Discharging ] || { echo "not discharging: unplug first" >&2; exit 1; }
# Battery idle settings would otherwise blank or suspend the machine mid-run.
ops=$HOME/.config/omarchy/plugins/omarchy-power-hub/bin/ops.py
cfg=$HOME/.config/omarchy/omarchy-power-hub.json
keys="screensaver displaysOff lock sleep hibernateAfter"
restore=""
for k in $keys; do
  restore+="python3 $ops set battery $k $(jq -r ".battery.$k // 0" "$cfg") >/dev/null; "
  python3 "$ops" set battery "$k" 0 >/dev/null
done
omarchy toggle idle stay-awake >/dev/null 2>&1
trap "$restore omarchy toggle idle allow-idle >/dev/null 2>&1" EXIT

echo "settling 45 s for the embedded controller's smoothed current" >&2
sleep 45

watts() { awk -v c="$(cat "$bat/current_now")" -v v="$(cat "$bat/voltage_now")" 'BEGIN{printf "%.2f\n", (c<0?-c:c)*v/1e12}'; }
for w in $(seq 1 "$windows"); do
  [ "$(cat "$bat/status")" = Discharging ] || { echo "stopped: charger connected" >&2; exit 1; }
  s0=$(cat "$gpu/power/runtime_suspended_time" 2>/dev/null || echo 0)
  m=$(for _ in $(seq 1 "$seconds"); do watts; sleep 1; done | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
  s1=$(cat "$gpu/power/runtime_suspended_time" 2>/dev/null || echo 0)
  line="$(date +%T) $label window=$w median_W=$m gpu=$(cat "$gpu/power/runtime_status" 2>/dev/null) gpu_slept_ms=$((s1-s0)) profile=$(powerprofilesctl get) brightness=$(brightnessctl -m 2>/dev/null | cut -d, -f4) hz=$(hyprctl monitors -j | jq -r '.[0].refreshRate|floor') screen_on=$(hyprctl monitors -j | jq -r '.[0].dpmsStatus') load=$(cut -d' ' -f1 /proc/loadavg)"
  echo "$line" | tee -a "$out"
done
