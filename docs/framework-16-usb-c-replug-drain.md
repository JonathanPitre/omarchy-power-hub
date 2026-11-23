# Framework Laptop 16 AMD: battery discharges after adapter replug with JSAUX FlipGo attached

Public firmware report for Framework. Omarchy Power Hub only reflects EC/UPower state; it cannot raise charger input above what the EC allows.

## GitHub

File firmware bugs on [SoftwareFirmwareIssueTracker](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker), not [Framework-Laptop-16](https://github.com/FrameworkComputer/Framework-Laptop-16) (CAD/models only).

This machine is tracked on **[issue #193](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/193)** (AI 300, 240 W EPR / charging). Details and a request for the test BIOS from @quinchou77 are in [comment #5830790592](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/193#issuecomment-5830790592) (2026-09-25).

That thread covers “stuck at 100 W SPR instead of 240 W EPR.” This setup adds **800 mA charger input after a dual-port charge switch** when a bus-powered USB-C monitor stays connected during brick hotplug.

## Summary

With a bus-powered USB-C monitor connected, unplugging the Framework **240 W** adapter and plugging it back into the same port can leave the laptop on AC while the battery **discharges**. The EC still reports the adapter as connected, but **Chg Input Current is stuck at 800 mA** (~16 W). System draw is ~30–48 W, so the pack covers the gap. Some replugs **hard-freeze** the machine (~10 s power-button reset).

Unplugging the monitor and reconnecting the adapter restores several amps input (~9 A seen) and charging. A 90% charge limit does not explain this; discharge continued from ~82% toward ~50% with the brick in.

On BIOS 4.02 the port often retries for 13-40 s and returns to 240 W EPR on its own, but it can also stay stuck for minutes or until power-off. See [BIOS 4.02](#bios-402).

## System (reporter)

| Item | Value |
| --- | --- |
| Product | Framework Laptop 16 (AMD Ryzen AI 300 Series) |
| Board | FRANMHCP09, version A9 |
| CPU | AMD Ryzen AI 9 HX 370 |
| GPU | NVIDIA GeForce RTX 5070 Laptop GPU |
| BIOS | 04.02, 2026-11-06 (DMI build date; released 2026-11-16) |
| EC | `tulip-4.0.2-d229205` (RO/RW), Nuvoton npcx9m3f, built 2026-10-30 |
| `framework_tool` | 0.6.5 |
| OS | Arch / Omarchy, Hyprland, kernel `7.2.15-1-omarchy` |
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
4. `sudo framework_tool --power` and `--pdports`. Failure: **800 mA** + **Battery discharging**. Poll once a second for a minute: on 4.02 the EC retries and can restore EPR without unplugging the monitor (13-40 s in two runs).

**Control:** disconnect FlipGo, then unplug/replug only the adapter → ~9 A input, charging.

## Workaround

Unplug the JSAUX FlipGo (or similar bus-powered USB-C display), then reconnect the Framework adapter. On 4.02, wait about a minute first: the EC retried for 13-40 s and then renegotiated 240 W EPR by itself (see [BIOS 4.02](#bios-402)). Do not unplug anything while it retries, and avoid hotplugging the brick with the FlipGo attached.

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

When the hero line shows **DISCHARGING** with the adapter in, that matches EC/UPower: mains present while `BAT1` is discharging. The plugin uses USB-C contract **≥15 W** for “AC” detection and applies the **battery** power profile while discharging to reduce replug freezes; it does **not** fix the 800 mA EC cap.

## BIOS 4.02

Flashed 2026-11-19. PD moved to `0.0.23`, the EC to `tulip-4.0.2-d229205`, and
the Input Module App to `0.3.17`. Three notes touch this code path, none names
the symptom:

- USB4 devices link down during data transitions or PCIe enumeration
- DP Alt Mode fallback negotiation for partners rejecting SVDM 2.1 structures
- EC crash from corrupted or malformed PD payload data

Nothing in the notes covers the 800 mA floor after a 4→2 charge-port switch,
EPR not coming back, or the replug freeze (`GPU HPD wake`).

Two replugs on 2026-11-19, FlipGo attached and never unplugged, both entered the
800 mA state and both recovered without help:

| Attempt | First `800mA` | Back at `9000mA` | Dip |
| --- | --- | --- | --- |
| 16:17 | 16:17:14 | 16:17:55 | ~40 s, four retry cycles |
| 16:27 | 16:27:44 | 16:27:57 | ~13 s, one retry cycle |

A retry cycle reads as one second at `800mA`, two at `10000mA`, then either
`2900mA` or a single `0mA` sample before the floor again. On 04.01 the floor
held until the monitor was pulled off the port. The EC log of the successful
attempt shows the floor and the recovery 1.5 s later:

```text
[6683.226500 Entered EPR]
[6683.332100 Updating charger with EPR correction: ma 490, 3lvl_buck ma 490]
[6683.339300 CL: p3 s0 i500 v20000]
[6684.089200 Updating charger with EPR correction: ma 9025, 3lvl_buck ma 4900]
[6684.093600 CL: p3 s0 i5000 v48000]
[6684.112100 board_set_active_charge_port port 3, prev:3]
[6684.119100 3lv-buck update! V:48000mV,W:240000mW]
```

The platform was throttled while the port retried: `PMF: SPL 28000mW,
fPPT 50000mW` during the dip, `SPL 45000mW, fPPT 71000mW` after EPR settled.
No freeze in either run.

4.02 cannot be rolled back to 04.01: the AMD AGESA PI SPL update blocks it.

Healthy state after the flash, brick and FlipGo attached:

- PD App `0.0.23` in backup and main on all three controllers (right, left, back)
- `Chg Input Current: 9000mA`, battery `Not charging` at the 80% ceiling
- framework_tool port 3: `48.000 V, 5000 mA, 240.0 W`, `EPR: Active (Supported)`, `Sink Active: Yes`
- Charge limit: `Minimum 0%, Maximum 80%`

framework_tool prints ports 0-3 here, while the EC log quoted above says `port 2` and `prev:4`. The two labels do not line up, so compare port maps by reading `--pdports`, not by EC port number.

### No recovery on 2026-11-21

On 4.02 the port did not always recover. The FlipGo stayed attached throughout. The timeline below comes from the UPower 1 Hz voltage history for `BAT1`:

| Time | `BAT1` | What happened |
| --- | --- | --- |
| 17:02:20 | Pending charge to discharging (16.725 V to 16.197 V) | Unattended, screen locked; no physical event known |
| 17:15:50 | Discharging to charging, 36-58 W | Adapter replugged by hand; FlipGo USB link reset |
| 17:17:38 | Charging to discharging | Adapter replugged by hand, same middle-left port |
| 17:18 to 19:33 | Discharging, 75% to 2% at ~17-35 W | Adapter seated; no AC, UCSI or typec event in the kernel log |
| 19:33:56 | 2% | UPower critical action: suspend-then-hibernate |
| 22:13 | 80% | Charged with the laptop off, after hibernation |

Input from the adapter was zero, not the 800 mA floor. In `s2idle` (suspend to idle) at 2%, the pack still lost 1.0 W over 302 s (`/var/log/battery-plus-sleep.log`). A 16 W input would have charged it. Only a full power-off restored charging. Charge control is ruled out: every `ops-priv charge-limit` call that boot was a read, and the limit was 80%.

The plugin now sends a critical notification when two checks 3 minutes apart both find the battery discharging while UCSI shows an adapter of at least 15 W, unless it is draining to the charge limit. No UCSI reading was captured on 2026-11-21, so it may miss a 0 W case where UCSI drops the adapter.

### Back and right ports on 2026-11-24

The adapter was on the back port, then the right port, and the FlipGo on the right controller (typec `port0`, backfeeding 9 V × 0.33 A). Several replugs on both ports kept the floor:

- `--power`: `Chg Input Current: 800mA`, battery discharging at 71%, about 37 W draw, limit 80%
- `--pdports` lists only the Right and Left controllers (ports 0-3), so it cannot show the back port or its EPR state; `Sink Active: No` there says nothing about the adapter
- UCSI read 20 V × 5 A on the back port during the floor, so UCSI shows the adapter online but cannot tell the floor apart
- Tell-tale without root: with the adapter in, the laptop's typec port for the FlipGo shows `power_role` `[sink]` instead of `[source]`

About 5 minutes in, the FlipGo's USB hub re-enumerated and the EC returned to `9000mA` with the FlipGo still attached.

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
