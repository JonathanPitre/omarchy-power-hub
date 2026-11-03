# Quickshell idle timer failure

Investigated on 2026-10-16 with Quickshell `0.3.1-1`, Hyprland `0.56.2-2`,
and three monitors. Automatic locking can stop even though the plugin and
the running shell both have the correct lock timeout.

## Cause and plugin trigger

The defect is in Quickshell's `IdleMonitor`. Omarchy uses that component to
start its screensaver and lock cycle. A timeout change replaces the
underlying Wayland notification object. If the allocator reuses the old
object's address, Quickshell treats the reference as unchanged and does not
subscribe to the replacement object's idle state. The compositor still
sends events, but the shell's `isIdle` value stops updating.

The power plugin contains a direct trigger for this defect:

1. `bin/ops.py` calls `idle_for_blank()` when displays turn off or wake up.
2. `idle_for_blank()` calls `shell_idle()` to update `shell.json`.
3. During blanking, it substitutes 604800 seconds for the screensaver timeout.
4. Omarchy's idle service uses the smaller of the screensaver and lock
   timeouts for its `IdleMonitor`.

With the investigated AC plan, the effective monitor timeout changes from
120 seconds to 300 seconds during blanking, then back to 120 on wake.
The saved lock setting remains 300 seconds throughout. Editing idle settings
or switching power plans can also change the monitor timeout.

Once the idle value freezes, the shell cannot observe another active-to-idle
transition. A previous lock cycle can finish, but a later unlock does not
start a new idle cycle by itself. This explains why automatic locking can
work initially and then stop for hours.

Relevant sources:

- [Plugin timer updates](../bin/ops.py), `shell_idle()`, `idle_for_blank()`,
  and the `displays` command.
- Installed Omarchy service,
  `/usr/share/omarchy/shell/plugins/services/idle/Service.qml`.
- [Quickshell 0.3.1 listener replacement](https://github.com/quickshell-mirror/quickshell/blob/v0.3.1/src/wayland/idle_notify/monitor.cpp).
- [Upstream fix, PR #1097](https://github.com/quickshell-mirror/quickshell/pull/1097).

## What the investigation established

The live shell reported `enabled: true`, `stayAwake: false`, `idle: true`,
and `inIdleCycle: false`, with no lock timer running. Its configured
screensaver and lock timeouts were 120 and 300 seconds. The lock plugin
reported an unlocked session and three screens.

The journal recorded a successful lock at 13:12 local time and an unlock
at 14:24. The idle service logged no further transitions after its last
lock command at 13:17 until the listener reset at 17:38. These times use
Asia/Bangkok, UTC+07:00.

A separate Quickshell process reproduced the defect on this machine:

- Fresh idle monitors delivered idle and resume events.
- After changing a live timeout to zero, Wayland delivered `idled`, but
  both monitors continued reporting `isIdle: false`.
- Debug logs showed the old and replacement notification at the same address.
- Disabling and enabling those monitors restored idle delivery.

A fixture test of the plugin's actual `idle_for_blank()` function confirmed
the timeout sequence of 120, 300, and 120 seconds without changing desktop
configuration or display power.

The defect and the plugin's trigger are reproduced. The historical journal
does not record each configuration write, so the exact write that caused
the original frozen listener remains inferred.

## Patch safety assessment

As checked on 2026-10-16, PR #1097 is open and unmerged. The reviewed head is
`1cbf9ee8444f1a65d605c5cc0308040595d831ff`.

The runtime change is small. It clears `bNotification` before deleting the
old object, then assigns the replacement. Those separate reference changes
allow the idle-state binding to subscribe again even when an address is
reused. The remaining files add regression tests and their build setup.

The source change is a low-risk candidate for a controlled backport. It
directly addresses the reproduced failure without changing the intended
timeout values. Source review alone does not verify a patched binary.
The initial investigation used the unpatched package. The later build and
installation results are recorded below.

Prefer a package built from the installed Quickshell release with this
runtime fix backported. Installing the PR's entire development branch
would introduce unrelated changes from its base. Preserve the distribution
package's build options, especially its Wayland and PAM support.

Before installation, obtain and retain a trusted rollback package. The
original `quickshell-0.3.1-1` package was absent from this machine's pacman
cache during initial inspection. Install and remove a custom build through pacman
so package ownership remains intact. Reinstalling a binary requires
restarting the shell or logging out before the running shell uses it.

## Installed release backport

On 2026-10-16, the runtime fix was backported onto the `v0.3.1` tag and
installed as `quickshell 0.3.1-1.1`. The source change affects only
`src/wayland/idle_notify/monitor.cpp`.

- [Personal fork and current fix branch](https://github.com/JonathanPitre/quickshell/tree/fix/idle-notification-subscription).
- [Backport commit c291feca](https://github.com/JonathanPitre/quickshell/commit/c291fecab19c1d2e670890ba137d89cf42cfab3c).
- Local checkout at `~/Work/quickshell`.
- Build, package, checksums, test logs, and rollback files at
  `~/.local/share/quickshell-idle-fix/`.

The package preserves the original package's installed paths and dependency
list. The build retains PAM, Wayland session locking, and the other desktop
features. It uses the installed Qt libraries and identifies itself as
`JonathanPitre 0.3.1 PR1097 backport`. The version remains 0.3.1 with a local
package release suffix, rather than switching to the upstream development branch.

The same idle-event harness reproduced the failure on the original binary
and passed on the patched build, the packaged executable, and the installed
`/usr/bin/quickshell`. After a live timeout change, both inhibitor modes now
report `isIdle: true`. The installed executable's hash matches the packaged
one. Omarchy's shell was restarted while unlocked and loads the patched
executable. Its idle settings remain 120 and 300 seconds, and the lock plugin
reports three screens with PAM available.

These checks verify the timeout-change defect and the restarted shell. A full
five-minute automatic lock cycle, followed by unlock and a second cycle on all
three monitors, has not yet been observed after installation.

### Roll back this installation

The retained original package is
`~/.local/share/quickshell-idle-fix/rollback/quickshell-0.3.1-1-x86_64.pkg.tar.zst`.
Its detached signature was verified against the Arch keyring. Its executable
also matched the originally installed binary byte for byte before replacement.

From a terminal in an unlocked desktop session, run:

```sh
~/.local/share/quickshell-idle-fix/rollback.sh
```

The script installs the retained `0.3.1-1` package with `sudo pacman -U`,
restarts the shell with `omarchy restart shell`, and prints the installed
version. A rollback restores the original idle-notification bug as well.

This local package has not been excluded from normal package updates. A later
distribution package with a newer version can replace it. Check whether that
version includes the upstream fix. Quickshell uses private Qt APIs, so retain
the source and rebuild the backport if a Qt update requires it. As of
2026-10-21, the installed package is again excluded from updates through
`IgnorePkg = quickshell` in `/etc/pacman.conf` (see the status below).

### Upstream review on 2026-10-18

The fix branch was renamed to `fix/idle-notification-subscription`. It includes
upstream `master` at `2b4501a282689792b62e94f3fb30d8f512324fe6`, beyond the
latest release tag `v0.3.2`. The difference against that upstream commit remains
one file with five added and three removed lines. The affected idle-monitor
code is unchanged between `v0.3.2` and that upstream commit.

The installed package had meanwhile advanced to Omarchy's
`0.3.2.r7.g2b4501a-1`, replacing the local 0.3.1 backport. In an isolated
omabox Hyprland session, the newer installed executable reproduced the lost
idle subscription. A build of the updated fix branch passed the same test
with both inhibitor modes. This review did not replace the installed package
or test the user's physical displays.

Existing upstream reports [#938](https://github.com/quickshell-mirror/quickshell/issues/938)
and [#1309](https://github.com/quickshell-mirror/quickshell/issues/1309) describe
the same defect. PR #1097 remains open as of this review. No duplicate bug
report is needed.

### Status on 2026-10-21

The support comment with the harness results was posted on
[PR #1097](https://github.com/quickshell-mirror/quickshell/pull/1097#issuecomment-6308220645).
The delivery route is to back that PR rather than open a duplicate.
All 36 checks passed on the PR head `1cbf9ee`, which is also the commit
reviewed above. The PR is still unmerged and this thread stays subscribed
until a maintainer responds.

The fix branch, which includes upstream `master` at `2b4501a`, was packaged
from the build directory verified on 2026-10-18 and installed as
`quickshell 0.3.2.r7.g2b4501a-1.1`. The packaged executable's SHA-256
(`7b53c0c5…`) matches `/usr/bin/quickshell`, and `IgnorePkg = quickshell`
keeps updates from replacing it with stock.

Before installation, the extracted package executable passed the same idle
harness in an isolated omabox Hyprland session: both inhibitor modes report
`true` after a live timeout change and stay `true` after a reset. Omarchy's
shell was restarted while unlocked at 12:05 local time (Asia/Bangkok) and
runs the patched executable (PID 1847284 at restart, `/usr/bin/quickshell`).
The section below describes the desktop checks that remain.

To return to stock, first remove the `IgnorePkg` line from
`/etc/pacman.conf`, then run `sudo pacman -S quickshell`, then restart the
shell. This restores the idle-notification bug as well.

## Temporary recovery

If Stay Awake is intentionally enabled, leave it enabled. Otherwise, reset
the idle listener from a terminal in the desktop session:

```sh
omarchy-shell idle disable
omarchy-shell idle enable
omarchy-shell idle status
```

Run the enable command even if the first command reports an error. Confirm
that the final status has `enabled: true` and `stayAwake: false`. The reset
does not change your saved timeout values. It does not prevent recurrence
when a timeout changes again.

For an immediate lock, use `omarchy system lock`. Display blanking alone
does not establish that the session is locked.

## Verification after a permanent fix

Validate the installed build on the desktop, with no intentional Stay Awake
setting or application idle inhibitor:

1. Confirm that the running shell uses the patched build and loads the
   plugin's saved timeouts.
2. Verify that idle events still arrive after a live timeout change.
3. Leave the desktop idle for the saved lock interval. Confirm that the
   session is secure and that every connected monitor shows a lock surface.
4. Unlock, then repeat a full idle interval to check that locking rearms.
5. Repeat after display blanking and wake, and after a power-plan change.

A passing Python plugin test suite does not verify Quickshell's native
idle listener or lock-screen coverage. If the patched build fails these
checks, reinstall the retained distribution package and restart the shell.
