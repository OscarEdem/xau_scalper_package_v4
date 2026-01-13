import math
from PySide6.QtWidgets import (QWidget, QPushButton, QCheckBox, QHBoxLayout, QVBoxLayout, # type: ignore
                               QSpinBox, QDoubleSpinBox, QAbstractSpinBox, QLabel, QFrame, QSizePolicy)
from PySide6.QtCore import (Qt, QTimer, Property, QPropertyAnimation, QEasingCurve,  # type: ignore
                            Signal, QRectF, QPointF, QSize)
from PySide6.QtGui import QColor, QPainter, QBrush, QPen, QPainterPath, QPolygonF, QConicalGradient, QFont, QFontMetrics # type: ignore

class SignalIndicator(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(10, 10)
        self.color = QColor("#444746") # Inactive gray

    def set_active(self, active):
        new_color = QColor("#A8C7FA") if active else QColor("#444746")
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
    def __init__(self, size=10, color="#444746", parent=None):
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

class ATRGauge(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(140, 80)
        self._value = 0.0
        self.threshold = 1.0
        self.timeframe = "M5"
        
        self.anim = QPropertyAnimation(self, b"value", self)
        self.anim.setDuration(600)
        self.anim.setEasingCurve(QEasingCurve.OutCubic)
        self.anim.setEndValue(0.0)

    def get_value(self):
        return self._value

    def set_gauge_value(self, val):
        self._value = val
        self.update()

    value = Property(float, get_value, set_gauge_value)

    def set_value(self, value, threshold, timeframe="M5"):
        self.threshold = threshold
        self.timeframe = timeframe
        
        # Prevent restarting animation if value hasn't changed significantly
        if abs(value - self.anim.endValue()) < 0.001:
            return
        
        if self.anim.state() == QPropertyAnimation.Running:
            self.anim.stop()
        self.anim.setStartValue(self._value)
        self.anim.setEndValue(value)
        self.anim.start()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        
        w = self.width()
        h = self.height()
        cx = w / 2
        cy = h - 30
        radius = min(w/2, h-30) - 5
        
        # Track (Background)
        path_track = QPainterPath()
        rect = QRectF(cx - radius, cy - radius, radius * 2, radius * 2)
        path_track.arcMoveTo(rect, 180)
        path_track.arcTo(rect, 180, -180)
        
        pen_track = QPen(QColor("#1E1F20"), 6, Qt.SolidLine, Qt.FlatCap)
        painter.setPen(pen_track)
        painter.drawPath(path_track)
        
        # Colored Segments
        max_val = self.threshold * 2.0 if self.threshold > 0 else 1.0
        if max_val < self._value: max_val = self._value * 1.1
        if max_val == 0: max_val = 1.0
        
        safe_ratio = min(1.0, self.threshold / max_val)
        
        # Dynamic Gradient (Blue -> Green -> Yellow -> Red)
        grad = QConicalGradient(cx, cy, 0)
        stop_yellow = 0.5 - (safe_ratio * 0.5)
        stop_green = 0.5 - (safe_ratio * 0.5 * 0.35) # 35% of safe zone is blue
        
        grad.setColorAt(0.5, QColor("#669DF6")) # Blue (Quiet)
        grad.setColorAt(stop_green, QColor("#81C995")) # Green (Normal)
        grad.setColorAt(stop_yellow, QColor("#FDD663")) # Yellow (Threshold)
        
        if safe_ratio < 1.0:
            grad.setColorAt(0.0, QColor("#F28B82"))
            
        pen_grad = QPen(QBrush(grad), 6, Qt.SolidLine, Qt.FlatCap)
        painter.setPen(pen_grad)
        painter.drawPath(path_track)
        
        # Needle
        val_ratio = min(1.0, self._value / max_val)
        angle_deg = 180 - (val_ratio * 180)
        angle_rad = math.radians(angle_deg)
        
        needle_len = radius - 2
        nx = cx + needle_len * math.cos(angle_rad)
        ny = cy - needle_len * math.sin(angle_rad)
        
        # Determine Status & Color
        status_text = "Normal"
        status_color = QColor("#81C995")
        
        if self._value <= self.threshold * 0.35:
            status_text = "Quiet"
            status_color = QColor("#669DF6")
        elif self._value >= self.threshold:
            status_text = "High"
            status_color = QColor("#F28B82")
        else:
            status_color = QColor("#81C995") # Normal
            
        painter.setPen(Qt.NoPen)
        painter.setBrush(status_color)
        
        perp = angle_rad + math.pi/2
        base_w = 3
        p1 = QPointF(cx + base_w * math.cos(perp), cy - base_w * math.sin(perp))
        p2 = QPointF(cx - base_w * math.cos(perp), cy + base_w * math.sin(perp))
        p3 = QPointF(nx, ny)
        
        painter.drawPolygon(QPolygonF([p1, p2, p3]))
        
        # Pivot
        painter.drawEllipse(QPointF(cx, cy), 4, 4)
        
        # Text Value
        painter.setPen(QColor("#E3E3E3"))
        f = painter.font()
        f.setPixelSize(12)
        f.setBold(True)
        painter.setFont(f)
        
        # Dynamic precision for small values
        if self._value < 1.0 and self._value > 0.0001:
            txt = f"ATR ({self.timeframe}): {self._value:.4f}"
        else:
            txt = f"ATR ({self.timeframe}): {self._value:.2f}"
            
        painter.drawText(QRectF(0, h - 28, w, 14), Qt.AlignCenter, txt)
        
        f.setPixelSize(10)
        f.setBold(False)
        painter.setFont(f)
        painter.setPen(status_color)
        painter.drawText(QRectF(0, h - 14, w, 14), Qt.AlignCenter, status_text)

class SafetyButton(QPushButton):
    """
    A button that requires holding for 1 second to trigger.
    Provides visual progress feedback.
    """
    triggered = Signal()

    def __init__(self, text, parent=None, color_base="#F28B82", color_fill="#E57373"):
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
        painter.setPen(QColor("#000000"))
        font = self.font()
        font.setBold(True)
        painter.setFont(font)
        painter.drawText(rect, Qt.AlignCenter, self.text())

class ToggleSwitch(QCheckBox):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(30, 20)
        self.setCursor(Qt.PointingHandCursor)
        self._circle_position = 2
        self._bg_color = QColor("#444746")
        self._circle_color = QColor("#F1F1F1")
        self._active_color = QColor("#A8C7FA")
        
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
            self.animation.setEndValue(self.width() - 18)
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
        p.drawEllipse(int(self._circle_position), 2, 16, 16)

class ExposurePanel(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumWidth(80)
        self.exposure = 0.0
        self.max_exposure = 10.0 # Dynamic scale
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(8, 2, 8, 2)
        layout.setSpacing(0)
        
        self.lbl_title = QLabel("NET EXPOSURE")
        self.lbl_title.setStyleSheet("color: #A8C7FA; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        self.lbl_title.setAlignment(Qt.AlignCenter)
        
        self.lbl_val = QLabel("0.00")
        self.lbl_val.setStyleSheet("color: #F1F1F1; font-size: 11pt; font-weight: bold; font-family: Consolas; background: transparent;")
        self.lbl_val.setAlignment(Qt.AlignCenter)
        
        layout.addWidget(self.lbl_title)
        layout.addWidget(self.lbl_val)
        
        self.setStyleSheet("background-color: #1E1F20; border-radius: 4px; border: 1px solid #444746;")

    def set_exposure(self, value):
        self.exposure = value
        if abs(value) > self.max_exposure:
            self.max_exposure = abs(value) * 1.2
        elif self.max_exposure > 10.0 and abs(value) < self.max_exposure * 0.5:
             self.max_exposure = max(10.0, abs(value) * 1.5)
             
        self.lbl_val.setText(f"{value:.2f}")
        
        if self.max_exposure > 0:
            pct = (abs(self.exposure) / self.max_exposure) * 100.0
            self.setToolTip(f"Used: {pct:.1f}% of Max ({self.max_exposure:.2f})")
            
        self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        
        w = self.width()
        h = self.height()
        
        # Draw Background manually since paintEvent overrides stylesheet
        painter.setBrush(QColor("#1E1F20"))
        painter.setPen(QPen(QColor("#444746"), 1))
        painter.drawRoundedRect(0, 0, w-1, h-1, 4, 4)
        
        if self.max_exposure > 0:
            ratio = abs(self.exposure) / self.max_exposure
            ratio = min(1.0, ratio)
            
            bar_w = (w - 2) * ratio
            
            if self.exposure > 0:
                color = QColor("#81C995")
            else:
                color = QColor("#F28B82")
            
            color.setAlpha(60)
            rect = QRectF(1, 2, bar_w, h-4)
            
            painter.setBrush(QBrush(color))
            painter.setPen(Qt.NoPen)
            
            path = QPainterPath()
            path.addRoundedRect(0, 0, w-1, h-1, 4, 4)
            painter.setClipPath(path)
            
            painter.drawRect(rect)

class MarginPanel(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumWidth(80)
        self.margin = 0.0
        self.equity = 1.0
        self.flash_state = False
        
        self.timer = QTimer(self)
        self.timer.setInterval(500)
        self.timer.timeout.connect(self.toggle_flash)
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(8, 2, 8, 2)
        layout.setSpacing(0)
        
        self.lbl_title = QLabel("MARGIN USED")
        self.lbl_title.setStyleSheet("color: #FDD663; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        self.lbl_title.setAlignment(Qt.AlignCenter)
        
        self.lbl_val = QLabel("$0.00")
        self.lbl_val.setStyleSheet("color: #F1F1F1; font-size: 11pt; font-weight: bold; font-family: Consolas; background: transparent;")
        self.lbl_val.setAlignment(Qt.AlignCenter)
        
        layout.addWidget(self.lbl_title)
        layout.addWidget(self.lbl_val)
        
        self.setStyleSheet("background-color: #1E1F20; border-radius: 4px; border: 1px solid #444746;")

    def set_data(self, margin, equity):
        self.margin = margin
        self.equity = equity if equity > 0 else 1.0
        self.lbl_val.setText(f"${margin:.2f}")
        
        usage_pct = (margin / self.equity) * 100.0
        
        if usage_pct > 80.0:
            if not self.timer.isActive():
                self.timer.start()
        else:
            if self.timer.isActive():
                self.timer.stop()
                self.flash_state = False
                self.lbl_title.setStyleSheet("color: #FDD663; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        
        self.update()

    def toggle_flash(self):
        self.flash_state = not self.flash_state
        if self.flash_state:
             self.lbl_title.setStyleSheet("color: #F1F1F1; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        else:
             self.lbl_title.setStyleSheet("color: #FDD663; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        
        w = self.width()
        h = self.height()
        
        # Background
        if self.flash_state:
            bg_color = QColor("#F28B82")
            border_color = QColor("#F28B82")
        else:
            bg_color = QColor("#1E1F20")
            border_color = QColor("#444746")

        painter.setBrush(bg_color)
        painter.setPen(QPen(border_color, 1))
        painter.drawRoundedRect(0, 0, w-1, h-1, 4, 4)
        
        # Usage Bar
        if not self.flash_state and self.equity > 0:
            ratio = min(1.0, self.margin / self.equity)
            if ratio > 0.001:
                bar_w = (w - 2) * ratio
                rect = QRectF(1, 2, bar_w, h-4)
                
                bar_color = QColor("#FDD663")
                if ratio > 0.5: bar_color = QColor("#F28B82")
                
                bar_color.setAlpha(60)
                painter.setBrush(QBrush(bar_color))
                painter.setPen(Qt.NoPen)
                
                path = QPainterPath()
                path.addRoundedRect(0, 0, w-1, h-1, 4, 4)
                painter.setClipPath(path)
                
                painter.drawRect(rect)

class PortfolioStats(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedHeight(40)
        
        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(10)
        layout.setAlignment(Qt.AlignCenter)
        
        self.exp_panel = ExposurePanel()
        self.mrg_panel = MarginPanel()
        
        layout.addWidget(self.exp_panel)
        layout.addWidget(self.mrg_panel)

    def update_data(self, exposure, margin, equity):
        self.exp_panel.set_exposure(exposure)
        self.mrg_panel.set_data(margin, equity)

class StrategyChips(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.modes = []
        self.setFixedHeight(20)
        self.setSizePolicy(QSizePolicy.Minimum, QSizePolicy.Fixed)
        self.flashing_modes = set()

    def set_modes(self, modes):
        if self.modes != modes:
            self.modes = modes
            self.setToolTip(", ".join(modes) if modes else "Idle")
            self.updateGeometry()
            self.update()

    def flash(self, mode_name):
        # Find matching mode (case-insensitive)
        target = None
        for m in self.modes:
            if m.lower() == mode_name.lower():
                target = m
                break
        
        if target:
            self.flashing_modes.add(target)
            self.update()
            QTimer.singleShot(600, lambda: self._stop_flash(target))

    def _stop_flash(self, mode):
        if mode in self.flashing_modes:
            self.flashing_modes.remove(mode)
            self.update()

    def sizeHint(self):
        font = self.font()
        font.setPixelSize(9)
        font.setBold(True)
        fm = QFontMetrics(font)
        
        width = 0
        if not self.modes:
            width = fm.horizontalAdvance("IDLE") + 6
        else:
            for mode in self.modes:
                text = mode.upper()
                width += fm.horizontalAdvance(text) + 6 + 2
            width -= 2
            
        return QSize(max(int(width), 10), 20)

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        
        x = 0
        y = 2
        h = 16
        
        font = painter.font()
        font.setPixelSize(9)
        font.setBold(True)
        painter.setFont(font)
        
        if not self.modes:
            text = "IDLE"
            fm = painter.fontMetrics()
            w = fm.horizontalAdvance(text) + 6
            rect = QRectF(x, y, w, h)
            painter.setBrush(QBrush(QColor("#444746")))
            painter.setPen(Qt.NoPen)
            painter.drawRoundedRect(rect, 8, 8)
            painter.setPen(QColor("#E3E3E3"))
            painter.drawText(rect, Qt.AlignCenter, text)
            return

        for mode in self.modes:
            text = mode.upper()
            if mode in self.flashing_modes:
                bg, fg = "#F1F1F1", "#000000" # Flash Bright White
            elif "SCALP" in text: bg, fg = "#81C995", "#000000"
            elif "SWING" in text: bg, fg = "#A8C7FA", "#000000"
            elif "MANUAL" in text: bg, fg = "#FDD663", "#000000"
            else: bg, fg = "#444746", "#E3E3E3"
                
            fm = painter.fontMetrics()
            w = fm.horizontalAdvance(text) + 6
            rect = QRectF(x, y, w, h)
            
            painter.setBrush(QBrush(QColor(bg)))
            painter.setPen(Qt.NoPen)
            painter.drawRoundedRect(rect, 8, 8)
            
            painter.setPen(QColor(fg))
            painter.drawText(rect, Qt.AlignCenter, text)
            x += w + 2

class SessionPanel(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumWidth(140)
        self.setFixedHeight(40)
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(8, 2, 8, 2)
        layout.setSpacing(0)
        
        self.lbl_title = QLabel("MARKET")
        self.lbl_title.setStyleSheet("color: #444746; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        self.lbl_title.setAlignment(Qt.AlignCenter)
        
        self.lbl_val = QLabel("--:--")
        self.lbl_val.setStyleSheet("color: #F1F1F1; font-size: 11pt; font-weight: bold; font-family: Consolas; background: transparent;")
        self.lbl_val.setAlignment(Qt.AlignCenter)
        
        layout.addWidget(self.lbl_title)
        layout.addWidget(self.lbl_val)
        
        self.setStyleSheet("background-color: #1E1F20; border-radius: 4px; border: 1px solid #444746;")

    def set_data(self, title, value, color):
        self.lbl_title.setText(title)
        self.lbl_title.setStyleSheet(f"color: {color}; font-size: 7pt; font-weight: bold; letter-spacing: 1px; background: transparent;")
        self.lbl_val.setText(value)

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
            
        self.input.setRange(0, 1000000)
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