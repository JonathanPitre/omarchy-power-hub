import QtQuick
import Quickshell
import Quickshell.Io

// Runs the plugin's bin/ helpers and exposes their JSON as reactive state.
// One poll process on a timer while the panel is open; a separate action
// process for user-triggered changes (both print the same JSON shape).
Item {
  id: root

  property string binDir: ""
  property int pollInterval: 5000
  property bool live: false

  property var snapshot: ({
    battery: { present: false, percent: 0, status: "Unknown", watts: 0, health: 0,
               whFull: 0, whDesign: 0, cycles: 0, minutes: 0, acOnline: 0, fraction: 0 },
    rapl:   { available: false, package: 0, core: 0, gpu: 0 },
    tlp:    { installed: false, active: false, mode: "unknown", manual: false },
    tweaks: { wifi: false, usb: false, pcie: false, audio: false },
    sleep:  { installed: false },
    wifiFix: { chipPresent: false, installed: false, problemSeen: false, failCount: 0 }
  })

  property bool busy: actionProc.running

  readonly property var battery: snapshot.battery
  readonly property var rapl: snapshot.rapl
  readonly property var tlp: snapshot.tlp
  readonly property var tweaks: snapshot.tweaks
  readonly property var sleep: snapshot.sleep
  readonly property var wifiFix: snapshot.wifiFix

  signal updated()

  function _ingest(text) {
    var parsed
    try { parsed = JSON.parse(String(text || "").trim()) } catch (e) { return }
    if (!parsed || !parsed.battery) return
    root.snapshot = parsed
    root.updated()
  }

  function refresh() {
    if (pollProc.running) return
    pollProc.command = [root.binDir + "/battery-plus-data"]
    pollProc.running = true
  }

  function act(verb, arg, arg2) {
    if (actionProc.running) return
    actionProc.command = [root.binDir + "/battery-plus-action",
                          String(verb), String(arg === undefined ? "" : arg),
                          String(arg2 === undefined ? "" : arg2)]
    actionProc.running = true
  }

  function openTerminal(which) {
    Quickshell.execDetached([root.binDir + "/battery-plus-action", String(which)])
  }

  Process {
    id: pollProc
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root._ingest(text) }
  }

  Process {
    id: actionProc
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root._ingest(text) }
  }

  // Poll fast while the panel is open; keep a slow background poll otherwise
  // so opening the panel shows fresh data.
  Timer {
    interval: root.live ? root.pollInterval : 60000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }
}
