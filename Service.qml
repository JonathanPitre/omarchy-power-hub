import QtQuick
import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Services.UPower
import "Model.js" as Model

Item {
  id: root

  readonly property string ops: String(Qt.resolvedUrl("bin/ops.py")).replace(/^file:\/\//, "")
  readonly property string priv: String(Qt.resolvedUrl("bin/ops-priv")).replace(/^file:\/\//, "")
  property var state: ({})
  property bool displaysOff: false
  property int displaysTimeout: 0
  property int saverTimeout: 0
  property int lockTimeout: 0
  property int sleepTimeout: 0
  property int hibernateAfter: 0
  property bool parked: false
  property var parkQueue: []
  readonly property bool saverOn: ToplevelManager.toplevels.values.some(function(t) {
    return t.appId === "org.omarchy.screensaver"
  })
  property bool lidClosed: false
  property bool saverArmed: false


  property bool refreshAgain: false

  function refresh() {
    if (statusProc.running) {
      root.refreshAgain = true
      return
    }
    statusProc.command = ["python3", root.ops, "config"]
    statusProc.running = true
  }

  // Panel edits land here within ~a second instead of on the 30 s poll. The
  // debounce skips the truncated file between ops.py's truncate and write.
  FileView {
    path: Quickshell.env("HOME") + "/.config/omarchy/omarchy-power-hub.json"
    watchChanges: true
    onFileChanged: configDebounce.restart()
  }
  Timer {
    id: configDebounce
    interval: 150
    onTriggered: root.refresh()
  }

  function ingest(text) {
    var parsed
    try { parsed = JSON.parse(String(text || "")) } catch (e) { return }
    root.state = parsed
    var side = parsed.side === "battery" ? "battery" : "ac"
    var src = (parsed.config && parsed.config[side]) || ({})
    root.displaysTimeout = Number(src.displaysOff) || 0
    root.saverTimeout = Model.effectiveSaver(src.screensaver, src.displaysOff)
    root.lockTimeout = Number(src.lock) || 0
    root.sleepTimeout = Number(src.sleep) || 0
    root.hibernateAfter = Number(src.hibernateAfter) || 0
  }

  property bool displaysIdle: false

  function dpms(mode) {
    if (mode === "on" || mode === "off") root.dismissScreensaver()
    dpmsProc.command = ["python3", root.ops, "displays", mode]
    dpmsProc.running = true
    root.displaysOff = mode === "off"
  }

  function dismissScreensaver() {
    if (!dismissProc.running) dismissProc.running = true
  }

  function park(name) {
    if (!name) return
    root.parkQueue = root.parkQueue.concat([name])
    root.nextPark()
  }

  function nextPark() {
    if (parkProc.running || root.parkQueue.length === 0) return
    var name = root.parkQueue[0]
    root.parkQueue = root.parkQueue.slice(1)
    parkProc.command = ["python3", root.ops, "displays", "park", name]
    parkProc.running = true
  }

  property var saverWindows: ({})

  function saverWindow(address, open) {
    var next = Object.assign({}, root.saverWindows)
    if (open) next[address] = true
    else delete next[address]
    root.saverWindows = next
    if (open) saverSettle.restart()
    else if (Object.keys(next).length === 0) {
      saverSettle.stop()
      root.saverArmed = false
    }
  }

  Connections {
    target: Hyprland
    function onRawEvent(event) {
      var name = String(event && event.name || "")
      var data = String(event && event.data || "")
      if (name === "monitoradded") root.park(data.trim())
      else if (name === "openwindow" && data.split(",")[2] === "org.omarchy.screensaver")
        root.saverWindow(data.split(",")[0], true)
      else if (name === "closewindow" && root.saverWindows[data]) root.saverWindow(data, false)
    }
  }

  // Launching the screensaver hops focus across monitors, which reads as input.
  // Arm once the last window has been up long enough for that to go idle. Still
  // active then means real input, which may be continuous and never toggle again.
  Timer {
    id: saverSettle
    interval: 2000
    onTriggered: {
      if (saverPointer.isIdle) root.saverArmed = true
      else root.dismissScreensaver()
    }
  }

  Process {
    id: parkProc
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: if (text.trim() === "parked") root.parked = true
    }
    onExited: root.nextPark()
  }

  Process {
    id: syncProc
    command: ["python3", root.ops, "displays", "sync"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var out = text.trim()
        if (out === "restored" || out === "idle") root.parked = false
        else if (out === "waiting") root.parked = true
      }
    }
  }

  Timer {
    interval: 1000
    running: root.parked || root.displaysOff
    repeat: true
    onTriggered: {
      if (!wakePoll.running) {
        wakePoll.command = ["python3", root.ops, "idle-wake-poll",
                            root.displaysOff ? "blanked" : "on"]
        wakePoll.running = true
      }
      if (root.parked && !syncProc.running) syncProc.running = true
    }
  }

  Process {
    id: wakePoll
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var out = text.trim()
        if (out === "closed") root.lidClosed = true
        else root.lidClosed = false
        if (out === "wake") {
          root.displaysIdle = false
          if (root.displaysOff) root.dpms("on")
          else root.dismissScreensaver()
          return
        }
        if (root.displaysOff && root.displaysIdle && !dpmsProc.running) root.dpms("off")
      }
    }
  }

  Process {
    id: statusProc
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.ingest(text) }
    onExited: if (root.refreshAgain) {
      root.refreshAgain = false
      root.refresh()
    }
  }
  Process { id: dpmsProc }
  Process {
    id: dismissProc
    command: ["bash", "-lc", "pkill -x ttfx; pkill -f '[o]rg.omarchy.screensaver'"]
  }
  Process { id: sleepProc }
  Process { id: boostProc; command: ["python3", root.ops, "boost-check"] }
  Process {
    id: inhibitProc
    running: true
    command: ["systemd-inhibit", "--what=handle-lid-switch", "--who=omarchy-power-hub",
              "--why=managed lid action", "--mode=block", "sleep", "infinity"]
  }

  // Ignoring inhibitors makes this track real input only, so the launcher's
  // focus hops across monitors don't count as the user waking the screensaver.
  // The cursor is hidden on the screensaver and cursorpos can't be used.
  IdleMonitor {
    id: saverPointer
    timeout: 1
    respectInhibitors: false
    onIsIdleChanged: if (!isIdle && root.saverArmed) {
      root.saverArmed = false
      root.dismissScreensaver()
    }
  }

  Process { id: saverLaunchProc; command: ["python3", root.ops, "saver-past-inhibitor"] }

  // IdleMonitor does not re-arm when its timeout or enabled state changes after
  // creation, so each new timeout gets a fresh monitor.
  Instantiator {
    model: root.saverTimeout > 0 ? [root.saverTimeout] : []
    delegate: IdleMonitor {
      timeout: modelData
      respectInhibitors: false
      onIsIdleChanged: {
        if (isIdle && !root.displaysOff && !root.saverOn && !saverLaunchProc.running)
          saverLaunchProc.running = true
      }
    }
  }

  Instantiator {
    model: root.displaysTimeout > 0 ? [root.displaysTimeout] : []
    delegate: IdleMonitor {
      timeout: modelData
      respectInhibitors: true
      onIsIdleChanged: {
        root.displaysIdle = isIdle
        if (isIdle) root.dpms("off")
        else if (root.displaysOff && !root.lidClosed) root.dpms("on")
      }
    }
  }


  // Holds the lock deadline across Displays off, including a tie: see idle_lock in ops.py.
  Timer {
    interval: Math.max(1, (root.lockTimeout - root.displaysTimeout) * 1000)
    running: root.displaysIdle && root.lockTimeout >= root.displaysTimeout && root.lockTimeout > 0
    onTriggered: if (!idleLockProc.running) idleLockProc.running = true
  }
  Process { id: idleLockProc; command: ["python3", root.ops, "idle-lock"] }

  Instantiator {
    model: root.sleepTimeout > 0 ? [root.sleepTimeout] : []
    delegate: IdleMonitor {
      timeout: modelData
      respectInhibitors: true
      onIsIdleChanged: {
        if (!isIdle) return
        sleepProc.command = root.hibernateAfter > 0
            ? ["sh", "-c", "systemctl suspend-then-hibernate || systemctl suspend"]
            : ["systemctl", "suspend"]
        sleepProc.running = true
      }
    }
  }

  Timer {
    interval: 30000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: {
      root.refresh()
      if (!boostProc.running) boostProc.running = true
    }
  }

  Process { id: applyProc; command: ["python3", root.ops, "apply"] }
  Process { id: applyProfileProc; onExited: root.refresh() }
  Timer {
    id: applyProfileDebounce
    interval: 300
    repeat: false
    onTriggered: {
      if (applyProfileProc.running) return
      applyProfileProc.command = ["python3", root.ops, "apply-profile"]
      applyProfileProc.running = true
    }
  }
  Connections {
    target: UPower
    function onOnBatteryChanged() { applyProfileDebounce.restart() }
  }

  // Plugging back in turns Travel mode off. Keyed on the battery leaving
  // Discharging, not on OnBattery: a bus-powered USB-C monitor keeps AC online
  // after unplug, so OnBattery may never change. A Travel session started while
  // already plugged in sees no such transition and stays on.
  property int lastBatteryState: -1
  Process { id: travelOffProc; command: ["python3", root.ops, "travel", "off"]; onExited: root.refresh() }
  Connections {
    target: UPower.displayDevice
    function onStateChanged() {
      var s = UPower.displayDevice.state
      // The same monitor means OnBattery may miss the switch, so re-apply the profile here too.
      if ((s === UPowerDeviceState.Discharging) !== (root.lastBatteryState === UPowerDeviceState.Discharging))
        applyProfileDebounce.restart()
      if (root.lastBatteryState === UPowerDeviceState.Discharging
          && (s === UPowerDeviceState.Charging || s === UPowerDeviceState.FullyCharged
              || s === UPowerDeviceState.PendingCharge)
          && !travelOffProc.running)
        travelOffProc.running = true
      root.lastBatteryState = s
    }
  }
  Process {
    id: ledNeedProc
    command: ["python3", root.ops, "led-sudo-needed"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: if (text.trim() === "needed") ledPkexec.running = true
    }
  }
  Process { id: ledPkexec; command: ["pkexec", root.priv, "led-sudoers"] }
  Process {
    id: helperNeedProc
    command: ["python3", root.ops, "helper-needed"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        if (text.trim() === "needed") helperPkexec.running = true
        else applyProc.running = true
      }
    }
  }
  Process {
    id: helperPkexec
    command: ["pkexec", root.priv, "helper-install"]
    onExited: applyProc.running = true
  }
  Component.onCompleted: {
    root.refresh()
    syncProc.running = true
    ledNeedProc.running = true
    helperNeedProc.running = true
  }
}
