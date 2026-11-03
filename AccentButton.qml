import QtQuick
import qs.Commons
import qs.Ui

// Ui.Button with the theme's resting fill and an accent outline on hover and when on.
Button {
  bordered: true
  background: Style.normalFillFor(foreground, accent)
  borderSpec: _showFocusRing ? _focusBorderSpec
    : enabled && (hot || selected || active) ? Border.flat(accent, Math.max(1, Style.normalBorderWidth))
    : _normalBorderSpec
  // Disabled greys out the whole button, a selected fill included.
  opacity: enabled ? 1 : 0.35
}
