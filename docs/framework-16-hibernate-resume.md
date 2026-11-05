# Framework Laptop 16 AMD Ryzen AI 300: hibernate resume rejected (E820)

Public firmware/kernel report. Omarchy Power Hub can only ask logind
whether hibernation is allowed and fall back to suspend. It cannot make
resume succeed when the firmware memory map changes across boots.

This is separate from “hibernate is unavailable” while Bitwarden holds
`memfd_secret`. That is Linux-wide:
[docs/hibernate-memfd-secret-bitwarden.md](hibernate-memfd-secret-bitwarden.md).

## Summary

Once the kernel allows hibernation (`/sys/power/disk` is not `[disabled]`),
Laptop 16 (AMD Ryzen AI 300) often **writes** a hibernation image, then
**resumes as a cold boot**. Firmware places a small ACPI region at a
slightly different physical address each boot. The kernel’s E820 checksum
in `arch_hibernation_header_restore()` then fails with
`Hibernate inconsistent memory map detected!` and throws the image away.

The bulk RAM layout is unchanged; only a non-RAM ACPI slice moves (~36 KiB
in community captures).

## Links

- Framework community: [FW16 hibernation fails to resume, firmware presenting different memory map every boot](https://community.frame.work/t/fw16-hibernation-fails-to-resume-firmware-presenting-different-memory-map-every-boot/83015)
- Related AI 300 resume race (`pm_async=0`): [Framework 13 Krackan thread](https://community.frame.work/t/hibernate-resume-failures-on-framework-13-amd-ryzen-ai-300-krackan-a-b-tested-workaround-pm-async-0/83040)
- Proposed kernel knob `hibernate=relaxed_memmap` ([LKML v2](https://lkml.iu.edu/2606.2/11762.html)). Not in kernel `7.2.3-arch1-3`. The plugin cannot add it.

## System (reporter)

Same machine as [docs/framework-16-usb-c-replug-drain.md](framework-16-usb-c-replug-drain.md).

| Item | Value |
| --- | --- |
| Product | Framework Laptop 16 (AMD Ryzen AI 300 Series) |
| Board | FRANMHCP09, version A9 |
| BIOS | 04.01, 2026-06-12 |
| OS | Arch / Omarchy, Hyprland, kernel `7.2.3-arch1-3` |
| Resume setup | swapfile on LUKS/btrfs, `resume=/dev/mapper/root`, matching `resume_offset`, mkinitcpio `resume` hook |
| Hibernate attempts in this journal | none recorded (availability was blocked by Bitwarden until documented) |

ACPI reports S4. Swap is sized to RAM. Setup is not the missing-swap class
of logind `CanHibernate` failure.

## Expected

Hibernate, power off, power on, unlock LUKS, restore the previous session.

## Actual

Resume boots a new session. Community logs show the E820 mismatch. A
separate AI 300 class of failure is a hang or reset on device resume;
`pm_async=0` is an experimental workaround for that race, not a firmware fix.

## Workaround

Treat hibernate-after-sleep as best-effort on this board until firmware
stops moving the ACPI region or a kernel with `hibernate=relaxed_memmap`
ships. The panel still sleeps (suspend) when hibernation is refused.

Do not set `hibernate=relaxed_memmap` unless that kernel is actually
installed; the parameter is ignored on current Arch `7.2.3`.
