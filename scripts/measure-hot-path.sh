#!/usr/bin/env bash
# Times the hot-path verbs of bin/power-hub: the median wall and CPU time of N runs.
#   scripts/measure-hot-path.sh [--runs N]
# Runs the real verbs: status and boost-check may re-apply a charge limit, and
# idle-wake-poll sends real eval requests (only with the lid open and displays on).
# Wall time is from date +%s%N, so it includes the two date forks (about 1 ms).
# CPU time is the user+sys time that bash's `time` reads from getrusage, to 1 ms.
# Peak memory is not measured.
set -euo pipefail
export LC_ALL=C
bin=$(dirname "$(readlink -f "$0")")/../bin/power-hub
verbs=("data" "status" "gpu" "boost-check" "idle-wake-poll" "displays sync")
runs=9
case ${1:-} in
  "") ;;
  --runs) runs=${2:?usage: $0 [--runs N]} ;;
  *) echo "usage: $0 [--runs N]" >&2; exit 2 ;;
esac
[[ $runs =~ ^[1-9][0-9]*$ ]] || { echo "--runs takes a positive number" >&2; exit 2; }

# idle-wake-poll sends real eval requests: only with the lid open and every display on.
safe_for_idle_poll() {
  local lids=(/proc/acpi/button/lid/*/state) lid text monitors
  [[ -e ${lids[0]} ]] || { why="lid state unknown"; return 1; }
  for lid in "${lids[@]}"; do
    text=$(<"$lid") || { why="lid state unreadable"; return 1; }
    [[ $text == *open* && $text != *closed* ]] || { why="lid not open"; return 1; }
  done
  monitors=$(timeout 5 hyprctl -j monitors 2>/dev/null) || { why="cannot read displays"; return 1; }
  jq -e 'length > 0 and all(.[]; .dpmsStatus)' <<<"$monitors" >/dev/null 2>&1 || { why="displays off"; return 1; }
}

# Prints "wall_us cpu_us" for one run of the verb, or aborts on a non-zero exit.
run_once() {
  local -a args
  read -ra args <<<"$1"
  local t0 t1 code=0 report
  report=$(mktemp)
  TIMEFORMAT='%3U %3S'
  t0=$(date +%s%N)
  { time "$bin" "${args[@]}" >/dev/null 2>&1; } 2>"$report" || code=$?
  t1=$(date +%s%N)
  if ((code)); then
    rm -f "$report"
    echo "power-hub $1 exited $code; aborting" >&2
    exit 1
  fi
  awk -v ns=$((t1 - t0)) '{printf "%d %d\n", ns / 1000, ($1 + $2) * 1e6}' "$report"
  rm -f "$report"
}

median() { sort -n | awk '{a[NR] = $1} END {print (NR % 2) ? a[(NR + 1) / 2] : (a[NR / 2] + a[NR / 2 + 1]) / 2}'; }

declare -A wall cpu
measured=()
for verb in "${verbs[@]}"; do
  if [[ $verb == idle-wake-poll ]]; then
    why=""
    safe_for_idle_poll || { echo "skipping idle-wake-poll: $why"; continue; }
  fi
  samples=$(for ((i = 0; i < runs; i++)); do run_once "$verb"; done)
  wall[$verb]=$(cut -d' ' -f1 <<<"$samples" | median)
  cpu[$verb]=$(cut -d' ' -f2 <<<"$samples" | median)
  measured+=("$verb")
done

# Medians are in microseconds; print milliseconds.
printf '%-16s%10s%10s\n' verb "wall ms" "cpu ms"
for verb in "${measured[@]}"; do
  awk -v v="$verb" -v w="${wall[$verb]}" -v c="${cpu[$verb]}" 'BEGIN {printf "%-16s%10.1f%10.1f\n", v, w / 1000, c / 1000}'
done
if [[ -v cpu[data] && -v cpu[gpu] ]]; then
  awk -v d="${cpu[data]}" -v g="${cpu[gpu]}" 'BEGIN {printf "5 s panel poll (data + gpu): %.2f%% of one core\n", (d + g) / 1e6 / 5 * 100}'
fi
if [[ -v cpu[idle-wake-poll] ]]; then
  awk -v w="${wall[idle-wake-poll]}" -v c="${cpu[idle-wake-poll]}" \
    'BEGIN {printf "1 Hz idle poll: %.2f%% client, up to %.2f%% wall (compositor not counted)\n", c / 1e6 * 100, w / 1e6 * 100}'
  echo "note: measured with displays on; a blanked session may differ"
fi
echo "peak memory is not measured here"
