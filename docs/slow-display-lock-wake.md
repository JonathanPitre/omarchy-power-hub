# Lock screen re-blanks before slow monitors wake

On 2026-10-21, the lock-screen wake issue from [Omarchy PR #8773](https://github.com/omacom/omarchy/pull/8773) was reproduced on this system. The built-in Framework display woke first; the external RTK FlipGo panels connected over USB-C carrying DisplayPort video came back one at a time while the password was being entered.

## Cause

The lock service blanks displays five seconds after locking. On a slow DisplayPort link, the compositor can report that an output woke before the panel has finished link training and can show the lock prompt. A new five-second timer can then blank the prompt again while the external display is still recovering.

PR #8773 tracks whether the lock display was blanked and allows a 30-second wake grace after the first wake. Re-arming the blank timer preserves the remaining grace, so password checks and monitor reconnect events do not shorten it. The same 30-second delay is used for a system resume while the session is locked.

## Local change

The PR implementation is installed in:

```text
/usr/share/omarchy/shell/plugins/lock/Service.qml
```

The installed service has `displayBlanked`, `wakeGraceUntil`, a five-second initial blank delay, and a 30-second wake delay. The Omarchy shell was restarted after the patch was installed. This is a local edit to the package-owned file, not an Omarchy package update; a package upgrade may replace it.

## Local retest

The test on 2026-10-21 used the live desktop:

| Local time | Observation |
| --- | --- |
| 12:20:30 | Lock requested and session locked |
| 12:20:36 | Displays entered sleep |
| 12:20:45 | Display wake detected; the built-in Framework panel woke first |
| 12:20:49–12:20:54 | External DP-2 and DP-3 disconnected and reconnected; the compositor briefly reported that displays were still connecting |
| During password entry | External FlipGo panels became visible one at a time |
| 12:20:59 | Session unlocked |

This confirms that the slow external-display wake path affects this setup. The lock screen remained available long enough to enter the password. The observation ended about 14 seconds after the first wake, so it does not verify the full 30-second grace period or establish whether a panel went black during its reconnects.

The [PR report](https://github.com/omacom/omarchy/pull/8773#issuecomment-6308735158) contains the same retest and journal observations.

## After an Omarchy update

Because the change is in a package-owned file, check the installed service after Omarchy updates. If the wake-grace properties and timer logic are gone, the package update replaced the local patch. Review the current Omarchy version and PR status before reapplying it; do not copy an old service file over a newer one.
