# Framework Laptop 16 AMD: battery discharges after adapter replug with JSAUX FlipGo attached

Public firmware report for Framework. Omarchy Power Hub only reflects EC/UPower state; it cannot raise charger input above what the EC allows.

## GitHub

File firmware bugs on [SoftwareFirmwareIssueTracker](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker), not [Framework-Laptop-16](https://github.com/FrameworkComputer/Framework-Laptop-16) (CAD/models only).

This machine is tracked on **[issue #193](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/193)** (AI 300, 240 W EPR / charging). Details and a request for the test BIOS from @quinchou77 are in [comment #5830790592](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/193#issuecomment-5830790592) (2026-09-25).

That thread covers “stuck at 100 W SPR instead of 240 W EPR.” This setup adds **800 mA charger input after a dual-port charge switch** when a bus-powered USB-C monitor stays connected during brick hotplug.

## Summary

With a bus-powered USB-C monitor connected, unplugging the Framework **240 W** adapter and plugging it back into the same port can leave the laptop on AC while the battery **discharges**. The EC still reports the adapter as connected, but **Chg Input Current is stuck at 800 mA** (~16 W). System draw is ~30–48 W, so the pack covers the gap. Some replugs **hard-freeze** the machine (~10 s power-button reset).

Unplugging the monitor and reconnecting the adapter restores several amps input (~9 A seen) and charging. A 90% charge limit does not explain this; discharge continued from ~82% toward ~50% with the brick in.

## System (reporter)

| Item | Value |
| --- | --- |
| Product | Framework Laptop 16 (AMD Ryzen AI 300 Series) |
| Board | FRANMHCP09, version A9 |
| CPU | AMD Ryzen AI 9 HX 370 |
| GPU | NVIDIA GeForce RTX 5070 Laptop GPU |
| BIOS | 04.01, 2026-06-12 |
| EC | `tulip-4.0.1-9b2a0b1` (RO/RW), Nuvoton npcx9m3f, built 2026-06-11 |
| `framework_tool` | 0.6.5 |
| OS | Arch / Omarchy, Hyprland, kernel `7.2.3-arch1-3` |
| Battery | NVT FRANDBA, 85.0 Wh design, ~67% health (3667 mAh LFCC), 62 cycles |
| Adapter | Stock Framework **240 W** USB-C + cable, middle-left, **EC port 2** |
| Monitor | **JSAUX FlipGo** (EDID FlipGo-A1 / FlipGo-A2, vendor RTK), USB-C only, **EC port 4** |
| Usage | Full laptop, standalone mode not enabled |

## Expected

After the 240 W adapter is reconnected, the EC should select the brick as the active charge port, draw tens of watts from the adapter (EPR **48 V × 5 A** when healthy), stop discharging, and charge or hold only at the configured charge limit.

## Actual

1. Adapter and FlipGo attached: charging or holding; input current several amps.
2. Unplug the adapter. Battery discharges. Linux may still report “on AC” because the FlipGo sources ~5 W back (~12 V × 0.42 A); ACPI `ACAD/online=1`.
3. Plug the adapter into the **same** middle-left port, FlipGo still attached.
4. UCSI may show **20 V × 5 A** on the brick port, but `framework_tool --power` reports **Chg Input Current: 800mA** and **Battery discharging**.
5. Some replugs freeze instantly; EC log shows **GPU HPD wake** during charge-port switch.

## Port roles

- **Port 2 (adapter):** sink. Healthy plug: EPR **48 V × 5 A (240 W)**. After bad hot replug: often **20 V × 5 A (100 W)** SPR, EPR inactive; EC input **800 mA**.
- **Port 4 (FlipGo):** sources ~5 W toward the laptop. Becomes active charge port while the brick is unplugged (`board_set_active_charge_port port 2, prev:4`).

Linux UCSI during the 800 mA state does **not** expose EPR PDOs on the brick port (partner source caps: 5/9/15/20 V SPR only). Trust **`framework_tool --pdports`** for negotiated EPR; the kernel often misreports EPR (see issue #193).

## Reproduce

Replug can freeze the machine. Be ready to hold the power button.

1. Boot with JSAUX FlipGo on USB-C and the 240 W adapter in the middle-left port.
2. `sudo framework_tool --power` — AC connected, `Chg Input Current` several amps (4900–9000 mA seen).
3. Unplug the adapter ~20 s, plug back into the **same** port; do **not** unplug the FlipGo.
4. `sudo framework_tool --power` and `--pdports`. Failure: **800 mA** + **Battery discharging**.

**Control:** disconnect FlipGo, then unplug/replug only the adapter → ~9 A input, charging.

## Workaround

Unplug the JSAUX FlipGo (or similar bus-powered USB-C display), then reconnect the Framework adapter. Avoid hotplugging the brick while the FlipGo is attached until firmware fixes the charge-port handoff.

## EC evidence

Healthy (`framework_tool --power`):

```
Charger Status
  AC is:            connected
  Chg Input Current:4900mA   # also seen at 9000mA
```

Broken, adapter physically in:

```
Charger Status
  AC is:            connected
  Chg Input Current:800mA
Battery Status
  AC is:            connected
  Battery discharging
```

Live confirmation **2026-09-25**, pack ~82% then falling:

```sh
sudo framework_tool --power | grep -E 'AC is|Input Current|discharging'
  AC is:            connected
  Chg Input Current:800mA
  AC is:            connected
  Battery discharging
```

800 mA is the **minimum** Tulip charger input (5 mΩ sense resistor; see Framework EC `fwk: tulip: grl pdo 5v/0A disable pd cfet`). After port switch 4→2, the charger IC stays at that floor instead of amps.

EC console around failing replug (`framework_tool --console recent`):

```
GPU HPD wake
CYPD_RESPONSE_PORT_CONNECT 2
P2 EPR mode capable
board_set_active_charge_port port 2, prev:4
AC off
Updating charger with EPR correction: ma 4900
CL: p2 s0 i5000 v20000
AC on
board_set_active_charge_port port 2, prev:2
3lv-buck update! V:20000mV,W:100000mW
```

Immediately afterward, `--power` still showed **800 mA** and discharging.

## Linux / UCSI (same moment)

| Node | Value |
| --- | --- |
| `BAT1/status` | Discharging |
| `BAT1/capacity` | 82% then 77% (later ~50%) |
| `BAT1` draw | ~42–48 W |
| `ACAD/online` | 1 |
| `ucsi-source-psy-USBC000:003` | 20 V × 5 A (brick) |
| `ucsi-source-psy-USBC000:005` | 12 V × 0.42 A (FlipGo) |

Kernel log (also seen when charging works; context only):

- `ucsi_acpi USBC000:00: unknown error 256`
- `ucsi_acpi USBC000:00: unknown error 0`
- `ucsi_acpi USBC000:00: UCSI_GET_PDOS failed (-5)`

FlipGo partner PDOs: 5 V/3 A, 9 V/560 mA, 12 V/420 mA.

## Omarchy Power Hub (plugin)

When the hero line shows **ON AC · DISCHARGING**, that matches EC/UPower: mains present (including FlipGo backfeed or brick) while `BAT1` is discharging. The plugin uses USB-C contract **≥15 W** for “AC” detection and applies the **battery** power profile while discharging to reduce replug freezes; it does **not** fix the 800 mA EC cap.

## Test BIOS

Asked @quinchou77 on issue #193 for the private Laptop 16 AI 300 test BIOS (checksum, flash, rollback). Waiting for a non-public channel. Will capture `--pdports`, `--pd-info`, `--power -vv`, and `--console recent` before/after if a build is provided.

## Capture commands

Omit serial numbers in public posts. EC commands need root.

```sh
sudo framework_tool --power
sudo framework_tool --power -vv
sudo framework_tool --pdports
sudo framework_tool --pd-info
sudo framework_tool --charge-limit
sudo framework_tool --console recent
cat /sys/class/dmi/id/product_name
cat /sys/class/dmi/id/bios_version
cat /sys/class/dmi/id/bios_date
cat /sys/class/chromeos/cros_ec/version
cat /sys/class/power_supply/BAT1/status
cat /sys/class/power_supply/BAT1/capacity
cat /sys/class/power_supply/ACAD/online
journalctl -k -b | grep -iE 'ucsi|cypd|typec'
```
