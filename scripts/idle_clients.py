#!/usr/bin/env python3
import json, subprocess
raw = subprocess.check_output(["hyprctl", "clients", "-j"], text=True)
data = json.loads(raw)
print("count", len(data))
if data:
    print("keys", sorted(data[0].keys()))
for c in data:
    bits = []
    for k, v in c.items():
        if any(s in k.lower() for s in ("idle", "inhibit", "fullscreen", "floating")):
            bits.append(f"{k}={v}")
    print(c.get("class"), "|", c.get("title"), "|", " ".join(bits))
