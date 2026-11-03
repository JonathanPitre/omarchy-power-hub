# quickshell 0.3.2 rebuild and upstream delivery

- [x] 1. Confirm patched 0.3.2 build exists and matches upstream commit g2b4501a
- [x] 2. Package patched build as quickshell 0.3.2.r7.g2b4501a-1.1
- [x] 3. Verify package binary passes harness in omabox (stock broken, patched fixed)
- [x] 4. Install package, add IgnorePkg entry for quickshell
- [x] 5. Restart Omarchy shell unlocked, confirm live shell uses patched binary
- [x] 6. Prepare PR for upstream delivery (target pending operator decision)
  (decision: back #1097, no duplicate; fork branch review-ready; offer comment held until maintainer responds)
- [x] 7. Update docs/quickshell-idle-timer-failure.md with current status
