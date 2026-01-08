from PySide6.QtWidgets import (QWidget, QPushButton, QCheckBox, QHBoxLayout,  # type: ignore
                               QSpinBox, QDoubleSpinBox, QAbstractSpinBox)
from PySide6.QtCore import (Qt, QTimer, Property, QPropertyAnimation, QEasingCurve,  # type: ignore
                            Signal, QRectF)
from PySide6.QtGui import QColor, QPainter, QBrush # type: ignore

class SignalIndicator(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(10, 10)
        self.color = QColor("#4C566A") # Inactive gray

    def set_active(self, active):
        new_color = QColor("#88C0D0") if active else QColor("#4C566A")
        if self.color != new_color:
            self.color = new_color
            self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        painter.setBrush(QBrush(self.color))
        painter.setPen(Qt.NoPen)
        painter.drawEllipse(1, 1, 8, 8)

class StatusCircle(QWidget):
    def __init__(self, size=10, color="#4C566A", parent=None):
        super().__init__(parent)
        self.setFixedSize(size, size)
        self.color = QColor(color)

    def set_color(self, color_hex):
        new_color = QColor(color_hex)
        if self.color != new_color:
            self.color = new_color
            self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        painter.setBrush(QBrush(self.color))
        painter.setPen(Qt.NoPen)
        painter.drawEllipse(0, 0, self.width(), self.height())

class SafetyButton(QPushButton):
    """
    A button that requires holding for 1 second to trigger.
    Provides visual progress feedback.
    """
    triggered = Signal()

    def __init__(self, text, parent=None, color_base="#BF616A", color_fill="#D08770"):
        super().__init__(text, parent)
        self.setCursor(Qt.PointingHandCursor)
        self.color_base = QColor(color_base)
        self.color_fill = QColor(color_fill)
        self.progress = 0.0
        self.is_holding = False
        
        self.timer = QTimer(self)
        self.timer.setInterval(16) # ~60 FPS
        self.timer.timeout.connect(self.update_progress)
        
        # Override stylesheet for custom painting
        self.setStyleSheet("border: none;") 
        self.setFixedHeight(30)

    def mousePressEvent(self, e):
        if e.button() == Qt.LeftButton:
            self.is_holding = True
            self.progress = 0.0
            self.timer.start()
        super().mousePressEvent(e)

    def mouseReleaseEvent(self, e):
        self.is_holding = False
        self.timer.stop()
        if self.progress >= 1.0:
            self.triggered.emit()
        self.progress = 0.0
        self.update()
        super().mouseReleaseEvent(e)

    def update_progress(self):
        if self.is_holding:
            self.progress += 0.016 # Approx 1 second to fill
            if self.progress >= 1.0:
                self.progress = 1.0
                self.timer.stop()
            self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        
        rect = self.rect()
        
        # Draw Base
        painter.setBrush(QBrush(self.color_base))
        painter.setPen(Qt.NoPen)
        painter.drawRoundedRect(rect, 4, 4)
        
        # Draw Progress
        if self.progress > 0:
            fill_width = rect.width() * self.progress
            fill_rect = QRectF(0, 0, fill_width, rect.height())
            painter.setBrush(QBrush(self.color_fill))
            painter.drawRoundedRect(fill_rect, 4, 4) # Rounded corners might look odd if partial, but acceptable
            
        # Draw Text
        painter.setPen(QColor("#ECEFF4"))
        font = self.font()
        font.setBold(True)
        painter.setFont(font)
        painter.drawText(rect, Qt.AlignCenter, self.text())

class ToggleSwitch(QCheckBox):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(44, 24)
        self.setCursor(Qt.PointingHandCursor)
        self._circle_position = 2
        self._bg_color = QColor("#4C566A")
        self._circle_color = QColor("#ECEFF4")
        self._active_color = QColor("#88C0D0")
        
        self.animation = QPropertyAnimation(self, b"circle_position", self)
        self.animation.setDuration(200)
        self.animation.setEasingCurve(QEasingCurve.OutQuad)
        
        self.stateChanged.connect(self.start_transition)

    @Property(float)
    def circle_position(self): # type: ignore
        return self._circle_position

    @circle_position.setter
    def circle_position(self, pos):
        self._circle_position = pos
        self.update()

    def start_transition(self, state):
        self.animation.stop()
        if state:
            self.animation.setEndValue(self.width() - 22)
        else:
            self.animation.setEndValue(2)
        self.animation.start()

    def hitButton(self, pos):
        return self.rect().contains(pos)

    def paintEvent(self, event):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        
        # Draw Track
        track_color = self._active_color if self.isChecked() else self._bg_color
        p.setBrush(track_color)
        p.setPen(Qt.NoPen)
        rect = self.rect()
        p.drawRoundedRect(0, 0, rect.width(), rect.height(), rect.height() / 2, rect.height() / 2)
        
        # Draw Circle
        p.setBrush(self._circle_color)
        p.drawEllipse(int(self._circle_position), 2, 20, 20)

class ModernSpinBox(QWidget):
    def __init__(self, value, is_float=False, step=1.0, decimals=2, parent=None):
        super().__init__(parent)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(4)
        
        self.btn_minus = QPushButton("-")
        self.btn_minus.setFixedSize(28, 28)
        self.btn_minus.setProperty("class", "SpinBtn")
        self.btn_minus.setCursor(Qt.PointingHandCursor)
        self.btn_minus.setFocusPolicy(Qt.NoFocus)
        
        if is_float:
            self.input = QDoubleSpinBox()
            self.input.setDecimals(decimals)
            self.input.setSingleStep(step)
        else:
            self.input = QSpinBox()
            self.input.setSingleStep(int(step))
            
        self.input.setRange(0, 9999)
        self.input.setValue(value)
        self.input.setButtonSymbols(QAbstractSpinBox.NoButtons)
        self.input.setAlignment(Qt.AlignCenter)
        self.input.setFixedHeight(28)
        
        self.btn_plus = QPushButton("+")
        self.btn_plus.setFixedSize(28, 28)
        self.btn_plus.setProperty("class", "SpinBtn")
        self.btn_plus.setCursor(Qt.PointingHandCursor)
        self.btn_plus.setFocusPolicy(Qt.NoFocus)

        self.btn_minus.clicked.connect(self.input.stepDown)
        self.btn_plus.clicked.connect(self.input.stepUp)
        
        layout.addWidget(self.btn_minus)
        layout.addWidget(self.input)
        layout.addWidget(self.btn_plus)
        
    @property
    def valueChanged(self):
        return self.input.valueChanged
        
    def value(self):
        return self.input.value()
        
    def setValue(self, val):
        self.input.setValue(val)