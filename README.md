# Omarchy Power Hub

Omarchy Power Hub is a theme-aware power panel and bar widget for Omarchy.

| Plugged in | On battery |
|---|---|
| ![Power Hub panel plugged in, showing battery status, power plans, and charge limit](docs/images/panel-plugged-in.png) | ![Power Hub panel on battery, showing time remaining, power draw, and power plans](docs/images/panel-on-battery.png) |

I built this because every other power plugin I tried caused issues on my
Framework Laptop 16. It's the only plugin I'm aware of that works properly on
that machine.

It's developed and tested primarily on the Framework Laptop 16. Other laptops
are supported where they expose the required Linux controls. See
[CHANGELOG.md](CHANGELOG.md) for release history.

## ✨ Supported features

- **Battery status:** charge level, charging state, estimated time remaining,
  live power draw, battery health, charge cycles, and design capacity.
- **Battery history:** expandable 24-hour charge and discharge chart, with
  power readings when available.
- **Sleep drain tracking:** optional measurement of battery use during sleep.
- **Per-power-source plans:** separate settings for plugged-in and battery use:
  power mode, screen saver, displays-off timeout, auto-lock, sleep timeout,
  hibernate timeout, and lid-close action.
- **Display idle control:** turns off every Hyprland output, including external
  displays.
- **Travel mode:** temporarily reduces internal display brightness and refresh
  rate, and applies supported power-saving options. Restore the captured
  settings when turning it off.
- **Quick dim:** temporarily caps brightness and restores the prior level.
  Quick dim is unavailable while Travel mode controls the display.
- **Charging controls:** Framework charge limit and one-time 100% Boost, or a
  UPower battery-health toggle on hardware that exposes charge thresholds.
- **Power tweaks:** optional device power saving for Wi-Fi, PCIe, USB, audio,
  and display settings, depending on available tools and hardware.
- **GPU status:** shows whether a supported discrete GPU is asleep or awake.
- **Framework lighting:** controls supported fingerprint power button lights.
- **Keyboard brightness (Laptop 16 only):** Auto, Off, Low, Medium or High. Auto turns the light
  off in daylight or when another keyboard or receiver is connected. Otherwise
  it picks Low, Medium or High from the room light. A change with Fn+Space holds
  until the displays wake.
- **Lights through sleep:** keyboard and power button lights go off for system
  sleep and return at wake. A root hook installed with the helper does this.
- **Cleanup on removal:** removing the plugin deletes its root files, sudoers
  rules, sleep hook and user unit. It also restores the lid binding in
  `bindings.lua` and removes the login service. It needs the helper install
  from the panel. It keeps `~/.config/omarchy/power-hub*`,
  `~/.config/omarchy/omarchy-power-hub.json` and
  `/var/log/battery-plus-sleep.log`.
- **On resume:** updates Omarchy, plugins, lean-ctx, Rulesync, Skills Manager
  and T3 Code after wake, displays on or unlock. It restores T3 Code's windows
  when hypr-persist supports `--class`.
- **Bar widget:** battery icon and optional percentage, configurable for
  battery and plugged-in states.

The panel uses the active Omarchy theme colors. Features that depend on
hardware, firmware, or optional system services are shown only when available.

## 🚀 Install

```sh
omarchy plugin add https://github.com/JonathanPitre/omarchy-power-hub.git --enable
```

The plugin ships an x86-64 Linux binary and needs no Rust tools. On other CPUs,
the panel says it can't run. Its notices are in
[THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES).

Optional controls may need additional system tools. For Framework charge
limits, install:

```sh
omarchy pkg add framework-system
```

For the bar percentage, configure **Show percentage on battery** and **Show
percentage when plugged in** in the plugin settings. Right-clicking the bar
widget toggles the percentage for the current power state.

## 🔋 Charging and battery health

On Framework laptops, the charge limit uses `framework_tool --charge-limit`
to set the embedded controller ceiling. It does not use Linux start/stop
threshold files. The default desk-docked limit is 80%; the service reapplies
the saved limit after login if firmware resets it. Boost temporarily charges
to 100%, then restores the saved limit. A passwordless sudoers rule for the
command or a polkit prompt is required. In BIOS, set **Battery charge limit**
to match the plugin (80%) and turn off **Battery Extender**, which can reset the
same control to 100%; see [the longevity note](docs/framework-battery-longevity.md).

On other laptops, **Battery health** appears when UPower reports charge
threshold support. It uses the firmware's charge band when enabled and allows
charging to 100% when disabled. The section stays hidden when thresholds are
not supported.

Do not use this plugin alongside another charge-threshold plugin. Framework
Laptop 16 systems with a USB-C monitor may fail to charge after reconnecting
the adapter; see [the USB-C charging note](docs/framework-16-usb-c-replug-drain.md).

## 🧳 Travel mode and Quick dim

Travel mode caps the internal display at 40% brightness and selects a
supported refresh rate at or below 60 Hz without raising a display already
running slower. External displays are unchanged. It also selects Power saver
when power-profiles-daemon is available, and restores the previous power mode
when disabled. On AMD, that profile disables CPU boost and favors energy
saving. Without power-profiles-daemon, only display controls are changed.

Travel mode enables Panel power saving and Reduce effects if they were off,
then turns them back off when restoring. Plugging in turns Travel mode off;
unplugging does not turn it back on. Wi-Fi, peripherals, and GPU mode are not
changed. If a display setting cannot be restored, the panel keeps the restore
state and offers a retry.

Quick dim independently caps brightness at 40% and restores the captured
level when turned off. It cannot run at the same time as Travel mode because
both controls need to own the display restore state.

## 🛠️ System power controls

Power-saving controls depend on available hardware. The panel manages device
powersave settings itself, without TLP.
Power plans use Power saver, Balanced, or Performance where supported. New
plugged-in plans default to Performance; new battery plans default to
Balanced.

On a hybrid laptop whose panel is on the AMD iGPU, Omarchy can still point
VA-API at the NVIDIA card. Browser video then stays black, and the discrete
GPU stays awake. The panel only reports that state. See
[the hybrid GPU note](docs/hybrid-amd-nvidia-black-video.md).

Framework keyboard lighting uses `qmk_hid` on Laptop 16 and `framework_tool
--kblight` on Laptop 12 and 13. The fingerprint power button light uses
`/sys/class/leds/chromeos:white:power`, a root-owned device, written through
the installed helper.

## 💤 Hibernate and idle locking

Hibernate is unavailable while a process holds `memfd_secret`. Bitwarden
Desktop can do this after login; the panel reports the cause and falls back
to ordinary sleep. See [the hibernation note](docs/hibernate-memfd-secret-bitwarden.md)
for details and a workaround.

On Framework Laptop 16 with AMD Ryzen AI 300, firmware may move an ACPI region
during hibernation resume, causing the kernel to reject the saved image. The
plugin cannot correct this firmware behavior; see
[the Framework resume note](docs/framework-16-hibernate-resume.md).

Quickshell 0.3.1 can lose idle notifications when a running timer changes.
That may prevent automatic locking after displays turn off or wake. See
[the idle timer diagnosis](docs/quickshell-idle-timer-failure.md) for the
installed backport, rollback steps, and verification.

## 🔄 On resume

Power Hub can update your tools when the laptop wakes. Run it by hand with
the panel's Run now button. Settings live under `onResume` in
`~/.config/omarchy/omarchy-power-hub.json`.

| Setting | Default | What it does |
|---|---|---|
| `triggers` | `["wake"]` | Events that start a run: `wake`, `displays` or `unlock`. Opening the lid counts as `displays`. |
| `awayMinutes` | `30` | Minimum time away for `displays` and `unlock`. Wake always counts. |
| `everyHours` | `4` | Minimum time between runs, for every trigger. |
| `omarchy` | `true` | Runs `omarchy update -y`. |
| `plugins` | `true` | Runs `omarchy plugin update --yes`. |
| `leanCtx` | `true` | Runs `lean-ctx update`. |
| `rulesync` | `true` | Runs `mise up npm:rulesync`, only when `omarchy` is off. |
| `skillsManager` | `true` | Runs `skills-manager-refresh`, only when `omarchy` is off. |
| `t3code` | `"restart"` | `off`, `update`, or `restart`. |
| `restartShell` | `true` | Restarts the shell after a plugin update. |
| `sessionRestore` | `true` | Restores T3 Code's windows after it restarts. |
| `script` | `""` | A script to run instead of every built-in step. |

Steps run in this order and a failed step never stops the next one. Tools that
are not installed have no row. Omarchy update already covers Rulesync and
Skills Manager.

### 🔐 Sudo and the terminal

If a step needs sudo, the run opens a floating terminal titled `Stack Refresh`
and lists the steps. With `omarchy` on, it says
`Omarchy update installs system packages`, and `omarchy update` asks for your
password once in that terminal. With only Skills Manager, it says
`Skills Manager installs a system package` and asks for your password once
before the steps. Otherwise the run stays in the background.

### 📝 Log and notifications

The log is at `~/.local/state/omarchy-power-hub/resume.log`. A run sends one
notification with each from and to version, or one naming the failed tool.
Nothing is sent when everything is current. Power Hub never reboots on its
own: when an update needs one, the notification offers a Reboot button.

### 🤖 T3 Code

Power Hub reads `updateChannel` from `~/.t3/userdata/desktop-settings.json`.
If it is missing, a `-nightly` version means nightly and anything else means
latest. It installs a new build with `omarchy-install-t3code-app`.

With `restart`, it restarts T3 Code only when no thread is running or recently
updated. If that check fails, it falls back to `update` and sends
`T3 Code updated, restart it to finish`.

### 🪟 Session Restore

Before T3 Code quits, Power Hub runs `hypr-persist save power-hub-t3`. After
it relaunches, Power Hub runs
`hypr-persist restore power-hub-t3 --class ^t3code$`.

This needs a [hypr-session-restore](https://github.com/JonathanPitre/hypr-session-restore)
build whose `restore` command supports `--class`. Without it, the setting is
hidden.

## ⚡ Performance

Panel data and background polling run in a small Rust binary, `bin/power-hub`.
The bar, panel and idle service stay in QML. Saving settings, profiles, Travel,
Quick dim, Power Tweaks, display control, idle lock, screen saver launch, UCSI
recovery and the lid handlers run in the binary too. Nothing needs Python.

Python and Bash (0.0.53) compared with Rust:

| Path | Runs | Time (ms) | CPU (ms) | Processes | Peak memory (MB) |
|---|---|---|---|---|---|
| Idle check | Every second, displays off | 102 → 38 | 96 → 0.9 | 19 → 1 | 18.7 → 0.7 |
| Battery data | Every 5 s, 60 s with panel closed | 89 → 13 | 100 → 4.8 | 100 → 3 | 18.4 → 6.2 |
| Panel status | Panel open, after edits | 273 → 53 | 273 → 35 | 44 → 20 | 31.9 → 7.3 |
| GPU status | Every 5 s, panel open | 39 → 0.9 | 39 → 0.8 | 1 → 1 | 18.7 → 0.7 |
| Small commands | Config, boost check | 34 → 0.5 | 34 → 0.4 | 1 → 1 | 18.8 → 0.7 |

- The idle check drops from 9.6% to 0.09% of one CPU core while it runs. Its
  remaining 38 ms is mostly waiting on Hyprland.
- Polling with the panel open drops from 2.8% to 0.11% of one core.
- The NVIDIA GPU stayed asleep through six 60 s polling windows, three on each
  backend.

These figures come from one Framework Laptop 16, plugged in, on the Performance
profile, with the display on. Time and CPU are medians of 15 runs, and memory of
5. Peak memory is the largest single process, including tools a command starts.
Hyprland's own work is not counted.

Battery draw with the display on showed no clear change: a median of 18.0 W
before and 17.2 W after, across five 60 s windows each. That gap is within the
noise, and the idle check doesn't run in this test.

## 📚 Troubleshooting notes

- [Slow display lock and wake](docs/slow-display-lock-wake.md)
- [Cursor screensaver workaround](docs/cursor-screensaver-workaround.md)
- [Framework 16 USB-C charging after replug](docs/framework-16-usb-c-replug-drain.md)
- [Framework battery longevity: BIOS limit and Battery Extender](docs/framework-battery-longevity.md)
- [Framework 16 hibernate resume](docs/framework-16-hibernate-resume.md)
- [Hibernation blocked by `memfd_secret`](docs/hibernate-memfd-secret-bitwarden.md)
- [Quickshell idle timer failure](docs/quickshell-idle-timer-failure.md)
- [Hybrid AMD + NVIDIA: black video and a discrete GPU that stays awake](docs/hybrid-amd-nvidia-black-video.md)
