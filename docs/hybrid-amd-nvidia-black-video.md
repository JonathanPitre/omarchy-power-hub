# Hybrid AMD + NVIDIA: black browser video, discrete GPU stays awake

On a laptop whose display is wired to an AMD iGPU, Omarchy 4.5 can still treat the NVIDIA card as the display GPU. Hyprland then exports NVIDIA's VA-API and GLX drivers for the whole session. Chromium and Brave paint a black video rectangle, and the discrete GPU leaves runtime suspend, so the pack pays for a card that is not driving the panel.

Omarchy Power Hub only reports that suspend state. It does not set these variables, and turning its power tweaks off does not clear them.

## What you see

- YouTube, X, and other `<video>` pages load. Audio can keep going. The picture stays black while hardware acceleration is on.
- `brave://gpu` can still say video decode is hardware accelerated. That line means the browser found a decoder. It does not mean the frames were shown.
- Turning **Use graphics acceleration when available** off makes the same video play. The browser is then on the software path.
- The Power Hub GPU row says **Awake** while nothing in the panel is asking the discrete GPU to wake. `power-hub gpu` reports `"asleep": false`.
- A one-off Brave launch with `--ozone-platform=x11` can show the video again, on either render node. That session can still log a failed GBM buffer import and a GPU process restart, then keep playing. X11 restores the picture. It does not remove the NVIDIA environment variables, so the discrete GPU can stay awake.

This was checked on a Framework Laptop 16, Omarchy 4.5.0-3, kernel `7.2.11-arch1-1`, Mesa 26.2.5, `nvidia-open` 615.78.08, `libva-nvidia-driver` 0.0.18, Brave 157.1.99.55.

| Device | Address | Class | `boot_vga` | Runtime at check |
| --- | --- | --- | --- | --- |
| NVIDIA GB206M RTX 5070 Max-Q (`10de:2d58`) | `0000:c2:00.0` | `0x030000` | `0` | `active` |
| AMD Strix Radeon 880M/890M (`1002:150e`) | `0000:c3:00.0` | `0x038000` | file absent | `active` |

The AMD device is the display controller. It has no `boot_vga` file. The NVIDIA device's `boot_vga` is `0`. A shell started inside the session had:

```text
LIBVA_DRIVER_NAME=nvidia
NVD_BACKEND=direct
__GLX_VENDOR_LIBRARY_NAME=nvidia
```

Brave's own GPU process was pinned to the AMD render node (`--render-node-override=/dev/dri/renderD129`) while those variables were set. Decode was aimed at NVIDIA, and presentation at AMD. Forcing the NVIDIA render node under Wayland exited with `SIGTRAP` before a window opened, so that run never tested playback.

The same shape is already filed upstream. [Issue #15148](https://github.com/omacom/omarchy/issues/15148) is an ASUS ProArt P16 with this Radeon 890M (`1002:150e`, no `boot_vga`) and an RTX 5070 whose `boot_vga` is `0`. A comment there reports the same detector result on a 2025 ROG Zephyrus G14 (Radeon 890M + RTX 5060).

## Why Omarchy does this

`/usr/bin/omarchy-hw-nvidia-display` is supposed to succeed only when NVIDIA drives the screen. Its own comment says the opposite case breaks Chromium (black video, audio still playing). The script looks for a class `0x03*` device with `boot_vga` equal to `1`. If no device claims it, including when the attribute is missing, the script assumes NVIDIA is the display and exits 0.

On this machine every detector exits 0: `omarchy-hw-nvidia`, `omarchy-hw-nvidia-gsp`, `omarchy-hw-nvidia-display`, and `omarchy-hw-hybrid-gpu`. The Strix iGPU is class `0x038000` and has no `boot_vga` file, so it never counts as the display.

`/usr/share/omarchy/default/hypr/nvidia.lua` then runs at Hyprland startup. For a GSP card it always exports `NVD_BACKEND=direct`. When the display detector succeeds it also exports `LIBVA_DRIVER_NAME=nvidia` and `__GLX_VENDOR_LIBRARY_NAME=nvidia`. Omarchy 4.5 already contains [PR #11690](https://github.com/omacom/omarchy/pull/11690), which added that display check. The check is what still returns the wrong answer here.

`libva-nvidia-driver` provides `/usr/lib/dri/nvidia_drv_video.so`. Chromium decodes through it and then fails to import the frames into the AMD GL context. Upstream describes that import as `eglCreateImage` failing with `EGL_BAD_MATCH` (`0x3009`). The video layer never gets a texture. Firefox was the driver this library was written for. On the same hybrid layout it tends to fail the NVIDIA decoder and fall back to the CPU instead of showing black ([issue #13549](https://github.com/omacom/omarchy/issues/13549)).

Separately, Omarchy's browser flags force the Wayland path. `omarchy-install-browser` copies `/usr/share/omarchy/config/chromium-flags.conf` to `~/.config/brave-flags.conf` and `~/.config/chromium-flags.conf`, and `/usr/bin/brave` prepends that file. Both copies contain `--ozone-platform=wayland`. `omarchy-refresh-chromium` can write the Wayland lines back over a local edit.

## Why the power draw changes

The iGPU is already `active` because it owns the panel. The NVIDIA card can runtime-suspend when nothing has it open. Power Hub's GPU row is that state: **Asleep** only when the discrete device's `power/runtime_status` is `suspended`.

`LIBVA_DRIVER_NAME=nvidia` sends VA-API clients at the NVIDIA decoder. Opening that device moves `runtime_status` to `active` and holds it there for the whole playback, including paused tabs that keep a decoder. `__GLX_VENDOR_LIBRARY_NAME=nvidia` can do the same for GL, not only for video. The panel then reads **Awake**, and the battery curve includes the discrete GPU's active idle on top of the iGPU. Frames also have to be copied back to the GPU that is actually scanning out.

There is no watt figure for this laptop in this note. The earlier Power Hub poll benchmark stayed asleep across its own 5-second GPU reads, because those reads use sysfs and do not touch PCI config space. `lspci` does, and that alone resumes a suspended NVIDIA device. An **Awake** row while the panel is only polling is some other client. A browser or `mpv` using the NVIDIA VA-API driver is one of them. [Issue #14285](https://github.com/omacom/omarchy/issues/14285) reports that `mpv` still wakes a runtime-suspended NVIDIA card on hybrid laptops after #11690.

Power Hub's Wi-Fi, PCIe, USB, audio, panel, and effects tweaks were all off while the video was black, and the PCIe saving switch was pinned off. Travel mode does not change GPU mode. None of those controls write `LIBVA_DRIVER_NAME`.

## Check it

Skip `lspci` until you are willing to wake the card.

```bash
omarchy-hw-nvidia-display; echo "nvidia-display exit:$?"
```

Exit 0 means Omarchy thinks NVIDIA drives the display. On this hardware that is the bug.

```bash
for d in /sys/bus/pci/devices/*; do
  vendor=$(cat "$d/vendor")
  class=$(cat "$d/class")
  [[ $class == 0x03* ]] || continue
  if [[ -f $d/boot_vga ]]; then boot=$(cat "$d/boot_vga"); else boot="<missing>"; fi
  echo "$(basename "$d") vendor=$vendor class=$class boot_vga=$boot runtime=$(cat "$d/power/runtime_status")"
done
```

The AMD iGPU should be the device with no `boot_vga` file. The NVIDIA device should show `boot_vga=0`. If `omarchy-hw-nvidia-display` still exits 0, the session is in the bad branch.

From a terminal Hyprland started:

```bash
tr '\0' '\n' < /proc/$$/environ | grep -E '^(LIBVA_DRIVER_NAME|NVD_BACKEND|__GLX_VENDOR_LIBRARY_NAME)='
```

`LIBVA_DRIVER_NAME=nvidia` together with an AMD-owned panel is the black-video configuration. `bin/power-hub gpu` should agree with the NVIDIA device's `runtime_status`.

## Until upstream fixes the detector

The change that lets the discrete GPU sleep is to stop pointing VA-API and GLX at NVIDIA while the iGPU owns the panel. The open pull request that picks the display from the connected monitor, and uses `boot_vga` only as a fallback, is [PR #13659](https://github.com/omacom/omarchy/pull/13659). [Issue #13785](https://github.com/omacom/omarchy/issues/13785) described the black Wayland video and was closed when #11690 merged. #15148 is the hole that fix left for this iGPU.

Do not edit `/usr/share/omarchy`. Package updates replace it. Hyprland loads `~/.config/hypr/hyprland.lua` after the Omarchy defaults, so a later `hl.env` replaces the value for processes started after the next login:

```lua
hl.env("LIBVA_DRIVER_NAME", "radeonsi")
```

`radeonsi` is the Mesa VA-API driver installed on this machine (`/usr/lib/dri/radeonsi_drv_video.so`). That is the AMD counterpart of the `LIBVA_DRIVER_NAME=iHD` override people used on Intel hybrid laptops ([issue #8215](https://github.com/omacom/omarchy/issues/8215)). Log out and back in, then confirm the variable and play one video. The picture should return with decode on the iGPU, and the NVIDIA `runtime_status` should be able to return to `suspended` once nothing else has the card open. This override was not re-run as a playback test here. The tests that were run are the ones above: software rendering, and X11 on each render node.

`__GLX_VENDOR_LIBRARY_NAME=nvidia` is a separate wake path for GL clients. If video is fine and the card stays `active` with no NVIDIA process, that variable is the next one to stop exporting.

Two workarounds restore a picture and leave the bad environment in place:

- Hardware acceleration off in Brave uses the CPU for decode and compositing. The discrete GPU is no longer needed for that video. CPU time goes up instead.
- `--ozone-platform=x11` in `~/.config/brave-flags.conf` (and the Chromium file, if that browser is affected) made hardware-accelerated playback work in a temporary profile. Quit every Brave process before judging a flags change. `omarchy-refresh-chromium` and a browser reinstall copy the Wayland defaults back over those files.

[PR #13189](https://github.com/omacom/omarchy/pull/13189) would force software video decode in Chromium whenever the NVIDIA display detector succeeds. On this laptop the detector is wrong, so that change would hide the black video by moving decode to the CPU. It would not put decode on the Radeon, and it is still open. [Issue #13188](https://github.com/omacom/omarchy/issues/13188) is the other configuration, where NVIDIA really does drive the display.

[PR #6333](https://github.com/omacom/omarchy/pull/6333) is the Wayland Vulkan crash (`--ozone-platform=wayland is not compatible with Vulkan`). That matches the `SIGTRAP` from the NVIDIA render-node test. It is not the black video. [Issue #8718](https://github.com/omacom/omarchy/issues/8718) is Brave video going transparent, including on AMD-only machines.
