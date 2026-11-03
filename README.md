# Omarchy Power Settings

Theme-aware Omarchy 4 bar plugin. Forked from [Battery+](https://github.com/amikeal/omarchy-battery-plus).

The working copy is this directory. The shell loads it through
`~/.config/omarchy/plugins/omarchy-power-settings`. See [CHANGELOG.md](CHANGELOG.md)
for what shipped today.

## What it does

- Battery hero, time left, power draw, health, cycles, capacity, and a 24-hour battery-usage chart
- Per-source screensaver, displays off, auto-lock, sleep, and hibernate-after-sleep
- Displays off (idle and lid) apply to every Hyprland output, not a hardcoded laptop panel
- Per-source lid action and power profile (Power saver / Balanced / Performance). A new Plugged in plan starts on performance, and a new Battery plan starts on balanced.
- Framework charge limit and Boost, or a UPower battery-health toggle on
  other laptops that expose charge thresholds
- Travel mode, and whether the discrete GPU is Asleep or Awake
- Device powersave without TLP

Colors come from the shell `Style` tokens, so the panel follows the active theme.

## Automatic lock can stop after display blanking

Quickshell 0.3.1 can lose idle notifications when a running timer changes.
This plugin can trigger the bug when displays turn off or wake up, leaving
the desktop unlocked despite the saved auto-lock setting. See
[Quickshell idle timer failure](docs/quickshell-idle-timer-failure.md)
for the diagnosis, installed 0.3.1 backport, rollback instructions, and
verification steps.

## Travel mode and Quick dim

**Travel mode** temporarily caps the internal laptop display at 40%
brightness and uses a supported mode at or below 60 Hz without increasing a
display already running slower. External displays are left alone. Turn it
off to restore the captured brightness and display mode; if a device cannot
be restored, the panel keeps the restore state and offers a retry.

Travel mode also switches to Power saver and restores your normal power mode
when turned off. Plugging the adapter back in turns Travel mode off; it does
not come back on when you unplug again. On AMD that profile already turns CPU boost off and prefers
energy saving. Heavy work may run slower. Without power-profiles-daemon only
the display is changed.

**Quick dim** independently caps brightness at 40% and restores the level it
captured when turned off. It is unavailable while Travel mode owns the
display, so the two restore points cannot overwrite one another.

## Charge limit and battery health

**Framework:** the panel uses `framework_tool --charge-limit` (EC ceiling),
not Linux start/stop thresholds. This board has no
`charge_control_*` sysfs files. Install once:

```sh
omarchy pkg add framework-system
```

Desk-docked default is 80%. The service re-applies that saved ceiling after
login if firmware forgot it (skipped while Boost is on). Turn **Battery
Extender** off in BIOS when using a custom ceiling; the two fight.

The panel sets the limit with `framework_tool --charge-limit`. That needs a
passwordless sudoers rule for that command, or a polkit prompt. Boost
temporarily charges to 100%, then restores the saved ceiling.

**Other laptops:** if UPower reports charge thresholds, the panel shows
**Battery health**. On uses the firmware band (often 75–80% or LongLife).
Off charges to 100% for travel. No sudo. If UPower has no threshold
support, the section is hidden.

Do not run this plugin together with another charge-threshold plugin.

Laptop 16 with a USB-C monitor can fail to charge after the adapter is
unplugged and plugged back in. That is a firmware issue. Public report:
[docs/framework-16-usb-c-replug-drain.md](docs/framework-16-usb-c-replug-drain.md).

## Hibernate

The kernel disables hibernation while any process holds `memfd_secret`.
Bitwarden Desktop does this after login. Details and the mlock workaround:
[docs/hibernate-memfd-secret-bitwarden.md](docs/hibernate-memfd-secret-bitwarden.md).
The panel names that cause and falls back to plain sleep.

Laptop 16 (AMD Ryzen AI 300) can still cold-boot on resume after hibernation
is allowed: firmware moves a small ACPI region, so the kernel rejects the
image. The plugin cannot fix that.
[docs/framework-16-hibernate-resume.md](docs/framework-16-hibernate-resume.md).

## Framework keyboard and power-button lights

Laptop 16 key backlight uses `qmk_hid`. Laptop 12 and 13 use `framework_tool --kblight`. The fingerprint power button is `/sys/class/leds/chromeos:white:power`. That node is root-only, and `framework_tool --fp-brightness 0` does not turn it off.

The first time this plugin applies a Framework plan, polkit may ask for a
password to install `/etc/sudoers.d/omarchy-power-settings-leds`. After that,
displays-off can write the LED without a prompt. You can also install it
yourself with `pkexec` on this plugin's `bin/ops-priv led-sudoers`.
