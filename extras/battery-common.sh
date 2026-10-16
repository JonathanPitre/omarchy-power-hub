# Shared battery probe for the battery-plus widget and battery-dashboard.
# Source this file, then call `battery_probe`. On success (return 0) it sets:
#   BAT_DIR      sysfs path of the battery
#   BAT_CAP      charge percent (int)
#   BAT_STATUS   Charging | Discharging | Full | Not charging | Unknown
#   BAT_ENOW     energy remaining      (µWh)
#   BAT_EFULL    energy at full charge (µWh)
#   BAT_EDES     design energy         (µWh)
#   BAT_PNOW     present power draw    (µW, always positive)
#   BAT_VNOW     voltage now           (µV)
#   BAT_CYCLES   charge cycle count (0 if unknown)
#   BAT_TEMP_C   battery temperature in °C ("" if unknown)
#   AC_ONLINE    1 if an AC adapter is plugged in, else 0
# Batteries that expose charge_* (µAh) instead of energy_* are converted
# using the design voltage, matching what upower reports.

_bg() { cat "$BAT_DIR/$1" 2>/dev/null || echo "${2:-0}"; }

battery_probe() {
  BAT_DIR=""
  local d
  for d in /sys/class/power_supply/BAT*; do [ -d "$d" ] && BAT_DIR="$d" && break; done
  [ -n "$BAT_DIR" ] || return 1

  BAT_CAP=$(_bg capacity)
  BAT_STATUS=$(_bg status Unknown)
  BAT_VNOW=$(_bg voltage_now)
  BAT_CYCLES=$(_bg cycle_count)

  local vdes
  vdes=$(_bg voltage_min_design "$BAT_VNOW")
  [ "${vdes:-0}" -gt 0 ] || vdes=$BAT_VNOW

  local enow efull edes pnow
  enow=$(_bg energy_now); efull=$(_bg energy_full); edes=$(_bg energy_full_design)
  pnow=$(_bg power_now)

  if [ "${enow:-0}" -eq 0 ] && [ "${vdes:-0}" -gt 0 ]; then
    local cnow cfull cdes
    cnow=$(_bg charge_now); cfull=$(_bg charge_full); cdes=$(_bg charge_full_design)
    enow=$((  cnow  * vdes / 1000000 ))
    efull=$(( cfull * vdes / 1000000 ))
    edes=$((  cdes  * vdes / 1000000 ))
  fi

  if [ "${pnow:-0}" -eq 0 ] && [ "${BAT_VNOW:-0}" -gt 0 ]; then
    local cur
    cur=$(_bg current_now); [ "${cur:-0}" -eq 0 ] && cur=$(_bg current_avg)
    cur=${cur#-}
    pnow=$(( cur * BAT_VNOW / 1000000 ))
  fi
  pnow=${pnow#-}

  BAT_ENOW=$enow
  BAT_EFULL=$efull
  BAT_EDES=$edes
  BAT_PNOW=$pnow

  BAT_TEMP_C=""
  local t
  t=$(_bg temp)
  if [ "${t:-0}" -gt 0 ]; then
    # ACPI battery temp is tenths of a degree (either K or C depending on firmware)
    if [ "$t" -gt 2000 ]; then
      BAT_TEMP_C=$(awk -v x="$t" 'BEGIN{printf "%.0f", x/10 - 273.15}')   # deci-Kelvin
    else
      BAT_TEMP_C=$(awk -v x="$t" 'BEGIN{printf "%.0f", x/10}')            # deci-Celsius
    fi
  fi

  AC_ONLINE=0
  local a
  for a in /sys/class/power_supply/A{C,DP}*/online; do
    [ -r "$a" ] && AC_ONLINE=$(cat "$a" 2>/dev/null) && break
  done
  return 0
}

# minutes until empty (Discharging) or full (Charging); empty string if unknown
battery_minutes() {
  [ "${BAT_PNOW:-0}" -gt 0 ] || return 0
  case $BAT_STATUS in
    Discharging) awk -v e="$BAT_ENOW"  -v p="$BAT_PNOW" 'BEGIN{printf "%d", e/p*60}' ;;
    Charging)    awk -v e="$BAT_EFULL" -v n="$BAT_ENOW" -v p="$BAT_PNOW" 'BEGIN{printf "%d", (e-n)/p*60}' ;;
  esac
}
