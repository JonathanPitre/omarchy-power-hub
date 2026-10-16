# Bitwarden Desktop blocks Linux hibernation (`memfd_secret`)

Linux-wide, not a Framework bug. The kernel refuses to hibernate while any
process holds `memfd_secret` memory, because that memory is not supposed to
be written to a hibernation image. Bitwarden Desktop 2026.6.1+ uses
`memfd_secret` for the vault key after login.

Upstream: [bitwarden/clients#21661](https://github.com/bitwarden/clients/issues/21661).

## Symptom

`/sys/power/disk` reads `[disabled]`. `/sys/power/state` has no `disk`.
`systemctl hibernate` and `systemctl suspend-then-hibernate` fail with
“Sleep verb … is not configured or configuration is not supported by kernel.”
Quitting Bitwarden restores the disk modes immediately.

Omarchy Power Settings reports this on the idle plan as Bitwarden holding
secret memory, and falls back to plain suspend so the laptop still sleeps.

## Confirm

Bitwarden log (`~/.config/Bitwarden/app.log`):

```
Using memfd_secret for secure key storage
```

If the log also says `" is not a valid secure key container backend"`, an
empty `SECURE_KEY_CONTAINER_BACKEND` is in the environment. Treat that as
unset; Bitwarden then auto-selects `memfd_secret`.

## Workaround

Force mlock (or `keyctl`). The file that lasts is systemd user environment,
not the XDG autostart `.desktop` (Bitwarden rewrites that `Exec=` line on
every launch).

```
# ~/.config/environment.d/60-bitwarden.conf
SECURE_KEY_CONTAINER_BACKEND=mlock
```

That applies on the next login to Bitwarden started from the menu, terminal,
or autostart. Optional extra for Hyprland-spawned children, at the bottom of
`~/.config/hypr/hyprland.lua`:

```lua
hl.env("SECURE_KEY_CONTAINER_BACKEND", "mlock")
```

Do not put `env …` on `~/.config/autostart/bitwarden.desktop`. It disappears
within seconds of a Bitwarden launch.

This session only (already logged in):

```sh
systemctl --user set-environment SECURE_KEY_CONTAINER_BACKEND=mlock
```

Then restart Bitwarden. The log should show
`Forcing mlock secure key container via environment variable`, and
`/sys/power/disk` should list real modes instead of `[disabled]`.

The vault key can then appear in the hibernation image. On this machine the
image is the swapfile on LUKS-encrypted root, so it is encrypted at rest.

## Plugin scope

The panel can name Bitwarden and fall back to suspend. It cannot change
Bitwarden’s key backend. Other apps that call `memfd_secret` produce the
same kernel block without the Bitwarden-specific hint.
