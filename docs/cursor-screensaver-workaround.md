# Cursor blocks the screensaver on Wayland

On a native Wayland session, Cursor holds a Wayland idle inhibitor for as long as its window is open. The compositor never reports the session as idle, so the screensaver, lock, and display-off timers never start. Closing Cursor releases the inhibitor. Opening it again sets the inhibitor again.

This is not limited to one laptop or one desktop. It shows up on any Wayland compositor that honors idle inhibitors, including Hyprland, once Cursor is built on Chromium 146 or newer (Cursor 3.19 and later).

Forum report, including an XWayland test where the screensaver did start with the workaround:

https://forum.cursor.com/t/wayland-cursor-holds-an-idle-inhibitor-and-blocks-the-screensaver-while-the-agents-window-is-open/172907

## What happens

Anything that respects Wayland idle inhibitors will wait forever while that Cursor window is mapped. Omarchy's idle service is one example: its Quickshell `IdleMonitor` runs with `respectInhibitors` enabled, so it follows the compositor and never begins the screensaver cycle.

Typical signs:

- The Cursor window class is `cursor`. The title is often `Cursor Agents` while an agent chat is open, but a normal editor window can inhibit too.
- On Hyprland, `hyprctl clients -j` shows `xwayland: false` and `inhibitingIdle: true` for that window only. Other clients stay `inhibitingIdle: false`.
- A desktop idle timer (screensaver, lock, DPMS) that works with Cursor closed does nothing with Cursor open.
- `systemd-inhibit` does not list an idle inhibitor from Cursor. The block is the Wayland idle-inhibit protocol, not a logind sleep inhibitor.

Stay-awake switches, a long timeout, or another app can fail the same way. Rule those out before blaming Cursor. On Omarchy, stay-awake is a state file under `~/.local/state/omarchy/`, and the idle timeouts live in `~/.config/omarchy/shell.json`.

## Why Cursor does this

Cursor staff traced it to Chromium, which Cursor is built on, not to the agent wakelock.

The wakelock Cursor takes while an agent is running is a suspend inhibitor on D-Bus. It does not create a Wayland idle inhibitor. Starting with Chromium 146, Chromium apps on Wayland began setting a Wayland idle inhibitor on ordinary windows. Cursor picked that Chromium up in the 3.19 release. There is no setting to turn the Wayland inhibitor off.

A separate forum thread describes that intentional wakelock and asks for a way to stop the computer from sleeping. That is a different mechanism. Those threads want more keep-awake, not a fix for the screensaver never starting:

https://forum.cursor.com/t/add-option-to-disable-computer-sleep-while-an-agent-is-running/159837

## How to see it

On Hyprland:

```bash
hyprctl clients -j
```

For the Cursor window, compare `xwayland` and `inhibitingIdle`. Native Wayland plus `inhibitingIdle: true` means the screensaver will not start. XWayland plus `inhibitingIdle: false` means the compositor is no longer blocked by Cursor.

The process command line shows which platform that window actually started with:

```bash
tr '\0' ' ' < /proc/$(hyprctl clients -j | jq -r '.[] | select(.class=="cursor") | .pid')/cmdline
echo
```

A native Wayland launch has no Ozone flag. An XWayland launch contains `--ozone-platform=x11`. The Electron binary path depends on the package (`electron`, `electron42`, a Cursor-bundled binary, and so on).

`--force-device-scale-factor` in that command line is the text fix below. It is not required for the screensaver.

Other compositors expose the same inhibitor through their own tools (`wayland-info`, `xprop` does not show it, a compositor debug socket, and so on). The Hyprland field is `inhibitingIdle`.

## Workaround

Run Cursor through XWayland with Chromium's Ozone flag:

```text
--ozone-platform=x11
```

On XWayland, Hyprland reports the Cursor window as `xwayland: true` and `inhibitingIdle: false`. After that, leaving the keyboard and mouse idle brings the screensaver up on the normal schedule.

The flag has to be on the Electron process when it starts. A Cursor that is already running does not pick it up. Quit every Cursor process, then start it again. A second launch while a native Wayland instance is still running just attaches to that instance, and the inhibitor stays.

### Where to put the flag

Put it somewhere every launch path reads. One path is not enough.

**Flags file.** The Arch Cursor launcher (`/usr/bin/cursor`, which points at the package script) reads `~/.config/cursor-flags.conf` and appends each non-comment line to the Electron command. It also reads `~/.config/electronN-flags.conf` (for example `electron42-flags.conf`) or `~/.config/electron-flags.conf` for Electron arguments. Extra arguments on the `cursor` command line are forwarded the same way. A flags file therefore covers the terminal, a shortcut that execs `cursor`, and the desktop file.

```text
--ozone-platform=x11
```

Other packages use a different flags file. Check the launcher script (`cursor` on `PATH`) before assuming the filename. If the script does not read a flags file, the desktop `Exec` line and every shortcut have to carry the flag themselves.

**Desktop files.** The app menu, a pinned launcher, and `cursor://` links use a `.desktop` file, not the flags file, unless the `Exec` line runs a launcher that reads the flags file. Prefer a copy in `~/.local/share/applications/` with the same filename as the package file. That copy wins over `/usr/share/applications/` and survives package updates. Do not edit the packaged file. The next Cursor update replaces it.

Find the packaged entries:

```bash
rg -n '^Exec=' /usr/share/applications/cursor*.desktop
```

A user override looks like this. The binary path must match the packaged `Exec` line on that system.

```text
Exec=/usr/share/cursor/cursor --ozone-platform=x11 %F
Exec=/usr/share/cursor/cursor --ozone-platform=x11 --new-window %F
Exec=/usr/share/cursor/cursor --ozone-platform=x11 --open-url %U
```

The second line is the "New Empty Window" action, when the desktop file has one. The third is the URL handler (`cursor-url-handler.desktop`). Without the flag on the URL handler, a `cursor://` link can start a native Wayland instance while Cursor is not already running.

**Shortcuts that skip the desktop file.** Some keybindings exec `cursor` directly. On Omarchy, SUPER+SHIFT+N is the stock Editor binding (`{ omarchy = "editor" }` in the default Hyprland bindings). `omarchy editor` runs `omarchy-launch-editor`, which reads the editor name from `~/.local/state/omarchy/defaults/editor` and starts it with `uwsm-app -- <editor>`. When that name is `cursor`, the shortcut runs the binary and ignores `cursor.desktop`. The flags file still applies, because the binary's launcher reads it. Changing only the desktop file leaves that shortcut on native Wayland.

Do not put `--ozone-platform=x11` inside the editor-name file. `omarchy-launch-editor` treats the whole line as a command name, fails to find it, and falls back to a terminal editor.

### Apply it

1. Find the monitor scale you actually use (below). Write `~/.config/cursor-flags.conf` with `--ozone-platform=x11`. Add `--force-device-scale-factor=<scale>` on the next line when the text side effect shows up.
2. If anything launches Cursor from a desktop file, install user overrides under `~/.local/share/applications/` as above. The scale factor does not have to be repeated on `Exec=` when the launcher reads `cursor-flags.conf`.
3. Quit Cursor completely. On Hyprland, confirm no Cursor window remains:

   ```bash
   hyprctl clients -j | jq -r '.[] | select(.class=="cursor") | .pid'
   ```

4. Start Cursor the way you normally do: shortcut, menu, or terminal.
5. Confirm the new window. On Hyprland:

   ```bash
   hyprctl clients -j | jq '.[] | select(.class=="cursor") | {title, xwayland, inhibitingIdle, pid}'
   ```

   Expect `xwayland: true` and `inhibitingIdle: false`.
6. Leave the keyboard and mouse idle for the screensaver timeout. The screensaver should start.

### Side effect: text looks different

XWayland does not draw text the way native Wayland does. After the screensaver workaround, Cursor's text can change weight and size. Cursor staff warned about this when they suggested `--ozone-platform=x11`: at fractional scale, text can look softer or simply different.

Hyprland often sets `xwayland:force_zero_scaling` (`hyprctl getoption xwayland:force_zero_scaling`). With that option, XWayland clients are not given the monitor scale. They render as if the scale were 1, and the window is fitted onto the scaled desktop. GTK apps hide this with `GDK_SCALE` set to an integer (often 2). Cursor is Electron, not GTK, so it ignores `GDK_SCALE`. Native Wayland Cursor asks the compositor for the real scale and rasterizes its own text there. XWayland Cursor rasterizes at 1 unless told otherwise. The glyphs come out a different size, and the antialiasing does not match the Wayland window.

Integer scale (2, exactly) often looks closer. Fractional scale (1.25, 1.5, 1.6, and so on) is where the difference is obvious.

This does not bring the idle inhibitor back. `inhibitingIdle` stays false while the Ozone platform is X11. The text change is only how the window is painted.

### Fix for the text

Tell Electron the monitor scale explicitly. In the same flags file:

```text
--ozone-platform=x11
--force-device-scale-factor=SCALE
```

Replace `SCALE` with the scale Hyprland is using, not a guessed default. On Hyprland:

```bash
hyprctl monitors -j | jq '.[] | {name, scale}'
```

On Omarchy the same number is `omarchy_monitor_scale` in `~/.config/hypr/monitors.lua`. Use one value. If two monitors differ, pick the scale of the monitor Cursor sits on, or accept that the other monitor will look slightly off.

`--force-device-scale-factor` makes Chromium render the UI at that scale. It does not switch Cursor back to Wayland, and it does not set a Wayland idle inhibitor. The screensaver workaround stays in place.

Quit Cursor completely and start it again. The running process keeps the flags it was born with. After the restart, the command line should contain both flags, `xwayland` should be true, and `inhibitingIdle` should be false.

If the interface comes back too large or too small, the number does not match the monitor. Fix the number, or delete only the `--force-device-scale-factor` line and leave `--ozone-platform=x11`. Text goes back to the XWayland scale-1 look. The screensaver still works.

## Rollback

### Undo only the text fix

Leave the screensaver workaround in place. In the flags file, delete the `--force-device-scale-factor` line and keep `--ozone-platform=x11`. Quit Cursor and start it again. Text goes back to the XWayland scale-1 look. `xwayland` stays true and `inhibitingIdle` stays false, so the screensaver still runs.

### Undo the screensaver workaround

This returns Cursor to native Wayland. The idle inhibitor comes back, and the screensaver will not start while Cursor is open. The text side effect goes away with it, because native Wayland draws at the monitor scale again.

1. Quit Cursor completely. A running XWayland process keeps its flags until it exits.
2. Remove the flags file you added, for example:

   ```bash
   rm -f ~/.config/cursor-flags.conf
   ```

   Leave `electron-flags.conf` or `electronN-flags.conf` alone unless you added the Ozone flag there yourself. Those files are shared with other Electron apps.

3. Remove only the user desktop overrides, if you created them:

   ```bash
   rm -f ~/.local/share/applications/cursor.desktop
   rm -f ~/.local/share/applications/cursor-url-handler.desktop
   ```

   Do not delete the files under `/usr/share/applications/`. Those belong to the package. After the user copies are gone, the menu and `cursor://` links use the package entries again.

4. Do not change the configured editor name (`~/.local/state/omarchy/defaults/editor` on Omarchy, or the equivalent elsewhere). It should still be the editor command, usually `cursor`, with no flags in that file.

5. Start Cursor again and confirm native Wayland. On Hyprland, expect `xwayland: false` and `inhibitingIdle: true`.
