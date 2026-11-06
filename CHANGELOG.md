# Changelog

All notable changes to this project are documented in this file.

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Clarify that the BIOS charge limit should match the plugin value and that
  Battery Extender can override it.

## [0.0.53] - 2026-11-06

### Fixed

- The NVIDIA GPU on hybrid laptops stayed awake about a third of the time while
  idle, which costs battery. Every status poll ran `lspci` to look for a
  Broadcom Wi-Fi chip, and `lspci` wakes a sleeping GPU. The check now reads the
  kernel's cached device list, so the GPU stays asleep.

## [0.0.52] - 2026-11-05

### Added

- **Panel power saving** in Power tweaks, AMD's adaptive backlight, shown on
  machines that support it.
- **Reduce effects** in Power tweaks, which turns off Hyprland's animations
  and shadows and restores your own values afterwards.
- Travel mode turns both on for its duration. It restores only what it turned
  on, so a tweak you enabled yourself stays on.
- `scripts/measure-draw.sh` measures battery draw in repeated windows and logs
  the conditions behind each number.

### Changed

- The plugin is now **Omarchy Power Hub**, with the ID `omarchy-power-hub`.
  Config, state, helper, sudoers, sleep, and service names follow the new ID.
- The status line reads `DISCHARGING` on battery, matching `CHARGING`.
- Pinning any Power tweak also reapplies it when the shell starts.
- The Screen saver menu offers only times before Displays off. If Displays off
  moves past it, Screen saver shows Never and your saved time returns when
  Displays off moves back.

### Removed

- The 2-second idle poll, which started a Python process and a login shell
  even with the panel closed. The lid state already comes from the wake poll
  while the displays are off, and the screensaver check now follows window
  events.

### Fixed

- Displays off turned the screens back on about a second later and left them
  on until the next input. The wake poll compared the cursor with its position
  from the previous blank and read the difference as motion.
- Displays off could delay Auto-lock by closing the screen saver, which Omarchy
  treats as the user returning. Auto-lock now keeps its time.
- Unplugging with a bus-powered USB-C monitor attached left the Plugged in
  power mode active, because UPower's `OnBattery` never changed. The plugin
  now also follows the battery's own state.
- Pinned Power tweaks showed as unpinned. The panel read a TLP file, while
  pins are saved in `power-settings-tweaks.json`.

## [0.0.51] - 2026-11-04

### Changed

- Plugging the adapter back in turns Travel mode off. It keys on the battery
  leaving Discharging, so a bus-powered USB-C monitor that keeps AC online
  does not matter, and a Travel session started while plugged in stays on.
  Travel does not come back on by itself when you unplug again.
- Travel mode rows read Max brightness and Max refresh rate instead of
  Brightness cap and Refresh cap.

### Fixed

- Right after unplugging, the time remaining read hours too long (7 h
  instead of 2.5 h) and then fell. The embedded controller smooths battery
  current over about 8 s, so a 28 W load first reads 10 W. For 30 s after a
  plug or unplug the status line now reads `CALCULATING…`, and the bar
  tooltip says calculating.
- A disabled Dim screen button still looked live: with Quick dim on and
  Travel mode then enabled, `Restore` kept its selected fill and full label.
  Disabled buttons now grey out completely.

## [0.0.50] - 2026-11-04

### Changed

- The status line under Battery now carries the time estimate, such as
  `BATTERY · 3h 48m REMAINING` or `CHARGING · FULL IN 1h 05m`. The Time left
  cell is gone, so plugged-in states no longer show a `—`.
- In that status line the time is white and the state stays dimmed, so the
  estimate stands out.
- Power draw shows only while the battery is charging or discharging. While
  charging it reads Charging at, with the charge rate in watts.
- Battery health is a 2×2 grid of tiles: Capacity and Charge limit, then
  Charge cycles and Health. Cycles is now Charge cycles, and Charge limit
  shows the active cap (`80%`, `Off`, `Auto`, or `100% (boost)`).
- Capacity and Health turn the theme's yellow below 80% and its urgent
  colour below 60%.

### Removed

- The Reduce CPU switch in Travel mode. Travel mode now always switches to
  Power saver without a separate row for it, which on AMD already turns per-CPU boost off and sets the
  energy-saving CPU preference. The switch's extra step, global boost off,
  saved nothing and broke power-profiles-daemon: with it off, amd_pstate
  rejects every profile change (`policy11/boost: Invalid argument`), so
  Restore settings failed. A Travel session already started with Reduce CPU
  still restores; boost is turned back on before the profile. The `C`
  shortcut and `travel on --reduce-cpu` are gone.

### Fixed

- Unplugging the adapter with a bus-powered USB-C monitor attached showed
  `FULLY CHARGED` or a bare `DISCHARGING` with no time. The monitor keeps
  `ACAD` online, so a discharging battery now wins over it and the status
  reads `DISCHARGING · 4h 32m REMAINING`.
- Time remaining and Power draw lagged changes such as Travel mode or Dim
  screen, and stayed blank for about 90 s after unplugging.
  They now come from the panel's 5 s battery poll instead of UPower, which
  refreshes only every 30 s or so.
- The panel repolls the battery as soon as the adapter is plugged in or
  unplugged, so the time appears within a second or two.
- Right-click on the bar battery hid the percentage only when plugged in. It
  now hides or shows it for the current power state, with a new
  **Show percentage on battery** setting beside **Show percentage when plugged
  in**. On battery follows UPower's live state, so a bus-powered USB-C monitor
  no longer makes the bar treat a draining battery as plugged in.
- The battery glyph showed full from 90% to 99%, both in the bar and in the
  panel. Each glyph now rounds down, so full means 100% and 95% shows the 90%
  glyph. The bar tooltip says remaining instead of left.
- Travel mode lost Power saver on every plug, unplug or shell restart and
  showed a bare `balanced` in red. Omarchy's battery service runs
  `omarchy-powerprofiles-set` on each power-source change, which reapplied
  the remembered AC or battery profile over Power saver. Travel now holds
  both remembered profiles at `power-saver` and writes your normal ones back
  when it turns off. A failed check now reads `Power saver was replaced by
  balanced`.
- The bar left a wide gap between the battery glyph and its percentage. The
  percentage is now a plain label with one space after the glyph, like the
  bar's other battery readouts.
- Tests no longer write to the real
  `~/.local/state/omarchy/powerprofiles` files.

## [0.0.49] - 2026-11-03

### Changed

- Redesigned Charge limit as a − 80% + stepper with a Boost button. The
  Battery Extender note moved into an info tooltip, and the Desk use line is
  gone.
- Buttons get the theme's resting fill and an accent outline on hover and
  when on. The Boost bolt uses the theme's yellow, and − and + dim while
  boosting or at the 40% and 100% ends.
- Travel mode is a plain section with Brightness cap, Refresh cap and a
  Reduce CPU switch. Its helper text moved into info tooltips, and Dim screen
  shares its action row. Boost and Reduce CPU have info tooltips too.
- Switches show the accent outline on hover and when on. Sleep stats put each
  label above its value, Power tweaks use shorter copy, and the scrollbar is a
  thin themed bar in the panel margin instead of over the content.
- The GPU section shows only Asleep or Awake for the discrete GPU, read from
  `/sys`. It no longer calls `nvidia-smi`, which could wake the card it was
  watching.

### Removed

- The DEVICES section and its Logitech (Solaar) and SteelSeries
  (HeadsetControl) battery readings.
- The first-run `omarchy pkg add solaar headsetcontrol` install.

### Fixed

- Charge limit showed `sudo: a password is required` and hid its controls. It
  now reads the limit through the plugin's helper and offers Allow when the
  helper is missing.
- The limit was read as 0% because the parser took the minimum from
  `framework_tool` output. The saved limit is also reapplied through the
  helper again after the EC drifts from it.
- Low-battery red, Travel warnings, and accent colors were `undefined`
  because bare `Color` does not resolve to the theme inside plugins.

## [0.0.48] - 2026-11-03

### Fixed

- Moving the mouse right as the screensaver appeared, or while it was
  appearing, did not dismiss it. You had to stop, then move again or press a
  key. The screensaver now waits for its windows to settle and dismisses on
  input that is still under way, and its own launch no longer closes it.

## [0.0.47] - 2026-10-24

### Removed

- Removed the CPU section, including Turbo boost and the four energy preference
  buttons. Travel mode retains the Reduce CPU performance option.
- Removed Top consumers and its process polling. CPU usage did not measure
  battery drain per process.

### Fixed

- Aligned device battery glyphs and bars across rows with different percentage
  widths, such as 90% and 100%.

## [0.0.46] - 2026-10-04

### Fixed

- A failed Travel enable could change a power profile it had never captured,
  while a monitor-discovery race could leave Quick dim with an unusable
  restore point. Rollback now leaves untouched profiles alone, and Quick
  dim acquires ownership only after identifying the internal display.


## [0.0.45] - 2026-10-04

### Added

- Battery work away from an outlet had no reversible laptop-saving preset.
  Travel mode now caps internal brightness at 40%, selects a supported
  refresh rate no higher than 60 Hz, and restores the captured settings
  without changing external displays. CPU power reduction is an explicit
  opt-in; unsupported controls are left alone and failed restores remain
  retryable. Quick dim is independently reversible and unavailable while
  Travel mode owns the display.

## [0.0.44] - 2026-10-03

### Changed

- DEVICES used the generic headphones glyph for a headset, so the row
  looked like speakers on a band instead of a headset. It now uses the
  same Nerd Font headset icon as gadget-batteries.

## [0.0.43] - 2026-09-28

### Changed

- The headset in DEVICES now updates within about a second of plugging it
  in or out while the panel is open. HeadsetControl answers in under 0.1 s,
  but the list reused the 5 s laptop refresh interval, so a cable change
  sat unseen until the next tick. It now polls on its own 1 s timer; the
  laptop settings keep the slower interval.

## [0.0.42] - 2026-09-28

### Changed

- Plugging a Logitech mouse or keyboard in or out now changes its DEVICES
  icon within about a second while the panel is open, instead of only after
  a 10 s re-scan. Every reading used to come from `solaar show`, which walks
  every feature and setting of every device over the wireless link. The
  panel now asks each device only for its battery through Solaar's library
  (about 0.2 s), then listens read-only on the receiver for the battery
  events the device sends itself. A device that stays silent is re-read at
  the panel refresh interval, and devices without Solaar's unified battery
  feature still use `solaar show`.
- The headset's battery is polled at the panel refresh interval while the
  panel is open, and no longer waits behind the Logitech read, so plugging
  it in or out shows up within a few seconds.
- Re-scan now re-identifies every Logitech device, which picks up a device
  re-paired into another slot.

## [0.0.41] - 2026-09-28

### Fixed

- A peripheral charging over USB-C, such as an MX Keys Mini, kept the plain
  battery glyph in DEVICES even though Solaar showed it charging. The
  plugin read only the percent from `solaar show` and dropped the
  `BatteryStatus` beside it, so every device looked like it was on battery.
  Devices now show the same charging and full glyphs as the laptop
  battery, from Solaar's status and HeadsetControl's `BATTERY_CHARGING`.
- The DEVICES refresh button reused a `solaar show` reading up to 60 s
  old, so plugging a device in and re-scanning showed nothing new. It now
  always reads fresh; opening the panel still reuses the recent reading so
  it doesn't wait on Solaar.

## [0.0.40] - 2026-09-28

### Fixed

- Choices in the Screen and sleep grid (When lid closes, the idle timers,
  Power mode) showed in the dropdown but were never saved, and reopening
  the panel brought the old value back. The panel skipped the save when
  the pick matched the dropdown's own value, which Omarchy's dropdown has
  already updated by then. It now compares against the saved setting, so
  a lid set to Displays off actually turns the screens off on close.
- Opening the lid after Displays off now brings back monitors that were
  switched off rather than blanked, such as heads that drop off the bus
  on DPMS. A key press only wakes blanked screens, so those stayed dark.
- Closing the lid with Displays off no longer crashes Hyprland. The FlipGo
  heads sit on the NVIDIA GPU; switching them off as outputs left that GPU
  with nothing to draw, and 0.56 exited to Safe Mode. Lid close now blanks
  with DPMS only, and does not disable a head that reconnects while the lid
  is still shut.

## [0.0.39] - 2026-09-28

### Changed

- Draw is now "Power draw" and stays on the grid when plugged in, showing
  "—" like Time left. Charge rate is not system power, so AC never
  pretends it is.
- The battery-usage chart is a continuous 15-minute battery-level graph
  with 50% and 100% lines and "24h ago" / "Now" labels. The previous
  version only drew hours where UPower logged a change, so a laptop held
  at the charge limit looked empty.

### Removed

- Temp and the Framework EC sensor list. Windows and macOS do not show
  temperature on the battery page, and system monitors already cover it.

## [0.0.38] - 2026-09-28

### Changed

- DEVICES refresh is an icon. The labeled button crowded the header;
  hover now reads "Re-scan peripheral batteries".

## [0.0.37] - 2026-09-28

### Changed

- POWER & BATTERY sits under DEVICES, then charge limit, travel, and GPU.
  Opening the panel no longer hides DEVICES until a scan finishes, which
  used to grow the popup and close it when a keyboard or mouse battery
  actually appeared. Refresh on that header re-scans peripheral batteries
  without reloading the laptop plan.

## [0.0.36] - 2026-09-28

### Added

- Framework laptops can expand Temp to the embedded-controller sensors
  (APU, charger, ambient, GPU). The (i) only appears when those sensors
  exist.

### Changed

- POWER DRAW is now BATTERY USAGE: a 24-hour battery-level chart from
  UPower, collapsed until you open it. Windows and macOS graph the
  battery, not live watts, and the old sparkline only filled while the
  panel was open, so it was a tall duplicate of Draw.

### Fixed

- Draw and POWER DRAW showed the same watts because CPU RAPL is
  root-only on this hardware, so the header fell back to the battery
  figure. The header now stays empty unless RAPL is actually readable.
- Temp was always "—" on AMD: the probe only knew Intel's
  `x86_pkg_temp` zone. It now reads `k10temp` Tctl, and hides the row
  when nothing reports.
- Draw on AC was the battery charge rate, so it read 0 W when full. It
  is labeled Charging while charging and hidden when full or not
  charging.

## [0.0.35] - 2026-09-28

### Fixed

- Keyboard and mouse batteries vanished from DEVICES. The idle-wake poll
  named a local `devices`, so Python treated the DEVICES command's
  `devices()` call as that same name and crashed before it could read
  Solaar. The panel now lists them again.

## [0.0.34] - 2026-09-28

### Changed

- The grid is titled "POWER & BATTERY". Sleep is "Sleep after", matching
  Windows 11's "Hibernate after". Both lists are the Windows screen/sleep
  set: Never, 1–5, 10, 15, 20, 25, 30, 45 minutes, then 1–5 hours, instead
  of Sleep stopping at 30 minutes and Hibernate jumping from 15 minutes to
  12 hours.

## [0.0.33] - 2026-09-28

### Fixed

- Power mode labels were clipped ("Power sav…") because each dropdown was
  118 px and the icon sat in the same string as the name. The columns and
  the panel are wider so Power saver, Balanced and Performance read in
  full, with the icon still in front.

## [0.0.32] - 2026-09-28

### Changed

- Power mode dropdowns show the Power saver, Balanced and Performance icons
  again. Omarchy's dropdown has no icon slot, so the glyph sits in the label.

## [0.0.31] - 2026-09-28

### Changed

- Power mode (Power saver / Balanced / Performance) is the first row of the
  Power and battery grid, with one dropdown per source. The old pair of
  three-button rows is gone, so you no longer set profile in one place and
  sleep timers in another.

## [0.0.30] - 2026-09-28

### Changed

- The idle grid is titled "Screen and sleep", the Windows 11 name for this
  group, now that "When lid closes" sits in the same table. Plugged in is
  the first column, with the plug and battery icons back on the headers.
  The source in use is bold; the extra "NOW" label is gone. "Hibernate
  after sleep" is shortened to "Hibernate after" so it fits beside the
  dropdowns.

## [0.0.29] - 2026-09-28

### Changed

- Idle timers are dropdowns now, laid out like Windows 11: one row per
  setting, with Battery and Plugged in side by side, so you no longer flip a
  toggle to see the other source. The fixed Off/1m/5m/10m/30m buttons
  couldn't hold a 2 or 3 minute timer. The list now offers Never, 1, 2, 3, 5,
  10, 15, 20 and 30 minutes. Hibernate after sleep offers 15 minutes to 12
  hours instead, since a one-minute hibernate delay isn't useful. Options
  read "5 minutes" and "Never", as on Windows and macOS. A saved value that
  isn't on the list, such as the 150 s screen saver default, shows as its
  own entry ("2 min 30 s") and stays until you pick another.
- The lid action joins the same grid as "When lid closes". The "Default"
  choice is gone, and both sources default to Sleep, as on Windows 11. A
  config that still says "Default" reads as Sleep, which is what logind did
  for it anyway.

## [0.0.28] - 2026-09-28

### Added

- README and docs for two hibernate failures: Bitwarden’s `memfd_secret`
  (Linux-wide) and Framework 16 AI 300 resume that cold-boots after an E820
  mismatch. The Bitwarden note uses `~/.config/environment.d/` for mlock;
  Bitwarden overwrites its autostart `.desktop` on every launch.

### Changed

- When hibernation is off, the idle plan says why: Bitwarden holding secret
  memory, kernel lockdown, `nohibernate`, or missing swap/`resume=`, instead
  of “not available on this system.”

### Fixed

- Idle sleep on battery never ran while hibernation was off. The plan asked
  for hibernate-after-sleep, so the service called
  `suspend-then-hibernate`, systemd refused, and the laptop stayed awake.
  Idle sleep and lid hibernate now fall back to plain suspend.

## [0.0.27] - 2026-09-28

### Added

- One fingerprint prompt the first time the service starts (and again only
  after an update changes the helper) installs a root-owned copy of the
  privileged helper plus a passwordless sudo rule for that copy alone.
  Hibernate delay, lid wake-up devices, device tweaks, charge limit and
  keyboard backlight stop asking for your password or fingerprint. The rule
  never covers the plugin folder, which your user can edit.

### Changed

- Idle timer titles use the same units as their buttons: "Sleep · 5m",
  "Screen saver · Off", and "2m 30s" for a value no button matches, instead
  of raw seconds.
- Plugging in or unplugging now switches the screen saver, auto-lock,
  hibernate delay and lid action to that source's settings right away, not
  just the power profile; the idle timers follow within a second instead
  of up to 30 s.

### Fixed

- Idle timer buttons lagged, sometimes for 10+ seconds, and occasionally
  froze the shell. Each click re-applied every setting on the active
  source: it rewrote shell.json (which makes the whole shell reload),
  re-set the power profile, and re-ran the hibernate delay and HID wake-up
  helpers through pkexec. It then waited on a full status read that
  includes `solaar show` (11 s once its one-minute cache expires). Now a
  click highlights immediately, changes only what that timer controls,
  skips writes that would change nothing, and never waits on the Logitech,
  headset or GPU readout. Rapid clicks are queued with the last one
  winning, instead of being silently dropped.

## [0.0.26] - 2026-09-28

### Added

- Charge health toggle on laptops where UPower exposes charge thresholds
  (75–80 or firmware LongLife). Off charges to 100% for travel.
- Framework: re-apply the saved charge ceiling after login when firmware
  forgot it. Default remains 80% for desk-docked use.

### Changed

- Charge Limit is Framework-only (EC). Other machines no longer show
  “not a Framework machine”; they get the health toggle or nothing.
- Framework panel notes Battery Extender should stay off with a custom
  ceiling.

## [0.0.25] - 2026-09-28

### Added

- Idle source buttons show a plug on Plugged in and a battery on Battery,
  matching the icons already used on the profile chips.

## [0.0.24] - 2026-09-28

### Changed

- One glossary for power-source and screen-off labels: idle tabs are
  Plugged in / Battery, plan titles and the hero use PLUGGED IN / BATTERY,
  lid Displays off matches the idle timer, lid Do nothing, and the profile
  chip is Power saver. Config keys are unchanged.

## [0.0.23] - 2026-09-28

### Fixed

- Changing an idle timer on the On battery tab no longer jumps the editor
  back to Plugged in. After each save the panel reloaded `status` and
  copied the live power source onto the idle tab, so a laptop on AC always
  snapped back. The tab still follows the live source until you click
  Plugged in or On battery; after that it stays put across reloads.

## [0.0.22] - 2026-09-27

### Fixed

- Mouse missing from DEVICES after a reboot. At login the Solaar tray
  starts in the same second as this plugin, and both talk to the Bolt
  receiver at once. The mouse is read first, so it comes back with a
  blank or codename-only name, or `solaar show` crashes partway through
  it, sometimes with exit code 0. The device list lived in
  `$XDG_RUNTIME_DIR`, which a reboot wipes, so there was no earlier list
  to fall back on, and the panel read the device list only once at
  startup.
  - Runs that print a traceback or return null bytes no longer replace the
    cached list, whatever their exit code.
  - The last good list is kept in
    `$XDG_STATE_HOME/omarchy-power-settings/solaar.json`, so it survives a
    reboot. The lock stays in `$XDG_RUNTIME_DIR`.
  - Opening the panel reloads DEVICES, so it picks up the background
    poll's latest reading.

## [0.0.21] - 2026-09-27

### Fixed

- The Omarchy screensaver no longer vanishes about a second after it
  appears when you have not touched the mouse or keyboard. The service
  had been polling `hyprctl cursorpos` every second while the overlay was
  up. Omarchy hides the pointer on the saver, so the position often
  stuck at one coordinate, and the multi-monitor launcher's focus hops
  looked like movement and called `pkill` on the saver. Screensaver wake
  now uses a Quickshell `IdleMonitor` with `respectInhibitors: false` so
  only real pointer activity dismisses it, with the cursor still hidden.
  The cursor-position poll runs only for displays-off and parked heads,
  not during the screensaver.

### Added

- The screensaver timeout from this panel now fires even when a browser
  or other app holds a Wayland idle inhibitor (for example YouTube playing
  in Brave). Omarchy's idle service still honors inhibitors; this plugin
  starts `omarchy-launch-screensaver` when your configured idle time is
  reached and an inhibitor is active, unless stay-awake is on or the
  session is locked. With no inhibitor, Omarchy continues to launch the
  saver as before so two launches do not race.

## [0.0.20] - 2026-09-27

### Changed

- Panel section order is now Devices, Power Profile, Charge Limit, Travel,
  GPU, Idle, Lid. Battery stats and power draw stay on top.
- Idle timers are labeled IDLE (was POWER PLAN).
- POWER TWEAKS (was DEVICE POWER SAVING) sits after SLEEP.

### Removed

- PowerTOP scan and Full dashboard footer buttons.

## [0.0.19] - 2026-09-27

### Added

- Pointer movement wakes the screensaver, displays-off, and suspend when
  the lid is open (same idea as Windows 11 / macOS).

### Changed

- With the lid closed, keyboard and pointers do not wake (travel). Opening
  the lid and the power button still do.
- DEVICES names drop the manufacturer so they stay short: “MX Keys Mini”,
  “MX Anywhere 3S”, “Arctis Nova Pro Wireless”.

## [0.0.18] - 2026-09-27

### Changed

- DEVICES rows: kind icon, name, then bar, percent, and battery icon as one
  group (no gap between percent and glyph).

## [0.0.17] - 2026-09-27

### Fixed

- Keyboard and mouse intermittently missing from DEVICES. Each bar's panel
  and the service ran `solaar show` at the same time; overlapping runs on
  the Bolt receiver fail (`FeatureCallError`, `TypeError`) and return no
  devices or only some. Solaar now runs one at a time behind a lock in
  `$XDG_RUNTIME_DIR/omarchy-power-settings`, the result is shared for 60 s,
  and a failed or partial run keeps the last good list.

### Changed

- Corrected 0.0.14–0.0.16: the causes named there (Repeater dropping a
  row, ListView height, `mouse` as a reserved role) were not the problem;
  the overlapping Solaar runs above were.

## [0.0.16] - 2026-09-27

### Changed

- Device kind for mice is `pointer` in the status JSON. Row order is kind
  icon, name, bar, percent, battery glyph.

## [0.0.15] - 2026-09-27

### Changed

- DEVICES rows use a Column Repeater instead of a ListView.

## [0.0.14] - 2026-09-27

### Fixed

- DEVICES lists keyboard, then mouse, then headset. Solaar enumerates the
  mouse first (`1: MX Anywhere 3S`), so rows are now sorted by kind.

## [0.0.13] - 2026-09-27

### Fixed

- DEVICES no longer walks UPower (laptop pack plus junk like FRANDBA, and
  a fake **Arctis 1%** from HeadsetControl’s “Found 1 device” line). It
  reads Logitech batteries from `solaar` and headset batteries from
  `headsetcontrol`, and hides a row until that tool returns 0–100%.

### Added

- DEVICES rows: kind icon, name, percent, battery glyph, and a thin bar.
- First load installs `solaar` and/or `headsetcontrol` with `omarchy pkg
  add` only when matching USB hardware is present (`046d` Logitech,
  `1038` SteelSeries) and the binary is missing.

## [0.0.12] - 2026-09-27

### Fixed

- Displays-off kills the Omarchy screensaver and pauses the saver timeout
  until wake. Coming back after blank shows dark panels, not the overlay
  that used to keep running on the laptop while the FlipGo heads dropped.

## [0.0.11] - 2026-09-26

### Fixed

- DEVICE POWER SAVING (i) buttons show each tweak’s full help on hover
  instead of the generic “What does this do?” prompt. Click still expands
  the same text plus **Keep after reboot**.

## [0.0.10] - 2026-09-26

### Changed

- Plan chips use the stock Omarchy button: outline when idle, filled when
  that option is current (same as Installed / Browse). Plugged in / On
  battery no longer append `·`. One-shot actions stay outline-only.

## [0.0.9] - 2026-09-26

### Fixed

- Waking displays after idle blank kills the Omarchy screensaver *before*
  the panels are re-enabled, so the first mouse or key should not flash the
  overlay. A locked session stays locked; only the saver process is stopped.

## [0.0.8] - 2026-09-25

### Changed

- The battery hero line names the source and the pack together:
  **ON BATTERY · DISCHARGING**, **ON AC · CHARGING**, **ON AC ·
  DISCHARGING**, or **ON AC · NOT CHARGING**. Watts stay on the Draw row.

### Fixed

- A USB-C source only counts as AC at 15 W or more. A JSAUX FlipGo (and similar
  bus-powered monitors) can feed ~5 W back into the laptop. ACPI `ACAD` and
  UPower treat that as mains, so the panel said ON AC and applied the AC
  power plan while the pack was draining. Detection now uses the USB-C
  contract watts, not `ACAD`.
- While `BAT1` is **Discharging**, the service applies the **battery**
  power profile instead of AC performance. On this Framework 16, unplug
  and replug of the brick with a JSAUX FlipGo attached left the EC charger input
  at 800 mA. Omarchy’s stock battery service still saw UPower “on AC” and
  switched to performance, which can freeze the machine. This plugin only
  changes the profile (no Hyprland reload). It cannot raise the 800 mA
  cap. That is firmware. Public Framework report:
  [docs/framework-16-usb-c-replug-drain.md](docs/framework-16-usb-c-replug-drain.md).
- The bar battery icon no longer disappears after a shell restart.
  `qs.Ui.Panel` has no `vertical` property. Binding `showBarPercent` to a
  bare `vertical` threw, `slotSize` became NaN, and the widget collapsed.

## [0.0.7] - 2026-09-25

### Changed

- The bar no longer always prints the battery percent next to the icon.
  On AC it is icon-only. On battery it still shows `N%`. Right-click, or
  the widget setting **Always show percentage (including on AC)**, keeps
  the number on both sides. The old `showPercentage` key is ignored.
  Hover and the open panel still show percent.

## [0.0.6] - 2026-09-25

### Added

- Displays off and lid **Blank screens** now turn off the Framework keyboard
  backlight and the fingerprint power-button light, then restore both when
  the screens wake. The product name decides the path. Laptop 16 keys are a
  USB module, so the plugin uses `qmk_hid via --backlight` and does not save
  that level into the keyboard. Laptop 12 and 13 keys are on the embedded
  controller, so those use `framework_tool --kblight`. The power button on
  all three is `framework_tool --fp-brightness`. A BIOS level such as Auto is
  restored with `--fp-led-level`, not pinned to the percent it happened to
  be showing. Anything that is not a Framework 12, 13, or 16 is left alone,
  even if `qmk_hid` or `framework_tool` is installed. Levels are kept in
  `$XDG_RUNTIME_DIR/omarchy-power-settings/kblight.json` until wake. The
  fingerprint power button is the EC LED `chromeos:white:power`.
  `framework_tool --fp-brightness` as a user exits 0 without talking to the
  EC, and BIOS will not set that LED to 0%. Displays-off writes 0 to the
  sysfs brightness node, with `sudo -n tee` when the node is root-only. The
  first time a Framework plan is applied, a polkit prompt can install a
  wheel NOPASSWD sudoers drop-in for that exact `tee` path so idle can blank
  the LED without a password. If a tool is missing, or neither a direct call
  nor `sudo -n` can reach it, the screens still blank and that light is left
  as it is.
- The sudoers install is a `pkexec` process on the service, not a child of
  `apply`. Asking from Quickshell's Python apply never showed a polkit
  dialog. After the drop-in is in place, idle can write the LED without a
  prompt.

## [0.0.5] - 2026-09-25

### Fixed

- Displays off and Sleep now fire after their idle timeout. Quickshell's
  `IdleMonitor` does not re-arm when its timeout or enabled state changes after
  it is created, and the timeouts arrive after the saved plan loads. Each
  timeout now gets a fresh idle monitor.
- Monitors that drop off the bus when they are blanked (seen on the FlipGo)
  now lose signal once per blank instead of twice. When an output has to be
  disabled because it came back during a blank, its description is saved to
  `~/.local/state/omarchy-power-settings/bus-droppers.json`. Later blanks
  disable it directly instead of sending DPMS, so it no longer comes back lit
  before going dark. Such a monitor still shows its own "No signal" screen
  once, because it resets on any power change, including DDC/CI standby. Other
  monitors keep using DPMS, and at least one output always stays enabled.

## [0.0.4] - 2026-09-25

### Changed

- Power profile is two rows, AC and battery, with Power-saver, Balanced, and
  Performance. Each choice has the same icon as the bar widget, and the saved
  choice is bold. A new AC plan starts on performance, and a new battery plan
  starts on balanced. Saved values `auto`, `saver`, and `full` are read as
  balanced, power-saver, and performance.

## [0.0.3] - 2026-09-25

### Fixed

- External monitors no longer light back up at the lock screen while the
  laptop panel stays dark. Some monitors (seen on the Realtek FlipGo dual
  panel over USB-C) drop off the bus when DPMS turns them off and reconnect a
  few seconds later, and Hyprland powers every newly connected output on. The
  service now watches Hyprland `monitoradded` events: an output that appears
  while another output is blanked is disabled, whoever blanked the screens
  (this plugin, the lock screen, or Omarchy). It is restored with its saved
  mode, position, and scale once the blanked outputs wake. At least one output
  always stays enabled.
- Runtime monitor rules use the monitor's description (`desc:`) when it is
  safe to quote, so they win over description-based rules in `monitors.lua`.

### Corrected

- The 0.0.2 entry below said FlipGo-style monitors ignore DPMS. They honor it,
  then disconnect and reconnect, which 0.0.2 did not handle.

## [0.0.2] - 2026-09-24

### Fixed

- Idle **Displays off** and lid **Blank screens** now blank every connected
  Hyprland output. Hyprland 0.56 rejected the old `hyprctl dispatch dpms`
  dispatcher, so the timer was a no-op, and lid close only targeted `eDP-2`.
  The plugin now sends `hl.dsp.dpms` per monitor. Outputs that still report
  DPMS on afterwards are disabled and restored on wake, while at least one
  output is kept enabled so the session lock cannot lose every surface.
- Travel-mode brightness no longer assumes `eDP-2`. It uses the first
  `eDP` / `LVDS` / `DSI` connector present.
- The lid-switch Hyprland bind pointed at a machine-specific `ops.py` path.
  It now uses this plugin's own script path so the bind works for any user.

### Changed

- Lid action label **Panel off** is now **Blank screens**, matching the
  all-outputs behavior.

## [0.0.1] - 2026-09-24

### Added

- Omarchy 4 bar widget and background service, forked from Battery+ and
  restyled on the shell `Style` tokens.
- Battery readout: percent, status, draw, time remaining, health, cycles,
  capacity, temperature, and a power-draw sparkline.
- Per-source (AC / battery) timers for screensaver, displays off, auto-lock,
  sleep, and hibernate-after-sleep, written through to `shell.json` idle.
- Per-source lid action (default, nothing, blank screens, suspend, hibernate)
  and power profile (auto / saver / full).
- Framework charge ceiling and Boost via `framework_tool` when present.
- Travel mode, peripheral device batteries, optional NVIDIA status, and
  device power tweaks that do not require TLP.
- `docs/cursor-screensaver-workaround.md` for Cursor's Wayland idle inhibitor
  blocking screensaver and lock.
