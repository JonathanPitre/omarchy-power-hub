import QtQuick
import QtQuick.Controls
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "Model.js" as Model

Column {
  id: root
  spacing: Style.space(10)

  property color fg: Commons.Color.foreground
  property color dim: Qt.darker(fg, 1.5)
  // Commons.Color drops the theme's yellow, so read it from colors.toml.
  property string themeColors: ""
  readonly property color yellow: (themeColors.match(/^\s*(?:yellow|color3)\s*=\s*["']?(#[0-9A-Fa-f]{6})/m) || [, fg])[1]
  FileView {
    id: themeColorsFile
    path: Commons.Color.currentThemePath + "/colors.toml"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.themeColors = text()
  }
  // Theme switches arrive over IPC, so re-read when the accent changes.
  readonly property color themeAccent: Commons.Color.accent
  onThemeAccentChanged: themeColorsFile.reload()
  property string fontFamily: Style.font.family
  property var state: ({})
  property bool opened: false
  property int pollMs: 5000
  property bool energyPending: false
  property bool energyRestoring: false
  property string energyError: ""
  property string energyErrorAction: ""
  readonly property var travelStatus: root.state.travel || ({ phase: "off", reduceCpu: false, controls: ({}), error: "" })
  signal energyChanged()
  property var gpu: ({})
  // Clicks show at once and queue their writes; a status read that started
  // before the latest click is dropped so it cannot undo the highlight.
  property var queue: []
  property int edits: 0
  property bool reloadAgain: false

  readonly property string ops: String(Qt.resolvedUrl("bin/ops.py")).replace(/^file:\/\//, "")
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
    root.refreshGpu()
  }

  function refreshGpu() {
    if (!gpuProc.running) gpuProc.running = true
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
      : which === "quickDim" ? energyDimButton : null
    if (item && item.enabled) item.forceActiveFocus()
    return item && item.enabled ? item : null
  }

  function activateEnergyFocus() {
    if (energyTravelButton.activeFocus && energyTravelButton.enabled) energyTravelButton.clicked()
    else if (energyDimButton.activeFocus && energyDimButton.enabled) energyDimButton.clicked()
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
      }
    }
    onExited: if (root.reloadAgain) {
      root.reloadAgain = false
      root.loadStatus()
    }
  }
  Process {
    id: gpuProc
    command: ["python3", root.ops, "gpu"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var parsed
        try { parsed = JSON.parse(text) } catch (e) { return }
        root.gpu = parsed
      }
    }
  }
  Timer {
    interval: root.pollMs
    repeat: true
    running: root.opened && root.gpu.present !== false
    onTriggered: root.refreshGpu()
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
      AccentButton {
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
        AccentButton {
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
        AccentButton {
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
      PanelActionButton {
        visible: chargeRow.ready
        anchors.right: boostButton.left
        anchors.rightMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        iconText: ""
        tooltipText: "Charges to 100% once, then returns to your limit."
        foreground: root.dim
        hoverColor: root.fg
        fontFamily: root.fontFamily
      }
      AccentButton {
        visible: chargeRow.ready
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(128)
        height: chargeRow.side
        id: boostButton
        bordered: true
        selected: chargeRow.boosting
        foreground: root.fg
        fontFamily: root.fontFamily
        onClicked: root.call(["boost", chargeRow.boosting ? "off" : "on"])
        // Ui.Button tints its icon like the label; the bolt keeps the theme yellow.
        Row {
          anchors.centerIn: parent
          spacing: Style.spacing.controlGap
          Text {
            anchors.verticalCenter: parent.verticalCenter
            text: ""
            color: root.yellow
            font.family: root.fontFamily
            font.pixelSize: Style.font.icon
          }
          Text {
            anchors.verticalCenter: parent.verticalCenter
            text: chargeRow.boosting ? "Boosting" : "Boost"
            color: boostButton.selected ? boostButton._selectedColor : root.fg
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            font.bold: boostButton.selected
          }
        }
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
    AccentButton {
      visible: (root.state.charge || {}).kind === "upower"
      text: (root.state.charge || {}).health ? "Battery health on" : "Battery health"
      bordered: true
      selected: !!(root.state.charge && root.state.charge.health)
      foreground: root.fg
      fontFamily: root.fontFamily
      onClicked: root.call(["health", (root.state.charge || {}).health ? "off" : "on"])
    }
  }

  readonly property bool travelOwned: root.travelStatus.phase !== "off"
  readonly property bool travelRefreshActionable:
    !!(((root.travelStatus.controls || {}).refresh || {}).available)
    && ((((root.travelStatus.controls || {}).refresh || {}).detail || "").indexOf("Laptop refresh capped at ") === 0)
  readonly property bool travelCanEnable:
    !!(((root.travelStatus.controls || {}).brightness || {}).available)
    || root.travelRefreshActionable
    || !!(((root.travelStatus.controls || {}).profile || {}).available)
  readonly property var quickDimStatus: root.state.quickDim || ({ on: false, available: false, error: "" })

  component SectionHeader: Item {
    id: sh
    property string text: ""
    property string help: ""
    width: parent ? parent.width : implicitWidth
    implicitHeight: Math.max(shLabel.implicitHeight, shInfo.implicitHeight)
    PanelSectionHeader {
      id: shLabel
      anchors.left: parent.left
      anchors.verticalCenter: parent.verticalCenter
      text: sh.text
      foreground: root.fg
      fontFamily: root.fontFamily
    }
    PanelActionButton {
      id: shInfo
      anchors.left: shLabel.right
      anchors.leftMargin: Style.space(6)
      anchors.verticalCenter: parent.verticalCenter
      iconText: ""
      tooltipText: sh.help
      foreground: root.dim
      hoverColor: root.fg
      fontFamily: root.fontFamily
    }
  }

  component ValueRow: Item {
    id: vr
    property string label: ""
    property string value: ""
    property bool available: true
    property string help: ""
    default property alias control: vrControl.data
    width: parent ? parent.width : implicitWidth
    implicitHeight: Math.max(Style.space(28), vrControl.childrenRect.height)
    Text {
      id: vrLabel
      anchors.left: parent.left
      anchors.verticalCenter: parent.verticalCenter
      text: vr.label
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.body
    }
    PanelActionButton {
      visible: vr.help !== ""
      anchors.left: vrLabel.right
      anchors.leftMargin: Style.space(6)
      anchors.verticalCenter: parent.verticalCenter
      iconText: ""
      tooltipText: vr.help
      foreground: root.dim
      hoverColor: root.fg
      fontFamily: root.fontFamily
    }
    Text {
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      visible: vrControl.children.length === 0 || !vr.available
      text: vr.available ? vr.value : "Unavailable"
      color: vr.available ? root.fg : root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.body
      font.bold: vr.available
    }
    Item {
      id: vrControl
      visible: vr.available
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      width: childrenRect.width
      height: childrenRect.height
    }
  }

  readonly property var travelControls: root.travelStatus.controls || ({})

  Column {
    width: parent.width
    spacing: Style.space(4)
    SectionHeader {
      text: "TRAVEL MODE"
      help: "Work longer away from an outlet. Wi-Fi, peripherals and GPU mode stay unchanged. Turning it off restores your settings."
    }
    ValueRow {
      label: "Brightness cap"
      value: "40%"
      available: !!(root.travelControls.brightness || {}).available
    }
    ValueRow {
      label: "Refresh cap"
      value: ((root.travelControls.refresh || {}).detail || "60 Hz").replace("Laptop refresh capped at ", "")
      available: !!(root.travelControls.refresh || {}).available
    }
    Text {
      width: parent.width
      visible: !root.travelOwned && !root.travelCanEnable
      text: "No supported battery-saving controls"
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      wrapMode: Text.WordWrap
    }
    Item { width: 1; height: Style.space(4) }
    Item {
      width: parent.width
      implicitHeight: energyTravelButton.height
      AccentButton {
        id: energyTravelButton
        anchors.left: parent.left
        anchors.right: dimInfo.left
        anchors.rightMargin: Style.space(6)
        height: Style.space(40)
        iconText: "\uf072"
        text: root.energyPending ? (root.energyRestoring ? "Restoring…" : "Applying…")
          : root.travelStatus.phase === "on" ? "Turn off Travel mode"
          : root.travelStatus.phase === "incomplete" ? "Restore settings" : "Enable Travel mode"
        focusable: true
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
          root.call(["travel", root.travelOwned ? "off" : "on"])
        }
      }
      PanelActionButton {
        id: dimInfo
        anchors.right: energyDimButton.left
        anchors.rightMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        iconText: ""
        tooltipText: root.travelStatus.phase === "incomplete" ? "Restore Travel mode settings before using Quick dim."
          : root.travelOwned ? "Travel mode is already dimming the screen."
          : "Temporarily cap brightness at 40%."
        foreground: root.dim
        hoverColor: root.fg
        fontFamily: root.fontFamily
      }
      AccentButton {
        id: energyDimButton
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(128)
        height: Style.space(40)
        iconText: "\uf186"
        text: root.quickDimStatus.on ? "Restore" : "Dim screen"
        focusable: true
        enabled: !root.energyPending && !root.travelOwned && root.quickDimStatus.available
        selected: root.quickDimStatus.on
        foreground: enabled ? root.fg : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.62)
        fontFamily: root.fontFamily
        Accessible.role: Accessible.Button
        Accessible.name: root.quickDimStatus.on ? "Restore brightness" : "Dim screen"
        Accessible.description: (root.energyErrorAction === "quick-dim" ? root.energyError : "")
          || root.quickDimStatus.error
          || "Temporarily cap brightness at 40%."
        Accessible.checkable: true
        Accessible.checked: root.quickDimStatus.on
        Accessible.onPressAction: if (enabled) clicked()
        onClicked: root.call(["quick-dim", root.quickDimStatus.on ? "off" : "on"])
      }
    }
    Text {
      width: parent.width
      visible: (root.energyErrorAction === "travel" && root.energyError !== "") || root.travelStatus.error !== ""
      text: root.energyErrorAction === "travel" && root.energyError !== "" ? root.energyError : (root.travelStatus.error || "")
      color: Commons.Color.urgent
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      wrapMode: Text.WordWrap
    }
    Text {
      width: parent.width
      visible: (root.energyErrorAction === "quick-dim" && root.energyError !== "") || root.quickDimStatus.error !== ""
      text: root.energyErrorAction === "quick-dim" && root.energyError !== "" ? root.energyError : root.quickDimStatus.error
      color: Commons.Color.urgent
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      wrapMode: Text.WordWrap
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
      text: root.gpu.asleep ? "Asleep" : "Awake"
    }
  }

}
