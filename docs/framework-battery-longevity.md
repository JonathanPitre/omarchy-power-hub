# Framework battery longevity: BIOS charge limit and Battery Extender

Recommended setup for the longest battery life, and why.

## Settings

| Where | Setting | Value |
| --- | --- | --- |
| BIOS > Advanced | Battery charge limit | `80`, same as the plugin limit |
| BIOS > Advanced | Battery Extender | `Disabled` |
| Plugin | Charge limit | `80` (default) |

- The BIOS limit is the only limit that applies before login, in the BIOS, or if the plugin is not running. Match it to the plugin so there is no window above the target after boot.
- 100% in the BIOS leaves the plugin as the only protection. 90% leaves a window above 80% after every boot.
- Boost and the plugin's limit buttons still work at runtime. A reboot returns to the BIOS limit.
- Framework: "leaving it at 100% state of charge for an extended period of time can shorten the lifetime of the battery." A manual lower limit preserves it further. An 80% limit holds between 75% and 80%.
- For a mostly docked laptop, 60-70% wears the battery less. 80% is the Framework-recommended balance.

## Why Battery Extender is off

Battery Extender is a longevity feature and is on by default. It lowers the ceiling after days on AC:

| Days plugged in | Ceiling |
| --- | --- |
| 0-5 | 99% to 100% |
| 5-7 | 90% to 95% |
| more than 7 | 85% to 87% |

The **Trigger** setting is the number of days. **Reset** is the minutes on battery that restart the timer.

- Every level is above an 80% limit, so it adds nothing on top of the limit.
- Framework's BIOS 3.08 notes list a known issue: with the limit below 100% and the extender enabled, the device charges to 100% after AC has been disconnected for more than 30 minutes. Their workaround is to disable the extender, then set the limit.
- Observed on a Framework with the latest BIOS: BIOS limit 90, extender enabled, and `framework_tool --charge-limit` reported `Maximum 100%` while the battery kept charging at 96%. I did not confirm the cause. The extender bug fits.

If the extender stays on, check that it works on your BIOS first: set the limit, unplug for 30+ minutes, replug, and confirm the limit has not gone back to 100%.

## Verify

```fish
sudo framework_tool --charge-limit
upower -i (upower -e | grep BAT) | grep -E 'state|percentage'
```

- `Maximum 80%` is the EC ceiling. Charging stops there, but the battery does not discharge down to it.
- `Maximum 100%` means the firmware reset the limit. The plugin reapplies it after login. To set it by hand: `sudo framework_tool --charge-limit 80`.

## Sources

- [Framework Laptop 13 Ryzen 7040 BIOS 3.08 release notes](https://community.frame.work/t/framework-laptop-13-ryzen-7040-bios-3-08-release-beta/66858)
- [Battery limit setting not working anymore](https://community.frame.work/t/battery-limit-setting-not-working-anymore/84952)
- [Bios Battery charging limit ignored](https://community.frame.work/t/bios-battery-charging-limit-ignored/69610)
