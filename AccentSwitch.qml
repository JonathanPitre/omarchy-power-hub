import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// On/off switch styled like AccentButton: theme resting fill, accent outline on hover and when on.
BorderSurface {
  id: root

  property bool checked: false
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  signal toggled()

  readonly property bool hot: mouse.containsMouse

  implicitHeight: Math.max(22, Math.round(Style.spacing.controlHeight * 0.55))
  implicitWidth: Math.round(implicitHeight * 1.9)
  radius: Style.cornerRadius > 0 ? height / 2 : 0
  opacity: enabled ? 1 : 0.35
  activeFocusOnTab: enabled
  color: checked ? Style.selectedFillFor(foreground, accent)
    : hot ? Style.hoverFillFor(foreground, accent)
    : Style.normalFillFor(foreground, accent)
  borderSpec: activeFocus ? Border.controlSpec("focus", foreground, accent)
    : enabled && (checked || hot) ? Border.flat(accent, Math.max(1, Style.normalBorderWidth))
    : Border.controlSpec("normal", foreground, accent)
  Behavior on color { ColorAnimation { duration: 120 } }

  Accessible.role: Accessible.CheckBox
  Accessible.checkable: true
  Accessible.checked: checked
  Accessible.onPressAction: toggled()
  Keys.onSpacePressed: toggled()

  Rectangle {
    readonly property int inset: Math.max(1, Math.round((root.height - height) / 2))
    width: Math.round(root.height * 0.64)
    height: width
    radius: root.radius > 0 ? height / 2 : 0
    x: root.checked ? root.width - width - inset : inset
    anchors.verticalCenter: parent.verticalCenter
    color: root.checked ? Style.selectedStateColor(root.foreground, root.accent) : Qt.darker(root.foreground, 1.25)
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
