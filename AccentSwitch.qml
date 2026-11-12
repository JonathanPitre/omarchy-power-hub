import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// Small pill switch: theme resting fill, filled with the accent when on.
BorderSurface {
  id: root

  property bool checked: false
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  signal toggled()

  readonly property bool hot: mouse.containsMouse

  implicitHeight: 18
  implicitWidth: 34
  radius: Style.cornerRadius > 0 ? height / 2 : 0
  opacity: enabled ? 1 : 0.35
  activeFocusOnTab: enabled
  color: checked ? accent
    : hot ? Style.hoverFillFor(foreground, accent)
    : Style.normalFillFor(foreground, accent)
  borderSpec: activeFocus ? Border.controlSpec("focus", foreground, accent)
    : enabled && hot && !checked ? Border.flat(accent, Math.max(1, Style.normalBorderWidth))
    : Border.controlSpec("normal", foreground, accent)
  Behavior on color { ColorAnimation { duration: 120 } }

  Accessible.role: Accessible.CheckBox
  Accessible.checkable: true
  Accessible.checked: checked
  Accessible.onPressAction: toggled()
  Keys.onSpacePressed: toggled()

  Rectangle {
    readonly property int inset: Math.max(1, Math.round((root.height - height) / 2))
    width: Math.round(root.height * 0.7)
    height: width
    radius: root.radius > 0 ? height / 2 : 0
    x: root.checked ? root.width - width - inset : inset
    anchors.verticalCenter: parent.verticalCenter
    color: root.checked ? Qt.darker(root.accent, 4) : Qt.darker(root.foreground, 1.25)
    Behavior on x { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
  }

  MouseArea {
    id: mouse
    anchors.fill: parent
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onClicked: root.toggled()
  }
}
