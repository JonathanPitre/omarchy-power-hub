import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

Column {
  id: root
  spacing: Style.space(10)

  property color fg: Color.foreground
  property color dim: Qt.darker(fg, 1.5)
  property string fontFamily: Style.font.family
  property var state: ({})
  property bool opened: false
  property int pollMs: 5000
  readonly property int headsetPollMs: 1000
  property bool reduceCpuPerformance: false
  property bool energyPending: false
  property bool energyRestoring: false
  property string energyError: ""
  property string energyErrorAction: ""
  readonly property var travelStatus: root.state.travel || ({ phase: "off", reduceCpu: false, controls: ({}), error: "" })
  signal energyChanged()
  property var logiRows: []
  property var headsetRows: []
  readonly property var devices: logiRows.concat(headsetRows)
  property bool logiRestart: false
  property var gpu: ({})
  // Clicks show at once and queue their writes; a status read that started
  // before the latest click is dropped so it cannot undo the highlight.
  property var queue: []
  property int edits: 0
  property bool reloadAgain: false

  readonly property string ops: String(Qt.resolvedUrl("bin/ops.py")).replace(/^file:\/\//, "")
  readonly property string logi: String(Qt.resolvedUrl("bin/logi.py")).replace(/^file:\/\//, "")
  readonly property var sides: [
    { side: "ac", title: "PLUGGED IN", icon: "󰚥" },
    { side: "battery", title: "BATTERY", icon: "󰁹" }
  ]
  readonly property real colWidth: Style.space(152)
  readonly property real colGap: Style.space(6)

  function sideCfg(side) {
    var cfg = root.state.config || ({})
    return cfg[side] || ({})
  }

  function reload() {
    root.loadStatus()
    root.refreshDevices()
  }

  function refreshDevices() {
    if (!devProc.running) devProc.running = true
  }

  // The Logitech watcher lives only while the panel is open; Re-scan
  // restarts it and re-identifies every device.
  function startLogi(identify) {
    if (!root.opened) return
    logiProc.command = ["python3", root.logi, "watch", "--poll", String(Math.round(root.pollMs / 1000))]
      .concat(identify ? ["--identify"] : [])
    if (logiProc.running) {
      root.logiRestart = true
      logiProc.running = false
      return
    }
    logiProc.running = true
  }

  function rescan() {
    root.refreshDevices()
    root.startLogi(true)
  }

  onOpenedChanged: {
    if (root.opened) root.startLogi(false)
    else logiProc.running = false
  }

  function loadStatus() {
    if (proc.running) {
      root.reloadAgain = true
      return
    }
    proc.startedAt = root.edits
    proc.running = true
  }

  function call(args) {
    if (args[0] === "set") {
      var cfg = Object.assign({}, root.state.config || {})
      var src = Object.assign({}, cfg[args[1]] || {})
      src[args[2]] = /^\d+$/.test(args[3]) ? Number(args[3]) : args[3]
      cfg[args[1]] = src
      root.state = Object.assign({}, root.state, { config: cfg })
      var id = args.slice(0, 3).join(" ")
      root.queue = root.queue.filter(function(q) { return q.slice(0, 3).join(" ") !== id })
    }
    if (args[0] === "travel" || args[0] === "quick-dim") {
      root.energyPending = true
      root.energyRestoring = args[1] === "off"
      root.energyErrorAction = args[0]
      root.energyError = ""
    }
    root.edits++
    root.queue = root.queue.concat([args])
    root.next()
  }

  function focusEnergy(which) {
    var item = which === "travel" ? energyTravelButton
      : which === "quickDim" ? energyDimButton
      : which === "cpu" ? reduceCpuCheck : null
    if (item && item.enabled) item.forceActiveFocus()
    return item && item.enabled ? item : null
  }

  function activateEnergyFocus() {
    if (energyTravelButton.activeFocus && energyTravelButton.enabled) energyTravelButton.clicked()
    else if (energyDimButton.activeFocus && energyDimButton.enabled) energyDimButton.clicked()
    else if (reduceCpuCheck.activeFocus && reduceCpuCheck.enabled)
      root.reduceCpuPerformance = !root.reduceCpuPerformance
  }

  function next() {
    if (act.running) return
    if (root.queue.length === 0) {
      root.loadStatus()
      return
    }
    act.command = ["python3", root.ops].concat(root.queue[0])
    root.queue = root.queue.slice(1)
    act.energyStdout = ""
    act.energyStderr = ""
    act.running = true
  }

  Process {
    id: proc
    property int startedAt: 0
    command: ["python3", root.ops, "status"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var parsed
        try { parsed = JSON.parse(text) } catch (e) { return }
        if (proc.startedAt !== root.edits) return
        root.state = parsed
        if (parsed.travel && parsed.travel.phase !== "off")
          root.reduceCpuPerformance = !!parsed.travel.reduceCpu
      }
    }
    onExited: if (root.reloadAgain) {
      root.reloadAgain = false
      root.loadStatus()
    }
  }
  Process {
    id: devProc
    command: ["python3", root.ops, "devices"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var parsed
        try { parsed = JSON.parse(text) } catch (e) { return }
        root.headsetRows = parsed.devices || []
        root.gpu = parsed.gpu || ({})
      }
    }
  }
  Process {
    id: logiProc
    stdout: SplitParser {
      onRead: function(line) {
        var parsed
        try { parsed = JSON.parse(line) } catch (e) { return }
        root.logiRows = parsed.devices || []
      }
    }
    onExited: if (root.logiRestart) {
      root.logiRestart = false
      logiProc.running = true
    }
  }
  Timer {
    interval: root.headsetPollMs
    repeat: true
    running: root.opened
    onTriggered: root.refreshDevices()
  }
  Process {
    id: act
    property string energyStdout: ""
    property string energyStderr: ""
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: act.energyStdout = text }
    stderr: StdioCollector { waitForEnd: true; onStreamFinished: act.energyStderr = text }
    onExited: function(exitCode) {
      if (act.command[2] === "travel" || act.command[2] === "quick-dim") {
        root.energyPending = false
        root.energyError = exitCode === 0 ? "" : (act.energyStderr.trim() || act.energyStdout.trim() || "Energy setting could not be changed")
        root.energyChanged()
      }
      root.next()
    }
  }

  Component.onCompleted: root.reload()

  PanelSeparator { foreground: root.fg }

  Column {
    width: parent.width
    spacing: Style.space(4)
    Item {
      width: parent.width
      implicitHeight: Math.max(devHeader.implicitHeight, refreshBtn.implicitHeight)
      PanelSectionHeader {
        id: devHeader
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: "DEVICES"
        foreground: root.fg
        fontFamily: root.fontFamily
      }
      PanelActionButton {
        id: refreshBtn
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        iconText: "\uf021"
        tooltipText: "Re-scan peripheral batteries"
        foreground: root.dim
        hoverColor: root.fg
        fontFamily: root.fontFamily
        onClicked: root.rescan()
      }
    }
    Text {
      width: parent.width
      visible: root.devices.length === 0
      text: "No battery readings"
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
    Repeater {
      model: root.devices
      delegate: RowLayout {
        TextMetrics {
          id: percentMetrics
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
          text: "100%"
        }
        width: root.width
        height: Style.font.body * 1.6
        spacing: Style.space(8)
        Text {
          Layout.preferredWidth: Style.space(16)
          horizontalAlignment: Text.AlignHCenter
          text: Model.kindIcon(modelData.kind)
          color: modelData.percent <= 10 ? Color.urgent : root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
        }
        Text {
          Layout.fillWidth: true
          elide: Text.ElideRight
          text: modelData.name
          color: modelData.percent <= 10 ? Color.urgent : root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
        }
        Item {
          Layout.preferredWidth: 72
          height: parent.height
          Rectangle {
            width: parent.width
            height: 6
            anchors.verticalCenter: parent.verticalCenter
            radius: 3
            color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.15)
            Rectangle {
              width: parent.width * (modelData.percent / 100)
              height: parent.height
              radius: 3
              color: modelData.percent <= 10 ? Color.urgent : root.fg
            }
          }
        }
        Text {
          Layout.preferredWidth: percentMetrics.width
          horizontalAlignment: Text.AlignRight
          text: modelData.percent + "%"
          color: modelData.percent <= 10 ? Color.urgent : root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
        }
        Text {
          Layout.preferredWidth: Style.space(16)
          horizontalAlignment: Text.AlignHCenter
          text: Model.batteryIcon(modelData.percent, modelData.status)
          color: modelData.percent <= 10 ? Color.urgent : root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
        }
      }
    }
  }

  Item {
    width: parent.width
    implicitHeight: idleHeader.implicitHeight

    PanelSectionHeader {
      id: idleHeader
      anchors.left: parent.left
      anchors.verticalCenter: parent.verticalCenter
      text: "POWER & BATTERY"
      foreground: root.fg
      fontFamily: root.fontFamily
    }
    Row {
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      spacing: root.colGap
      Repeater {
        model: root.sides
        delegate: Row {
          required property var modelData
          readonly property bool active: root.state.side === modelData.side
          width: root.colWidth
          spacing: Style.space(4)
          Text {
            id: sideIcon
            text: modelData.icon
            color: parent.active ? root.fg : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
          }
          Text {
            width: Math.max(0, parent.width - sideIcon.implicitWidth - parent.spacing)
            text: modelData.title
            color: parent.active ? root.fg : root.dim
            elide: Text.ElideRight
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            font.bold: parent.active
          }
        }
      }
    }
  }

  Repeater {
    model: [
      { key: "profile", label: "Power mode" },
      { key: "screensaver", label: "Screen saver", stops: Model.IDLE_STOPS },
      { key: "displaysOff", label: "Displays off", stops: Model.IDLE_STOPS },
      { key: "lock", label: "Auto-lock", stops: Model.IDLE_STOPS },
      { key: "sleep", label: "Sleep after", stops: Model.HIBERNATE_STOPS },
      { key: "hibernateAfter", label: "Hibernate after", stops: Model.HIBERNATE_STOPS },
      { key: "lid", label: "When lid closes" }
    ]
    delegate: Item {
      id: idleRow
      required property var modelData
      width: root.width
      implicitHeight: Style.spacing.controlHeight

      Text {
        anchors.left: parent.left
        anchors.right: idleDrops.left
        anchors.rightMargin: root.colGap
        anchors.verticalCenter: parent.verticalCenter
        text: idleRow.modelData.label
        elide: Text.ElideRight
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Row {
        id: idleDrops
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: root.colGap
        Repeater {
          model: root.sides
          delegate: Dropdown {
            required property var modelData
            readonly property var saved: root.sideCfg(modelData.side)[idleRow.modelData.key]
            width: root.colWidth
            showLabel: false
            fontFamily: root.fontFamily
            options: idleRow.modelData.key === "profile" ? Model.PROFILE_OPTIONS
              : idleRow.modelData.key === "lid" ? Model.LID_OPTIONS
              : Model.idleOptions(idleRow.modelData.stops, saved)
            value: idleRow.modelData.key === "profile" ? String(saved || "balanced")
              : idleRow.modelData.key === "lid" ? String(saved || "suspend")
              : String(Math.max(0, Math.round(Number(saved) || 0)))
            onChanged: function(v) {
              if (v !== String(saved)) root.call(["set", modelData.side, idleRow.modelData.key, v])
            }
          }
        }
      }
    }
  }

  Text {
    width: parent.width
    visible: !!(root.state.hibernate && root.state.hibernate.available === false)
    text: (root.state.hibernate && root.state.hibernate.reason) || "Hibernate is unavailable"
    color: root.dim
    wrapMode: Text.WordWrap
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
  }

  Column {
    width: parent.width
    visible: !!(root.state.framework) || (root.state.charge || {}).kind === "upower"
    spacing: Style.space(6)
    Item {
      width: parent.width
      implicitHeight: Math.max(chargeHeader.implicitHeight, chargeInfo.implicitHeight)
      PanelSectionHeader {
        id: chargeHeader
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: (root.state.charge || {}).kind === "upower" ? "BATTERY HEALTH" : "CHARGE LIMIT"
        foreground: root.fg
        fontFamily: root.fontFamily
      }
      PanelActionButton {
        id: chargeInfo
        visible: (root.state.charge || {}).kind === "framework"
        anchors.left: chargeHeader.right
        anchors.leftMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        iconText: ""
        tooltipText: "80% keeps the battery healthy when you're plugged in most of the day. Battery Extender in the BIOS overrides this limit, so turn it off."
        foreground: root.dim
        hoverColor: root.fg
        fontFamily: root.fontFamily
      }
    }
    Item {
      readonly property var c: root.state.charge || ({})
      readonly property bool ready: !!c.tool && c.limit !== null && c.limit !== undefined
      readonly property bool boosting: !!root.state.boost
      readonly property real side: Style.space(40)
      width: parent.width
      height: side
      visible: (root.state.charge || {}).kind !== "upower"
      id: chargeRow

      Text {
        anchors.left: parent.left
        anchors.right: allowBtn.visible ? allowBtn.left : parent.right
        anchors.verticalCenter: parent.verticalCenter
        visible: !chargeRow.ready
        text: !chargeRow.c.tool ? (chargeRow.c.reason || "framework_tool is not installed")
          : chargeRow.c.reason === "permission" ? "Needs permission"
          : (chargeRow.c.reason || "Charge limit unavailable")
        color: root.fg
        elide: Text.ElideRight
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
      }
      Button {
        id: allowBtn
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        visible: !chargeRow.ready && chargeRow.c.reason === "permission"
        text: "Allow"
        bordered: true
        foreground: root.fg
        fontFamily: root.fontFamily
        onClicked: root.call(["helper-install"])
      }

      Row {
        visible: chargeRow.ready
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(8)
        Button {
          width: chargeRow.side
          height: chargeRow.side
          text: "−"
          bordered: true
          enabled: !chargeRow.boosting && chargeRow.c.limit > 40
          opacity: enabled ? 1 : 0.35
          foreground: root.fg
          fontFamily: root.fontFamily
          fontSize: Style.font.title
          onClicked: root.call(["charge", String(Math.max(40, Number(chargeRow.c.limit) - 10))])
        }
        Text {
          width: Style.space(64)
          anchors.verticalCenter: parent.verticalCenter
          horizontalAlignment: Text.AlignHCenter
          text: chargeRow.c.limit + "%"
          color: root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.heading
          font.bold: true
        }
        Button {
          width: chargeRow.side
          height: chargeRow.side
          text: "+"
          bordered: true
          enabled: !chargeRow.boosting && chargeRow.c.limit < 100
          opacity: enabled ? 1 : 0.35
          foreground: root.fg
          fontFamily: root.fontFamily
          fontSize: Style.font.title
          onClicked: root.call(["charge", String(Math.min(100, Number(chargeRow.c.limit) + 10))])
        }
      }
      Button {
        visible: chargeRow.ready
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(128)
        height: chargeRow.side
        iconText: ""
        text: chargeRow.boosting ? "Boosting" : "Boost"
        bordered: true
        selected: chargeRow.boosting
        foreground: root.fg
        fontFamily: root.fontFamily
        onClicked: root.call(["boost", chargeRow.boosting ? "off" : "on"])
      }
    }
    Text {
      width: parent.width
      color: root.fg
      font.family: root.fontFamily
      font.pixelSize: Style.font.body
      wrapMode: Text.Wrap
      visible: (root.state.charge || {}).kind === "upower"
      text: {
        var c = root.state.charge || ({})
        var p = String(c.policy || "")
        if (p === "firmware")
          return c.health ? "Uses firmware optimized charging" : "Use firmware optimized charging"
        if (p.indexOf("-") >= 0) {
          var parts = p.split("-")
          return (c.health ? "Starts charging at " : "Start charging at ") + parts[0] + "% and " + (c.health ? "stops" : "stop") + " at " + parts[1] + "%"
        }
        if (p)
          return (c.health ? "Stops charging at " : "Limit charging to ") + p + "%"
        return c.health ? "Battery health on" : "Charge to 100% for travel"
      }
    }
    Button {
      visible: (root.state.charge || {}).kind === "upower"
      text: (root.state.charge || {}).health ? "Battery health on" : "Battery health"
      bordered: true
      selected: !!(root.state.charge && root.state.charge.health)
      foreground: root.fg
      fontFamily: root.fontFamily
      onClicked: root.call(["health", (root.state.charge || {}).health ? "off" : "on"])
    }
  }

  readonly property bool cpuReductionAvailable:
    !!(((root.travelStatus.controls || {}).profile || {}).available)
    || !!(((root.travelStatus.controls || {}).turbo || {}).available)
    || !!(((root.travelStatus.controls || {}).epp || {}).available)
  readonly property bool travelOwned: root.travelStatus.phase !== "off"
  readonly property bool travelRefreshActionable:
    !!(((root.travelStatus.controls || {}).refresh || {}).available)
    && ((((root.travelStatus.controls || {}).refresh || {}).detail || "").indexOf("Laptop refresh capped at ") === 0)
  readonly property bool travelCanEnable:
    !!(((root.travelStatus.controls || {}).brightness || {}).available)
    || root.travelRefreshActionable
    || (root.reduceCpuPerformance && root.cpuReductionAvailable)
  readonly property var quickDimStatus: root.state.quickDim || ({ on: false, available: false, error: "" })

  BorderSurface {
    width: parent.width
    height: travelCard.implicitHeight + Style.space(24)
    radius: Style.cornerRadius
    color: Style.normalFillFor(root.fg, Color.accent)
    borderSpec: Border.controlSpec("normal", root.fg, Color.accent)
    Column {
      id: travelCard
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.top: parent.top
      anchors.margins: Style.space(12)
      spacing: Style.space(7)
      Row {
        width: parent.width
        spacing: Style.space(8)
        Text {
          width: parent.width - travelStateLabel.implicitWidth - parent.spacing
          text: "Travel mode"
          color: root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.title
          font.bold: true
        }
        Text {
          id: travelStateLabel
          text: root.travelStatus.phase === "on" ? "On"
            : root.travelStatus.phase === "incomplete" ? "Incomplete" : "Off"
          color: root.travelStatus.phase === "incomplete" ? Color.urgent : root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
        }
      }
      Text {
        width: parent.width
        text: "Work longer away from an outlet."
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        text: (((root.travelStatus.controls || {}).brightness || {}).available)
          ? "Laptop brightness capped at 40%"
          : "Brightness cannot be read"
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        text: (((root.travelStatus.controls || {}).refresh || {}).available)
          ? (((root.travelStatus.controls || {}).refresh || {}).detail || "Laptop refresh capped at 60 Hz")
          : "No supported mode at or below 60 Hz"
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        visible: !root.travelOwned && !root.travelCanEnable
        text: "No supported battery-saving controls"
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        visible: !root.cpuReductionAvailable
        text: "Unavailable on this system"
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      CheckBox {
        id: reduceCpuCheck
        width: parent.width
        text: "Reduce CPU performance"
        checked: root.reduceCpuPerformance
        enabled: !root.travelOwned && !root.energyPending && root.cpuReductionAvailable
        onToggled: root.reduceCpuPerformance = checked
        Accessible.name: text
        Accessible.description: root.cpuReductionAvailable
          ? "Use power saver, disable turbo boost and prefer CPU energy saving. Heavy tasks may run slower."
          : "Unavailable on this system"
        Accessible.checkable: true
        Accessible.checked: checked
        indicator: BorderSurface {
          x: reduceCpuCheck.leftPadding
          y: (reduceCpuCheck.height - height) / 2
          width: Style.space(18)
          height: Style.space(18)
          color: Style.normalFillFor(root.fg, Color.accent)
          borderSpec: Border.controlSpec("normal", root.fg, Color.accent)
          radius: Style.cornerRadius
          Text {
            anchors.centerIn: parent
            text: reduceCpuCheck.checked ? "✓" : ""
            color: root.fg
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
          }
        }
        contentItem: Text {
          leftPadding: reduceCpuCheck.indicator.width + Style.space(8)
          text: reduceCpuCheck.text
          color: reduceCpuCheck.enabled ? root.fg : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.68)
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
          verticalAlignment: Text.AlignVCenter
        }
      }
      Text {
        width: parent.width
        visible: root.reduceCpuPerformance || root.travelStatus.reduceCpu
        text: {
          var controls = root.travelStatus.controls || ({})
          function available(name) { return !!((controls[name] || {}).available) }
          var epp = controls.epp || ({})
          return (available("profile") ? "Power saver" : "Power saver unavailable")
            + " · " + (available("turbo") ? "CPU turbo off" : "CPU turbo control unavailable")
            + " · " + (available("epp") ? (epp.detail || "Energy-saving CPU preference") : "No saving CPU preference supported")
            + ". Heavy tasks may run slower."
        }
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        text: "CPU performance unchanged"
        visible: !root.reduceCpuPerformance && !root.travelStatus.reduceCpu
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Text {
        width: parent.width
        text: "Wi-Fi, peripherals and GPU mode stay unchanged."
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        text: "Turn off to restore temporary settings and resume your normal power plan."
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Button {
        id: energyTravelButton
        width: parent.width
        text: root.energyPending ? (root.energyRestoring ? "Restoring…" : "Applying…")
          : root.travelStatus.phase === "on" ? "Turn off Travel mode"
          : root.travelStatus.phase === "incomplete" ? "Restore settings" : "Enable Travel mode"
        focusable: true
        bordered: true
        enabled: root.travelOwned ? !root.energyPending
          : !root.energyPending && root.travelCanEnable
        selected: root.travelStatus.phase === "on"
        foreground: enabled ? root.fg : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.62)
        fontFamily: root.fontFamily
        Accessible.role: Accessible.Button
        Accessible.name: text
        Accessible.description: (root.energyErrorAction === "travel" ? root.energyError : "")
          || root.travelStatus.error
          || "Work longer away from an outlet; temporary settings restore when disabled."
        Accessible.checkable: true
        Accessible.checked: root.travelStatus.phase === "on"
        Accessible.onPressAction: if (enabled) clicked()
        onClicked: {
          var enable = !root.travelOwned
          var args = ["travel", enable ? "on" : "off"]
          if (enable && root.reduceCpuPerformance) args.push("--reduce-cpu")
          root.call(args)
        }
      }
      Text {
        width: parent.width
        visible: (root.energyErrorAction === "travel" && root.energyError !== "") || root.travelStatus.error !== ""
        text: root.energyErrorAction === "travel" && root.energyError !== "" ? root.energyError : (root.travelStatus.error || "")
        color: Color.urgent
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
    }
  }

  Row {
    width: parent.width
    spacing: Style.space(8)
    Column {
      width: parent.width - energyDimButton.implicitWidth - parent.spacing
      spacing: Style.space(3)
      Text {
        text: "Quick dim"
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
        font.bold: true
      }
      Text {
        width: parent.width
        text: root.travelStatus.phase === "incomplete"
          ? "Restore Travel mode settings before using Quick dim."
          : root.travelOwned ? "Travel mode is already dimming the screen."
          : "Temporarily cap brightness at 40%."
        color: root.fg
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        width: parent.width
        visible: (root.energyErrorAction === "quick-dim" && root.energyError !== "") || root.quickDimStatus.error !== ""
        text: root.energyErrorAction === "quick-dim" && root.energyError !== "" ? root.energyError : root.quickDimStatus.error
        color: Color.urgent
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
    }
    Button {
      id: energyDimButton
      text: root.quickDimStatus.on ? "Restore brightness" : "Dim screen"
      focusable: true
      bordered: true
      enabled: !root.energyPending && !root.travelOwned && root.quickDimStatus.available
      selected: root.quickDimStatus.on
      foreground: enabled ? root.fg : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.62)
      fontFamily: root.fontFamily
      Accessible.role: Accessible.Button
      Accessible.name: text
      Accessible.description: (root.energyErrorAction === "quick-dim" ? root.energyError : "")
        || root.quickDimStatus.error
        || "Temporarily cap brightness at 40%."
      Accessible.checkable: true
      Accessible.checked: root.quickDimStatus.on
      Accessible.onPressAction: if (enabled) clicked()
      onClicked: root.call(["quick-dim", root.quickDimStatus.on ? "off" : "on"])
    }
  }

  Column {
    width: parent.width
    visible: !!root.gpu.present
    spacing: Style.space(4)
    PanelSectionHeader { text: "GPU"; foreground: root.fg; fontFamily: root.fontFamily }
    Text {
      width: parent.width
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      text: {
        var g = root.gpu
        if (g.asleep) return "Asleep"
        return (g.util || "—") + "%  ·  " + (g.watts || "—") + " W  ·  " + (g.state || "")
      }
    }
  }

}
