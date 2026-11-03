import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import Quickshell.Services.UPower
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "Model.js" as Model

Panel {
  id: root

  moduleName: "omarchy-power-settings"
  ipcTarget: moduleName
  manageIpc: false

  readonly property color fg: bar ? bar.foreground : Commons.Color.foreground
  readonly property color dim: Qt.darker(fg, 1.5)
  readonly property color faint: Qt.rgba(fg.r, fg.g, fg.b, 0.12)
  readonly property color urgent: bar ? bar.urgent : Commons.Color.urgent
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  // Live battery state comes from UPower (event-driven) so the bar icon is
  // always current — the polled JSON service only runs while the panel is
  // open, which used to leave the icon reading 0% until first opened.
  readonly property var dev: UPower.displayDevice
  readonly property bool batPresent: !!(dev && dev.isPresent)
  readonly property int batPct: batPresent ? Math.round(dev.percentage * 100) : 0
  readonly property string batStatus: !batPresent ? "Unknown"
    : dev.state === UPowerDeviceState.Charging ? "Charging"
    : dev.state === UPowerDeviceState.Discharging ? "Discharging"
    : dev.state === UPowerDeviceState.FullyCharged ? "Full"
    : dev.state === UPowerDeviceState.PendingCharge ? "Not charging"
    : "Unknown"
  readonly property bool charging: batStatus === "Charging"

  // Merge live UPower values with the polled extras (cycles, health).
  readonly property var b: ({
    present: root.batPresent,
    percent: root.batPct,
    status: root.batStatus,
    watts: root.batPresent ? Math.abs(Number(dev.changeRate) || 0) : 0,
    minutes: root.batPresent
      ? Math.round((root.charging ? (Number(dev.timeToFull) || 0) : (Number(dev.timeToEmpty) || 0)) / 60)
      : 0,
    fraction: root.batPresent ? Math.max(0, Math.min(1, dev.percentage)) : 0,
    acOnline: Number(service.battery.acOnline) === 1 ? 1 : 0,
    health: Number(service.battery.health) || 0,
    whFull: Number(service.battery.whFull) || 0,
    whDesign: Number(service.battery.whDesign) || 0,
    cycles: Number(service.battery.cycles) || 0
  })
  readonly property var tlp: service.tlp
  readonly property var tweaks: service.tweaks
  readonly property var sleepData: service.sleep
  readonly property var wifiFix: service.wifiFix
  property bool historyOpen: false
  property var historyPts: []

  readonly property bool alwaysShowPercentage: setting("alwaysShowPercentage", false) === true
  readonly property bool vertical: bar ? bar.vertical : false
  readonly property bool showBarPercent: !vertical && (alwaysShowPercentage || !b.acOnline)

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  function togglePercentage() {
    root.settings = Object.assign({}, root.settings, { alwaysShowPercentage: !root.alwaysShowPercentage })
    if (bar && bar.shell && bar.shell.updateEntryInline) bar.shell.updateEntryInline(moduleName, root.settings)
  }

  function focusEnergy(which) {
    var item = planSection.focusEnergy(which)
    if (!item) return
    var point = item.mapToItem(contentColumn, 0, 0)
    if (point.y < flick.contentY) flick.contentY = point.y
    else if (point.y + item.height > flick.contentY + flick.height)
      flick.contentY = point.y + item.height - flick.height
  }

  onOpenedChanged: {
    service.live = opened
    if (opened) {
      service.refresh()
      planSection.reload()
      flick.contentY = 0
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
  }


  readonly property string planOpsBin: String(Qt.resolvedUrl("bin/ops.py")).replace(/^file:\/\//, "")
  Process { id: planOps }

  function loadHistory() {
    if (histProc.running) return
    histProc.command = ["python3", root.planOpsBin, "battery-history"]
    histProc.running = true
  }

  Process {
    id: histProc
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        try { root.historyPts = JSON.parse(String(text || "[]")) }
        catch (e) { root.historyPts = [] }
      }
    }
  }
  Connections {
    target: service
    function onUpdated() {
      if (root.historyOpen) root.loadHistory()
      if (root.opened) planSection.loadStatus()
  }
    }

  PowerService {
    id: service
    binDir: String(Qt.resolvedUrl("bin")).replace(/^file:\/\//, "")
    pollInterval: Math.max(2000, Math.min(60000, (Number(root.setting("pollIntervalSec", 5)) || 5) * 1000))
  }

  IpcHandler {
    target: root.ipcTarget
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.toggle() }
    function refresh(): string { service.refresh(); return "ok" }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.showBarPercent
      ? (Model.batteryIcon(root.b.percent, root.b.status) + "  " + root.b.percent + "%")
      : Model.batteryIcon(root.b.percent, root.b.status)
    slotSize: Style.bar.iconSlot * (root.showBarPercent ? 2.3 : 1)
    tooltipText: root.b.present
      ? ("Battery " + root.b.percent + "%"
         + (root.b.status === "Discharging" ? "  ·  " + Model.fmtWatts(root.b.watts) : "")
         + (root.b.status === "Discharging" ? "  ·  " + Model.fmtDuration(root.b.minutes) + " left" : ""))
      : "Plugged in"
    onPressed: function(btn) {
      if (btn === Qt.RightButton) root.togglePercentage()
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(460), Style.space(500))
    contentHeight: Math.max(Style.space(160), Math.round(Math.min(
      availableCardHeight,
      Style.space(720),
      contentColumn.implicitHeight + verticalContentInset)))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onMoveRequested: function(dx, dy) {
        if (dy) flick.contentY = Math.max(0, Math.min(
          Math.max(0, flick.contentHeight - flick.height),
          flick.contentY + dy * Style.space(48)))
      }
      onTextKey: function(t) {
        if (t === "r" || t === "R") service.refresh()
        else if (t === "t" || t === "T") root.focusEnergy("travel")
        else if (t === "d" || t === "D") root.focusEnergy("quickDim")
        else if (t === "c" || t === "C") root.focusEnergy("cpu")
      }
      onActivateRequested: planSection.activateEnergyFocus()

      Flickable {
        id: flick
        anchors.fill: parent
        contentWidth: width
        contentHeight: contentColumn.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        // Sits in the card's right padding so it never covers content.
        ScrollBar.vertical: ScrollBar {
          id: vbar
          parent: flick.parent
          anchors.top: flick.top
          anchors.bottom: flick.bottom
          anchors.left: flick.right
          anchors.leftMargin: Math.max(0, Math.round((panel.padding - width) / 2))
          width: Style.space(4)
          padding: 0
          policy: ScrollBar.AsNeeded
          background: Item {}
          contentItem: Rectangle {
            implicitWidth: vbar.width
            radius: Style.cornerRadius > 0 ? width / 2 : 0
            color: root.fg
            opacity: vbar.pressed ? 0.6 : vbar.hovered ? 0.45 : vbar.active ? 0.3 : 0.15
            Behavior on opacity { NumberAnimation { duration: 120 } }
          }
        }

        Column {
          id: contentColumn
          width: flick.width
          spacing: Style.space(14)

          // ---------- Hero ----------
          PanelHero {
            width: parent.width
            title: "Battery"
            meta: Model.modeLabel(root.b)
            foreground: root.fg
            fontFamily: root.fontFamily
            iconComponent: Component {
              Text {
                text: Model.batteryIcon(root.b.percent, root.b.status)
                color: root.charging ? root.fg : (root.b.percent <= 10 ? root.urgent : root.fg)
                font.family: root.fontFamily
                font.pixelSize: Style.font.display
                Behavior on color { ColorAnimation { duration: 200 } }
              }
            }
            trailingControl: Component {
              Text {
                text: (root.b.percent || 0) + "%"
                color: root.fg
                font.family: root.fontFamily
                font.pixelSize: Style.font.displayLarge
                font.bold: true
              }
            }
          }

          // ---------- Charge bar ----------
          Item {
            width: parent.width
            implicitHeight: Style.space(8)

            Rectangle {
              anchors.fill: parent
              radius: height / 2
              color: root.faint
            }
            Rectangle {
              id: chargeFill
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              height: parent.height
              radius: height / 2
              color: root.b.percent <= 10 && !root.charging ? root.urgent : root.fg
              width: Math.max(height, parent.width * Math.max(0, Math.min(1, root.b.fraction)))

              Behavior on width { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }
              Behavior on color { ColorAnimation { duration: 220 } }

              SequentialAnimation on opacity {
                running: root.charging && root.opened
                loops: Animation.Infinite
                alwaysRunToEnd: true
                NumberAnimation { from: 1.0; to: 0.5; duration: 950; easing.type: Easing.InOutSine }
                NumberAnimation { from: 0.5; to: 1.0; duration: 950; easing.type: Easing.InOutSine }
              }
            }
          }

          // ---------- Wi-Fi suspend-fix banner (only when actually seen) ----------
          BorderSurface {
            visible: Model.showWifiFixBanner(root.wifiFix)
            width: parent.width
            implicitHeight: wifiFixCol.implicitHeight + Style.space(24)
            color: Style.normalFillFor(root.fg, root.urgent)
            borderSpec: Border.controlSpec("normal", root.fg, root.urgent)
            radius: Style.cornerRadius

            Column {
              id: wifiFixCol
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              anchors.margins: Style.space(12)
              spacing: Style.space(8)

              Text {
                width: parent.width
                text: Model.WIFI_FIX_INFO.title
                color: root.fg
                font.family: root.fontFamily
                font.pixelSize: Style.font.subtitle
                font.bold: true
              }
              Text {
                width: parent.width
                text: Model.wifiFixBlurb(root.wifiFix) + " " + Model.WIFI_FIX_INFO.help
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.WordWrap
              }
              AccentButton {
                text: "Install Wi-Fi suspend fix"
                bordered: true
                focusable: true
                foreground: root.fg
                fontFamily: root.fontFamily
                onClicked: { service.openTerminal("install-wifi-fix"); root.close() }
              }
            }
          }

          // ---------- Stats grid ----------
          Grid {
            id: statsGrid
            width: parent.width
            columns: 2
            columnSpacing: Style.space(16)
            rowSpacing: Style.spacing.sm

            StatPair {
              label: root.b.status === "Charging" ? "To full" : "Time left"
              value: root.b.status === "Discharging" || root.b.status === "Charging"
                ? Model.fmtDuration(root.b.minutes) : "—"
            }
            StatPair {
              label: "Power draw"
              value: Model.drawValue(root.b)
            }
            StatPair { label: "Health"; value: (root.b.health || 0) + "%" }
            StatPair { label: "Cycles"; value: String(root.b.cycles || "—") }
            StatPair {
              label: "Capacity"
              value: Number(root.b.whFull).toFixed(1) + "/" + Number(root.b.whDesign).toFixed(1) + " Wh"
            }
          }

          Column {
            width: parent.width
            spacing: Style.space(6)

            Item {
              width: parent.width
              implicitHeight: usageTitle.implicitHeight
              PanelSectionHeader {
                id: usageTitle
                text: "BATTERY USAGE"
                foreground: root.fg
                fontFamily: root.fontFamily
              }
              Text {
                anchors.right: usageChev.left
                anchors.rightMargin: Style.space(6)
                anchors.verticalCenter: usageTitle.verticalCenter
                text: service.rapl.available ? Model.raplLabel(service.rapl) : ""
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                font.bold: true
              }
              Text {
                id: usageChev
                anchors.right: parent.right
                anchors.verticalCenter: usageTitle.verticalCenter
                text: root.historyOpen ? "▾" : "▸"
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
              MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                  root.historyOpen = !root.historyOpen
                  if (root.historyOpen) root.loadHistory()
                }
              }
            }

            BorderSurface {
              visible: root.historyOpen
              width: parent.width
              height: Style.space(52)
              color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.03)
              borderSpec: Border.flat(root.faint, Style.normalBorderWidth)
              radius: Style.cornerRadius

              Canvas {
                id: usageBars
                anchors.fill: parent
                anchors.margins: Style.space(4)
                property var pts: root.historyPts
                property var bars: []
                property color fill: root.fg
                property color charge: Commons.Color.accent
                function rebuild() {
                  bars = Model.historyBars(pts, Date.now())
                  requestPaint()
                }
                onPtsChanged: rebuild()
                onFillChanged: requestPaint()
                onChargeChanged: requestPaint()
                Component.onCompleted: rebuild()
                property color grid: root.faint
                onGridChanged: requestPaint()
                onPaint: {
                  var ctx = getContext("2d")
                  ctx.reset()
                  var n = bars.length
                  if (n < 1) return
                  var w = width, h = height
                  ctx.strokeStyle = grid
                  ctx.lineWidth = 1
                  ctx.beginPath()
                  ctx.moveTo(0, 0.5)
                  ctx.lineTo(w, 0.5)
                  ctx.moveTo(0, Math.round(h / 2) + 0.5)
                  ctx.lineTo(w, Math.round(h / 2) + 0.5)
                  ctx.stroke()
                  var gap = 1
                  var bw = Math.max(1, (w - gap * (n - 1)) / n)
                  for (var i = 0; i < n; i++) {
                    var pct = Number(bars[i].pct)
                    if (pct < 0) continue
                    var bh = Math.max(1, (h - 2) * pct / 100)
                    var c = bars[i].charging ? charge : fill
                    ctx.fillStyle = bars[i].charging
                      ? Qt.rgba(c.r, c.g, c.b, 0.9)
                      : Qt.rgba(c.r, c.g, c.b, 0.35)
                    ctx.fillRect(i * (bw + gap), h - bh, bw, bh)
                  }
                }
              }
            }
            Item {
              visible: root.historyOpen
              width: parent.width
              implicitHeight: agoCap.implicitHeight
              Text {
                id: agoCap
                anchors.left: parent.left
                text: "24h ago"
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
              Text {
                anchors.right: parent.right
                text: "Now"
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
            }
          }

          PlanSection {
            id: planSection
            width: parent.width
            opened: root.opened
            pollMs: service.pollInterval
            fg: root.fg
            dim: root.dim
            fontFamily: root.fontFamily
            onEnergyChanged: service.refresh()
          }

          // ---------- Sleep-drain tracking ----------
          Column {
            width: parent.width
            spacing: Style.space(8)

            PanelSeparator { foreground: root.fg }
            PanelSectionHeader { text: "SLEEP"; foreground: root.fg; fontFamily: root.fontFamily }

            // Not installed yet — slim opt-in row, no TLP dependency.
            Item {
              visible: !root.sleepData.installed
              width: parent.width
              implicitHeight: Math.max(sleepOptInText.implicitHeight, sleepOptInBtn.implicitHeight)

              Text {
                id: sleepOptInText
                anchors.left: parent.left
                anchors.right: sleepOptInBtn.left
                anchors.rightMargin: Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                text: "Not tracked yet — measure real watts and %/hour drawn during sleep."
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.WordWrap
              }
              AccentButton {
                id: sleepOptInBtn
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: "Track sleep drain"
                bordered: true
                focusable: true
                foreground: root.fg
                fontFamily: root.fontFamily
                fontSize: Style.font.bodySmall
                onClicked: { service.openTerminal("install-sleep-tracking"); root.close() }
              }
            }

            // Installed, but no sleep cycle recorded yet.
            Text {
              visible: root.sleepData.installed && !Model.sleepSummary(root.sleepData)
              width: parent.width
              text: "Installed — data appears after the next real sleep cycle."
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.WordWrap
            }

            // Installed and measured.
            Grid {
              width: parent.width
              visible: root.sleepData.installed && !!Model.sleepSummary(root.sleepData)
              columns: 2
              columnSpacing: Style.space(16)
              rowSpacing: Style.spacing.sm
              readonly property var sum: Model.sleepSummary(root.sleepData) || ({})

              StatPair { label: "Last sleep"; value: parent.sum.duration || "—" }
              StatPair { label: "When"; value: parent.sum.ago || "—" }
              StatPair { label: "Last draw"; value: (parent.sum.lastWatts || "—") + "  ·  " + (parent.sum.lastRate || "") }
              StatPair {
                label: "Avg · " + (parent.sum.samples || 0) + " sleeps"
                value: (parent.sum.avgWatts || "—") + "  ·  " + (parent.sum.avgRate || "")
              }
            }
          }

          // ---------- Power tweaks ----------
          Column {
            visible: true
            width: parent.width
            spacing: Style.space(10)

            PanelSeparator { foreground: root.fg }
            PanelSectionHeader { text: "POWER TWEAKS"; foreground: root.fg; fontFamily: root.fontFamily }

            Repeater {
              model: Model.TWEAK_INFO

              TweakRow {
                required property var modelData
                width: parent.width
                tweakKey: modelData.key
                title: modelData.title
                blurb: modelData.blurb
                help: modelData.help
                on: root.tweaks[modelData.key] === true
                pinned: root.tweaks[modelData.key + "Pinned"] === true

                onSetOn: function(next) {
                  if (pinned) service.act("persist", tweakKey, next ? "on" : "off")
                  else service.act(tweakKey, next ? "on" : "off")
                }
                onSetPinned: function(next) {
                  if (next) service.act("persist", tweakKey, on ? "on" : "off")
                  else service.act("unpersist", tweakKey)
                }
              }
            }

          }

          // ---------- Tuning (needs TLP) ----------
          BorderSurface {
            visible: false
            width: parent.width
            implicitHeight: installCol.implicitHeight + Style.space(24)
            color: Style.normalFillFor(root.fg, Commons.Color.accent)
            borderSpec: Border.controlSpec("normal", root.fg, Commons.Color.accent)
            radius: Style.cornerRadius

            Column {
              id: installCol
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              anchors.margins: Style.space(12)
              spacing: Style.space(8)

              Text {
                width: parent.width
                text: "Power tuning isn’t set up yet"
                color: root.fg
                font.family: root.fontFamily
                font.pixelSize: Style.font.subtitle
                font.bold: true
              }
              Text {
                width: parent.width
                text: "Install TLP + the MacBook tunables to control power modes, Wi-Fi and PCIe power saving from here."
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.WordWrap
              }
              AccentButton {
                text: "Install power tuning"
                bordered: true
                focusable: true
                foreground: root.fg
                fontFamily: root.fontFamily
                onClicked: { service.openTerminal("install"); root.close() }
              }
            }
          }

          Column {
            visible: false
            width: parent.width
            spacing: Style.space(10)

            PanelSeparator { foreground: root.fg }
            PanelSectionHeader { text: "POWER MODE"; foreground: root.fg; fontFamily: root.fontFamily }

            Segmented {
              width: parent.width
              options: [
                { value: "auto",  label: "Auto" },
                { value: "saver", label: "Saver" },
                { value: "full",  label: "Full power" }
              ]
              value: Model.activeMode(root.tlp)
              onPicked: function(v) { service.act("mode", v) }
            }
            Text {
              width: parent.width
              text: root.tlp.active
                ? "Auto follows the AC adapter · currently on " + (root.tlp.mode === "battery" ? "battery profile" : "AC profile")
                : "tlp.service is not running"
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }

          // ---------- Footer ----------
          Text {
            width: parent.width
            text: "R refresh · T travel · D dim · C CPU option · J/K scroll · Esc close"
            wrapMode: Text.WordWrap
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            horizontalAlignment: Text.AlignHCenter
          }
        }
      }
    }
  }

  // ---- inline helpers -------------------------------------------------

  component StatPair: Column {
    id: sp
    property string label: ""
    property string value: ""
    // Sized for a 2-up grid; works in any Grid parent that sets columnSpacing.
    width: parent ? (parent.width - (parent.columnSpacing || 0)) / 2 : implicitWidth
    spacing: Style.space(2)

    Text {
      width: parent.width
      text: sp.label
      color: root.dim
      elide: Text.ElideRight
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
    Text {
      width: parent.width
      text: sp.value
      color: root.fg
      elide: Text.ElideRight
      font.family: root.fontFamily
      font.pixelSize: Style.font.body
      font.bold: true
    }
  }

  component Segmented: Row {
    id: seg
    property var options: []
    property string value: ""
    signal picked(string value)

    width: parent ? parent.width : implicitWidth
    spacing: Style.space(6)
    readonly property real cell: options.length > 0
      ? (width - spacing * (options.length - 1)) / options.length : 0

    Repeater {
      model: seg.options
      AccentButton {
        required property var modelData
        width: seg.cell
        text: modelData.label
        fontSize: Style.font.bodySmall
        foreground: root.fg
        fontFamily: root.fontFamily
        horizontalPadding: Style.spacing.sm
        bordered: true
        active: seg.value === String(modelData.value)
        onClicked: seg.picked(String(modelData.value))
      }
    }
  }

  // A device-power-saving row: label + one-line blurb, an (i) that expands
  // the full explanation and a "keep after reboot" control, and the switch.
  component TweakRow: Column {
    id: tw

    property string tweakKey: ""
    property string title: ""
    property string blurb: ""
    property string help: ""
    property bool on: false
    property bool pinned: false
    property bool expanded: false

    signal setOn(bool next)
    signal setPinned(bool next)

    width: parent ? parent.width : implicitWidth
    spacing: Style.space(4)

    Item {
      width: parent.width
      implicitHeight: Math.max(twSwitch.implicitHeight, twText.implicitHeight, Style.space(28))

      Column {
        id: twText
        anchors.left: parent.left
        anchors.right: twCtl.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(1)

        Text {
          width: parent.width
          text: tw.title
          color: root.fg
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
          font.bold: true
          elide: Text.ElideRight
        }
        Text {
          width: parent.width
          text: tw.blurb
          visible: text !== ""
          color: root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          wrapMode: Text.WordWrap
        }
      }

      Row {
        id: twCtl
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(6)

        PanelActionButton {
          iconText: tw.expanded ? "\uf056" : "\uf05a"
          tooltipText: tw.expanded ? "Hide details" : tw.help
          foreground: tw.expanded ? Commons.Color.accent : root.dim
          hoverColor: root.fg
          fontFamily: root.fontFamily
          anchors.verticalCenter: parent.verticalCenter
          onClicked: tw.expanded = !tw.expanded
        }
        AccentSwitch {
          id: twSwitch
          checked: tw.on
          foreground: root.fg
          accent: Commons.Color.accent
          anchors.verticalCenter: parent.verticalCenter
          onToggled: tw.setOn(!tw.on)
        }
      }

      MouseArea {
        anchors.left: parent.left
        anchors.right: twCtl.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        cursorShape: Qt.PointingHandCursor
        onClicked: tw.setOn(!tw.on)
      }
    }

    Column {
      visible: tw.expanded
      width: parent.width
      leftPadding: Style.space(2)
      bottomPadding: Style.space(2)
      spacing: Style.space(6)

      Text {
        width: parent.width - parent.leftPadding
        text: tw.help
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
        lineHeight: 1.15
      }

      AccentButton {
        text: tw.pinned ? "\uf00c  Kept after reboot" : "Keep after reboot"
        fontSize: Style.font.caption
        bordered: true
        active: tw.pinned
        foreground: root.fg
        fontFamily: root.fontFamily
        horizontalPadding: Style.spacing.sm
        verticalPadding: Style.space(3)
        onClicked: tw.setPinned(!tw.pinned)
      }
    }
  }
}
