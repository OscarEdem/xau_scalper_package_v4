import sys
import os
import json
import csv
import shutil
from typing import Dict, List, Any, Optional, Set

from PySide6.QtWidgets import (QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,  # type: ignore
                               QLabel, QLineEdit, QSpinBox, QDoubleSpinBox, QCheckBox, 
                               QPushButton, QComboBox, QTabWidget, QTreeWidget, QTreeWidgetItem, 
                               QFrame, QMessageBox, QGridLayout, QHeaderView, QSizePolicy, QAbstractSpinBox,
                               QDialog, QScrollArea, QDialogButtonBox, QFormLayout, QMenu, QFileDialog)
from PySide6.QtCore import (Qt, QTimer, Property, QPropertyAnimation, QEasingCurve, QByteArray,  # type: ignore
                            QThread, Signal, Slot, QObject, QRectF)
from PySide6.QtGui import QColor, QPainter, QBrush, QIcon, QPen # type: ignore

import MetaTrader5 as mt5 # type: ignore
from datetime import datetime, timedelta
import time

from config import CONFIG, DEFAULT_CONFIG, state

# =============================================================================
# STYLESHEET (NORD THEME)
# =============================================================================
GLOBAL_STYLESHEET = """
    QMainWindow { background-color: #2E3440; color: #D8DEE9; font-family: "Segoe UI", sans-serif; }
    QWidget { font-size: 10pt; color: #D8DEE9; }
    
    /* Panels & Frames */
    QFrame.Panel { background-color: #3B4252; border-radius: 6px; border: 1px solid #434C5E; }
    QTabWidget::pane { border: 1px solid #434C5E; background: #3B4252; }
    QTabBar::tab { background: #2E3440; color: #D8DEE9; padding: 8px 16px; border-top-left-radius: 4px; border-top-right-radius: 4px; margin-right: 2px; }
    QTabBar::tab:selected { background: #88C0D0; color: #2E3440; font-weight: bold; }
    
    /* Inputs */
    QLineEdit, QSpinBox, QDoubleSpinBox, QComboBox { background-color: #434C5E; border: 1px solid #4C566A; border-radius: 4px; padding: 5px; color: #ECEFF4; selection-background-color: #88C0D0; }
    QComboBox::drop-down { border: none; background: #4C566A; width: 20px; border-top-right-radius: 4px; border-bottom-right-radius: 4px; }
    
    /* Buttons */
    QPushButton { background-color: #4C566A; border: none; border-radius: 4px; padding: 8px; color: #ECEFF4; font-weight: bold; }
    QPushButton:hover { background-color: #5E81AC; }
    QPushButton:pressed { background-color: #81A1C1; }
    QPushButton.Danger { background-color: #BF616A; }
    QPushButton.Danger:hover { background-color: #D08770; }
    QPushButton.Success { background-color: #A3BE8C; color: #2E3440; }
    QPushButton.Success:hover { background-color: #B5D19E; }
    
    /* SpinBox Buttons */
    QPushButton.SpinBtn { background-color: #4C566A; border-radius: 2px; font-size: 14px; }
    QPushButton.SpinBtn:hover { background-color: #88C0D0; color: #2E3440; }
    
    /* Tree/List */
    QTreeWidget { background-color: #3B4252; border: none; font-family: "Consolas", monospace; font-size: 9pt; alternate-background-color: #434C5E; }
    QHeaderView::section { background-color: #2E3440; color: #D8DEE9; padding: 6px; border: none; font-weight: bold; }
    
    /* Scrollbars */
    QScrollBar:vertical { background: #2E3440; width: 10px; }
    QScrollBar::handle:vertical { background: #4C566A; border-radius: 5px; }
    QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0px; }
    
    /* Custom Labels */
    QLabel.Header { font-size: 14pt; font-weight: bold; color: #ECEFF4; }
    QLabel.SubHeader { font-size: 11pt; font-weight: bold; color: #88C0D0; }
    QLabel.Price { font-family: "Consolas", monospace; font-size: 20pt; font-weight: bold; color: #ECEFF4; }
    QLabel.StatusTag { padding: 4px 8px; border-radius: 4px; font-weight: bold; font-size: 9pt; }
"""

# =============================================================================
# WORKER THREAD
# =============================================================================
class MT5DataWorker(QObject):
    """
    Handles all MT5 read operations in a separate thread to prevent UI freezing.
    """
    data_updated = Signal(dict)

    def __init__(self):
        super().__init__()
        self.timer = QTimer(self)
        self.timer.setInterval(200)  # 5Hz update rate
        self.timer.timeout.connect(self.fetch_data)

    @Slot()
    def start_working(self):
        self.timer.start()

    @Slot()
    def stop_working(self):
        self.timer.stop()

    def fetch_data(self):
        if not state.get("running", False):
            return

        data: Dict[str, Any] = {}
        
        # 1. Connection & Terminal Info
        term = mt5.terminal_info()
        data["connected"] = term.connected if term else False
        data["trade_allowed"] = term.trade_allowed if term else False
        data["ping"] = term.ping_last // 1000 if term else 0
        
        # 2. Price Data
        symbol = CONFIG["trade_symbol"]
        tick = mt5.symbol_info_tick(symbol)
        if tick:
            data["bid"] = tick.bid
            data["ask"] = tick.ask
        else:
            data["bid"] = 0.0
            data["ask"] = 0.0
            
        # 3. Account Info
        acct = mt5.account_info()
        data["balance"] = acct.balance if acct else 0.0
        data["equity"] = acct.equity if acct else 0.0
        
        # 4. Positions
        positions = mt5.positions_get(symbol=symbol)
        pos_list = []
        counts = {"scalp": 0, "swing": 0, "manual": 0}
        total_open_pl = 0.0
        
        if positions:
            for p in positions:
                # Categorize
                if p.magic == CONFIG["magic_number"]: counts["scalp"] += 1
                elif p.magic == CONFIG["magic_number"] + 1: counts["swing"] += 1
                elif p.magic == 0: counts["manual"] += 1
                
                if p.magic in [CONFIG["magic_number"], CONFIG["magic_number"]+1, 0]:
                    # Calculate Net P/L (Profit + Swap)
                    net_pl = p.profit + p.swap
                    total_open_pl += net_pl
                    pos_list.append({
                        "ticket": p.ticket,
                        "type": "BUY" if p.type == mt5.ORDER_TYPE_BUY else "SELL",
                        "volume": p.volume,
                        "profit": net_pl,
                        "magic": p.magic,
                        "sl": p.sl,
                        "tp": p.tp,
                        "symbol": p.symbol
                    })
        
        data["positions"] = pos_list
        data["counts"] = counts
        data["open_pl"] = total_open_pl
        
        # 5. History (Last 24h)
        # Optimization: Only fetch if tab is likely visible or periodically? 
        # For now, fetch every cycle but lightweight.
        from_d = datetime.now() - timedelta(hours=24)
        deals = mt5.history_deals_get(from_d, datetime.now())
        hist_list = []
        total_pl_24h = 0.0
        
        if deals:
            # Sort by time descending
            sorted_deals = sorted(deals, key=lambda x: x.time, reverse=True)
            for d in sorted_deals:
                if d.symbol == symbol and d.magic in [CONFIG["magic_number"], CONFIG["magic_number"]+1, 0]:
                    if d.entry in [mt5.DEAL_ENTRY_OUT, mt5.DEAL_ENTRY_INOUT]:
                        total_pl_24h += d.profit
                        hist_list.append({
                            "ticket": d.ticket,
                            "type": "SELL" if d.type == mt5.ORDER_TYPE_BUY else "BUY",
                            "volume": d.volume,
                            "profit": d.profit,
                            "time": d.time
                        })
        
        data["history"] = hist_list
        data["total_pl_24h"] = total_pl_24h
        
        # 6. Logs (Consume from state)
        with state["lock"]:
            if state.get("gui_logs"):
                data["new_logs"] = state["gui_logs"][:]
                state["gui_logs"] = []
            else:
                data["new_logs"] = []
                
        # 7. Status Text
        data["status_text"] = state.get("status_text", "")
        data["connection_time"] = state.get("connection_time")
        data["last_signal_ts"] = state.get("last_signal_ts", 0)
        data["server_atr"] = state.get("server_atr", 0.0)

        self.data_updated.emit(data)

# =============================================================================
# CUSTOM WIDGETS
# =============================================================================
class SignalIndicator(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(14, 14)
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
        painter.drawEllipse(1, 1, 12, 12)

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

class AdvancedSettingsDialog(QDialog):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Configuration")
        self.resize(450, 600)
        # Stylesheet inherited from parent via global app style
        
        layout = QVBoxLayout(self)
        
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.NoFrame)
        scroll.setStyleSheet("background-color: transparent;")
        
        content = QWidget()
        self.form_layout = QFormLayout(content)
        self.form_layout.setSpacing(15)
        self.form_layout.setLabelAlignment(Qt.AlignLeft)
        
        self.inputs = {}
        
        self.add_section("General Settings")
        self.add_input("Signal Symbol", "signal_symbol", str, "Symbol to listen for signals.")
        self.add_input("Trade Symbol", "trade_symbol", str, "Symbol to execute trades on.")
        self.add_input("Scalp Lot Size", "fixed_lot_size", float, "Lot size for Scalp trades.")
        self.add_input("Swing Lot Size", "swing_lot_size", float, "Lot size for Swing trades.")
        self.add_input("Max Entries", "max_entries", int, "Maximum number of entries per signal.")
        self.add_input("Min Conviction (%)", "min_conviction", float, "Minimum conviction score required to execute.")
        
        self.add_section("Trailing Stop")
        self.add_input("Scalp Start (pips)", "trailing_start_pips_scalp", float, "Profit in pips required to activate trailing stop.")
        self.add_input("Scalp Dist (pips)", "trailing_dist_pips_scalp", float, "Distance in pips to maintain from current price.")
        self.add_input("Scalp Step (pips)", "trailing_step_pips_scalp", float, "Minimum price movement in pips to update stop loss.")
        self.add_input("Swing Start (pips)", "trailing_start_pips_swing", float, "Profit in pips required to activate trailing stop.")
        self.add_input("Swing Dist (pips)", "trailing_dist_pips_swing", float, "Distance in pips to maintain from current price.")
        self.add_input("Swing Step (pips)", "trailing_step_pips_swing", float, "Minimum price movement in pips to update stop loss.")
        self.add_input("Max Scalp SL (pips)", "max_scalp_sl_pips", float, "Maximum allowed Stop Loss distance for Scalp trades (Max 100).")
        
        self.add_section("ATR Trailing")
        self.add_bool("Use ATR Trailing", "use_atr_trailing", "Use Server ATR for trailing distance instead of fixed pips.")
        self.add_input("ATR Mult (Scalp)", "atr_dist_mult_scalp", float, "Multiplier for ATR to calculate trailing distance (Scalp).")
        self.add_input("ATR Mult (Swing)", "atr_dist_mult_swing", float, "Multiplier for ATR to calculate trailing distance (Swing).")
        self.add_input("High Vol Threshold", "atr_high_vol_threshold", float, "ATR value above which the indicator turns red.")
        
        self.add_section("Stagnation / Timeouts")
        self.add_bool("Use Stagnation", "use_stagnation", "Enable partial closing of trades that stall.")
        self.add_input("Stag. Sec", "stagnation_sec", int, "Seconds before a trade is considered stagnant.")
        self.add_input("Time Mult", "stag_time_mult", float, "Multiplier for stagnation time on Swing trades.")

        scroll.setWidget(content)
        layout.addWidget(scroll)
        
        btns = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        self.btn_reset = btns.addButton("Reset to Defaults", QDialogButtonBox.ResetRole)
        self.btn_reset.setCursor(Qt.PointingHandCursor)
        self.btn_reset.setToolTip("Restore all settings to their original default values.")
        self.btn_reset.clicked.connect(self.reset_defaults)
        
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        layout.addWidget(btns)

    def add_section(self, title):
        lbl = QLabel(title)
        lbl.setStyleSheet("font-weight: bold; color: #88C0D0; font-size: 11pt; margin-top: 10px;")
        self.form_layout.addRow(lbl)

    def add_input(self, label, key, dtype, tooltip=None):
        val = CONFIG.get(key, "" if dtype == str else 0)
        
        if dtype == str:
            widget = QLineEdit(str(val))
        else:
            step = 0.1 if dtype == float and ("mult" in key or "pct" in key) else 1.0
            if "lot" in key: step = 0.01
            widget = ModernSpinBox(val, is_float=(dtype == float), step=step)
            
        if tooltip:
            widget.setToolTip(tooltip)
        
        lbl = QLabel(label)
        if tooltip:
            lbl.setToolTip(tooltip)
        self.form_layout.addRow(lbl, widget)
        self.inputs[key] = (widget, dtype)

    def add_bool(self, label, key, tooltip=None):
        val = CONFIG.get(key, False)
        widget = ToggleSwitch()
        widget.setChecked(val)
        if tooltip:
            widget.setToolTip(tooltip)
        
        lbl = QLabel(label)
        if tooltip:
            lbl.setToolTip(tooltip)
        self.form_layout.addRow(lbl, widget)
        self.inputs[key] = (widget, bool)

    def reset_defaults(self):
        msg = QMessageBox(self)
        msg.setWindowTitle("Confirm Reset")
        msg.setText("Are you sure you want to reset all settings to defaults?")
        msg.setIcon(QMessageBox.Warning)
        msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
        msg.setDefaultButton(QMessageBox.No)
        
        if msg.exec() == QMessageBox.Yes:
            for key, (widget, dtype) in self.inputs.items():
                if key in DEFAULT_CONFIG:
                    val = DEFAULT_CONFIG[key]
                    if dtype == bool:
                        widget.setChecked(val)
                    elif dtype == str:
                        widget.setText(str(val))
                    else:
                        widget.setValue(val)

    def accept(self):
        msg = QMessageBox(self)
        msg.setWindowTitle("Confirm Changes")
        msg.setText("Are you sure you want to apply these advanced settings?")
        msg.setIcon(QMessageBox.Question)
        msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
        msg.setDefaultButton(QMessageBox.No)
        
        if msg.exec() == QMessageBox.Yes:
            for key, (widget, dtype) in self.inputs.items():
                if dtype == bool:
                    CONFIG[key] = widget.isChecked()
                elif dtype == str:
                    CONFIG[key] = widget.text()
                else:
                    CONFIG[key] = widget.value()
            super().accept()

class ManualExecutionDialog(QDialog):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Manual Execution")
        self.setWindowFlags(Qt.Window) # Modeless window
        self.resize(340, 520)
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        layout = QVBoxLayout(self)
        layout.setSpacing(15)
        layout.setContentsMargins(15, 15, 15, 15)
        
        # Inputs Panel
        input_frame = QFrame()
        input_frame.setProperty("class", "Panel")
        input_layout = QGridLayout(input_frame)
        input_layout.setVerticalSpacing(12)
        input_layout.setHorizontalSpacing(10)
        
        # Inputs
        input_layout.addWidget(QLabel("Price:"), 0, 0)
        
        price_container = QWidget()
        price_layout = QHBoxLayout(price_container)
        price_layout.setContentsMargins(0,0,0,0)
        price_layout.setSpacing(2)
        
        self.spin_man_price = ModernSpinBox(0, is_float=True, step=0.1)
        self.spin_man_price.input.setRange(0, 99999)
        self.spin_man_price.setToolTip("Entry Price. Required for Pending Orders. Leave 0 for Market execution.")
        
        self.btn_copy_price = QPushButton("📍")
        self.btn_copy_price.setFixedSize(24, 28)
        self.btn_copy_price.setCursor(Qt.PointingHandCursor)
        self.btn_copy_price.setToolTip("Copy current market price")
        self.btn_copy_price.clicked.connect(self.copy_current_price)
        
        price_layout.addWidget(self.spin_man_price)
        price_layout.addWidget(self.btn_copy_price)
        
        input_layout.addWidget(price_container, 0, 1)
        
        input_layout.addWidget(QLabel("Volume:"), 1, 0)
        self.spin_man_vol = ModernSpinBox(CONFIG["fixed_lot_size"], is_float=True, step=0.01)
        self.spin_man_vol.input.setRange(0.01, 100.0)
        input_layout.addWidget(self.spin_man_vol, 1, 1)
        
        input_layout.addWidget(QLabel("Count:"), 2, 0)
        self.spin_man_count = ModernSpinBox(1, is_float=False, step=1)
        self.spin_man_count.input.setRange(1, 100)
        input_layout.addWidget(self.spin_man_count, 2, 1)
        
        input_layout.addWidget(QLabel("Stop Loss:"), 3, 0)
        self.spin_man_sl = ModernSpinBox(0, is_float=True, step=1.0)
        self.spin_man_sl.input.setRange(0, 99999)
        self.spin_man_sl.setToolTip("Price for Stop Loss (0 = None)")
        input_layout.addWidget(self.spin_man_sl, 3, 1)
        
        input_layout.addWidget(QLabel("Take Profit:"), 4, 0)
        self.spin_man_tp = ModernSpinBox(0, is_float=True, step=1.0)
        self.spin_man_tp.input.setRange(0, 99999)
        self.spin_man_tp.setToolTip("Price for Take Profit (0 = None)")
        input_layout.addWidget(self.spin_man_tp, 4, 1)
        
        layout.addWidget(input_frame)
        
        # Buttons
        btn_layout = QHBoxLayout()
        btn_layout.setSpacing(10)
        
        self.btn_buy = QPushButton("BUY")
        self.btn_buy.setProperty("class", "Success")
        self.btn_buy.setFixedHeight(40)
        self.btn_buy.setStyleSheet("font-size: 12pt; font-weight: bold;")
        self.btn_buy.setCursor(Qt.PointingHandCursor)
        self.btn_buy.setToolTip("Execute a Market Buy order at current Ask price.")
        self.btn_buy.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_BUY))
        btn_layout.addWidget(self.btn_buy)
        
        self.btn_sell = QPushButton("SELL")
        self.btn_sell.setProperty("class", "Danger")
        self.btn_sell.setFixedHeight(40)
        self.btn_sell.setStyleSheet("font-size: 12pt; font-weight: bold;")
        self.btn_sell.setCursor(Qt.PointingHandCursor)
        self.btn_sell.setToolTip("Execute a Market Sell order at current Bid price.")
        self.btn_sell.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_SELL))
        btn_layout.addWidget(self.btn_sell)
        
        layout.addLayout(btn_layout)
        
        # Pending Buttons
        pending_layout = QGridLayout()
        pending_layout.setSpacing(10)
        
        self.btn_buy_limit = QPushButton("Buy Limit")
        self.btn_buy_limit.setToolTip("Place a Buy Order at a price LOWER than current market price.")
        self.btn_buy_limit.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_BUY_LIMIT))
        
        self.btn_sell_limit = QPushButton("Sell Limit")
        self.btn_sell_limit.setToolTip("Place a Sell Order at a price HIGHER than current market price.")
        self.btn_sell_limit.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_SELL_LIMIT))
        
        self.btn_buy_stop = QPushButton("Buy Stop")
        self.btn_buy_stop.setToolTip("Place a Buy Order at a price HIGHER than current market price.")
        self.btn_buy_stop.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_BUY_STOP))
        
        self.btn_sell_stop = QPushButton("Sell Stop")
        self.btn_sell_stop.setToolTip("Place a Sell Order at a price LOWER than current market price.")
        self.btn_sell_stop.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_SELL_STOP))
        
        pending_layout.addWidget(self.btn_buy_limit, 0, 0)
        pending_layout.addWidget(self.btn_sell_limit, 0, 1)
        pending_layout.addWidget(self.btn_buy_stop, 1, 0)
        pending_layout.addWidget(self.btn_sell_stop, 1, 1)
        
        layout.addLayout(pending_layout)
        
        self.load_geometry()

    def load_geometry(self):
        try:
            if os.path.exists("manual_ui_state.json"):
                with open("manual_ui_state.json", "r") as f:
                    data = json.load(f)
                    geom = QByteArray.fromBase64(data.get("geometry", "").encode())
                    self.restoreGeometry(geom)
        except Exception:
            pass

    def closeEvent(self, event):
        data = {
            "geometry": self.saveGeometry().toBase64().data().decode()
        }
        try:
            with open("manual_ui_state.json", "w") as f:
                json.dump(data, f)
        except Exception:
            pass
        super().closeEvent(event)

    def copy_current_price(self):
        tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
        if tick:
            self.spin_man_price.setValue(tick.bid)

    def execute_manual_trade(self, order_type):
        vol = self.spin_man_vol.value()
        count = self.spin_man_count.value()
        sl = self.spin_man_sl.value()
        tp = self.spin_man_tp.value()
        price_input = self.spin_man_price.value()
        symbol = CONFIG["trade_symbol"]
        
        is_pending = order_type in [mt5.ORDER_TYPE_BUY_LIMIT, mt5.ORDER_TYPE_SELL_LIMIT, mt5.ORDER_TYPE_BUY_STOP, mt5.ORDER_TYPE_SELL_STOP]
        
        if is_pending and price_input <= 0:
             QMessageBox.warning(self, "Invalid Price", "Pending orders require a valid Price > 0.")
             return
        
        type_map = {
            mt5.ORDER_TYPE_BUY: "BUY",
            mt5.ORDER_TYPE_SELL: "SELL",
            mt5.ORDER_TYPE_BUY_LIMIT: "BUY LIMIT",
            mt5.ORDER_TYPE_SELL_LIMIT: "SELL LIMIT",
            mt5.ORDER_TYPE_BUY_STOP: "BUY STOP",
            mt5.ORDER_TYPE_SELL_STOP: "SELL STOP"
        }
        type_str = type_map.get(order_type, "UNKNOWN")
        
        # Confirmation
        msg = QMessageBox(self)
        msg.setWindowTitle("Confirm Trade")
        price_msg = f" @ {price_input}" if is_pending else " @ Market"
        msg.setText(f"Execute {count} x {type_str} {vol} lots on {symbol}{price_msg}?")
        msg.setInformativeText(f"SL: {sl:.2f} | TP: {tp:.2f}")
        msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
        msg.setDefaultButton(QMessageBox.No)
        
        if msg.exec() != QMessageBox.Yes:
            return
            
        for i in range(count):
            self._send_manual_order(symbol, order_type, vol, sl, tp, price_input)
            time.sleep(0.05)

    def _send_manual_order(self, symbol, order_type, vol, sl, tp, price_input=0.0):
        tick = mt5.symbol_info_tick(symbol)
        if not tick: return
        
        is_pending = order_type in [mt5.ORDER_TYPE_BUY_LIMIT, mt5.ORDER_TYPE_SELL_LIMIT, mt5.ORDER_TYPE_BUY_STOP, mt5.ORDER_TYPE_SELL_STOP]
        
        if is_pending:
            action = mt5.TRADE_ACTION_PENDING
            price = float(price_input)
        else:
            action = mt5.TRADE_ACTION_DEAL
            price = tick.ask if order_type == mt5.ORDER_TYPE_BUY else tick.bid
        
        # Filling mode logic
        symbol_info = mt5.symbol_info(symbol)
        filling_type = mt5.ORDER_FILLING_FOK # Default
        if symbol_info:
            if action == mt5.TRADE_ACTION_PENDING:
                filling_type = mt5.ORDER_FILLING_RETURN
            else:
                # 1 = FOK, 2 = IOC
                if symbol_info.filling_mode & 2: filling_type = mt5.ORDER_FILLING_IOC
                elif symbol_info.filling_mode & 1: filling_type = mt5.ORDER_FILLING_FOK
        
        req = {
            "action": action,
            "symbol": symbol,
            "volume": float(vol),
            "type": order_type,
            "price": price,
            "sl": float(sl),
            "tp": float(tp),
            "magic": 0,
            "comment": "GUI Manual",
            "type_time": mt5.ORDER_TIME_GTC,
            "type_filling": filling_type,
        }
        
        res = mt5.order_send(req)
        with state["lock"]:
            if res and res.retcode == mt5.TRADE_RETCODE_DONE:
                type_map = {
                    mt5.ORDER_TYPE_BUY: "BUY", mt5.ORDER_TYPE_SELL: "SELL",
                    mt5.ORDER_TYPE_BUY_LIMIT: "BUY LIMIT", mt5.ORDER_TYPE_SELL_LIMIT: "SELL LIMIT",
                    mt5.ORDER_TYPE_BUY_STOP: "BUY STOP", mt5.ORDER_TYPE_SELL_STOP: "SELL STOP"
                }
                t_str = type_map.get(order_type, "EXEC")
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": str(res.order),
                    "type": "Manual Exec",
                    "details": f"{t_str} {vol} @ {price}"
                })
            else:
                err = res.comment if res else "Unknown"
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": "-",
                    "type": "Exec Fail",
                    "details": err
                })

class DashboardGUI(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("XAU Scalper v4")
        # Set Window Icon if available
        if os.path.exists("icon.ico"):
            self.setWindowIcon(QIcon("icon.ico"))
        self.resize(650, 850)
        self.setMinimumSize(500, 600)
        
        self.last_price = 0.0
        self.load_ui_state()
        self.blink_state = False
        self.has_active_trades = False
        
        # Apply Global Stylesheet
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        # Threading Setup
        self.mt5_thread = QThread()
        self.worker = MT5DataWorker()
        self.worker.moveToThread(self.mt5_thread)
        
        self.mt5_thread.started.connect(self.worker.start_working)
        self.worker.data_updated.connect(self.on_data_received)
        
        # Start Thread
        self.mt5_thread.start()
        
        self.manual_dialog = ManualExecutionDialog(self)

        self.central_widget = QWidget()
        self.setCentralWidget(self.central_widget)
        self.main_layout = QVBoxLayout(self.central_widget)
        self.main_layout.setSpacing(15)
        self.main_layout.setContentsMargins(15, 15, 15, 15)

        self.setup_ui()
        
        self.blink_timer = QTimer()
        self.blink_timer.timeout.connect(self.blink_labels)
        self.blink_timer.start(800) # Blink every 800ms
        
        # Maps for Tree Optimization
        self.position_items: Dict[int, QTreeWidgetItem] = {}

    def load_ui_state(self):
        try:
            with open("ui_state.json", "r") as f:
                data = json.load(f)
                geom = QByteArray.fromBase64(data.get("geometry", "").encode())
                self.restoreGeometry(geom)
        except Exception:
            pass

    def closeEvent(self, event):
        msg = QMessageBox(self)
        msg.setWindowTitle("Confirm Exit")
        msg.setText("Are you sure you want to exit?")
        msg.setIcon(QMessageBox.Question)
        msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
        msg.setDefaultButton(QMessageBox.No)
        
        if msg.exec() == QMessageBox.Yes:
            # Stop Worker
            self.worker.stop_working()
            self.mt5_thread.quit()
            self.mt5_thread.wait(2000)
            
            data = {
                "geometry": self.saveGeometry().toBase64().data().decode()
            }
            with open("ui_state.json", "w") as f:
                json.dump(data, f)
            super().closeEvent(event)
        else:
            event.ignore()

    def blink_labels(self):
        self.blink_state = not self.blink_state

    def setup_ui(self):
        # 1. HUD Header (Simplified)
        header_frame = QFrame()
        header_frame.setProperty("class", "Panel")
        header_layout = QHBoxLayout(header_frame)
        header_layout.setContentsMargins(15, 10, 15, 10)
        header_layout.setSpacing(20)
        
        # Symbol & Price
        self.lbl_symbol = QLabel(CONFIG["trade_symbol"])
        self.lbl_symbol.setStyleSheet("font-size: 10pt; font-weight: bold; color: #88C0D0;")
        header_layout.addWidget(self.lbl_symbol)
        
        self.price_label = QLabel("-- / --")
        self.price_label.setProperty("class", "Price")
        header_layout.addWidget(self.price_label)
        
        self.lbl_atr = QLabel("ATR: 0.00")
        self.lbl_atr.setStyleSheet("font-size: 12pt; color: #EBCB8B; font-weight: bold; margin-left: 10px;")
        header_layout.addWidget(self.lbl_atr)
        
        self.atr_indicator = StatusCircle(size=12, color="#4C566A")
        self.atr_indicator.setToolTip("Volatility Indicator (Green=Low, Red=High)")
        header_layout.addWidget(self.atr_indicator)
        
        header_layout.addStretch()
        
        # Net P/L
        self.lbl_open_pl = QLabel("$0.00")
        self.lbl_open_pl.setStyleSheet("font-size: 20pt; font-weight: bold; color: #ECEFF4;")
     
        
        # Settings
        self.btn_settings = QPushButton("⚙")
        self.btn_settings.setFixedSize(32, 32)
        self.btn_settings.setCursor(Qt.PointingHandCursor)
        self.btn_settings.clicked.connect(self.open_advanced_settings)
        header_layout.addWidget(self.btn_settings)

        self.main_layout.addWidget(header_frame)
        
        # Counts Row
        counts_frame = QFrame()
        counts_layout = QHBoxLayout(counts_frame)
        counts_layout.setContentsMargins(0, 0, 0, 0)
        
        self.lbl_scalp_count = QLabel("Scalp: 0")
        self.lbl_swing_count = QLabel("Swing: 0")
        self.lbl_manual_count = QLabel("Manual: 0")
        
        counts_layout.addStretch()
        counts_layout.addWidget(self.lbl_scalp_count)
        counts_layout.addSpacing(20)
        counts_layout.addWidget(self.lbl_swing_count)
        counts_layout.addSpacing(20)
        counts_layout.addWidget(self.lbl_manual_count)
        counts_layout.addStretch()
        
        self.main_layout.addWidget(counts_frame)

        # 2. System State (Toggles) - Prominent
        toggles_frame = QFrame()
        toggles_frame.setProperty("class", "Panel")
        toggles_layout = QGridLayout(toggles_frame)
        toggles_layout.setVerticalSpacing(15)
        toggles_layout.setHorizontalSpacing(20)
        
        self.create_toggle(toggles_layout, "Scalp Mode", "scalp_mode", 0, 0)
        self.create_toggle(toggles_layout, "Swing Mode", "swing_mode", 0, 1)
        self.create_toggle(toggles_layout, "Trailing Scalp", "use_trailing_scalp", 1, 0)
        self.create_toggle(toggles_layout, "Trailing Swing", "use_trailing_swing", 1, 1)
        self.create_toggle(toggles_layout, "Force Market", "force_market", 2, 0)
        self.create_toggle(toggles_layout, "Manage Manual", "manage_manual", 2, 1, "Enable Trailing Stop logic for manual trades")
        
        self.main_layout.addWidget(toggles_frame)

        # NEW: Manual Trade Section
        self.manual_btn = QPushButton("🎮 Manual Execution")
        self.manual_btn.setCursor(Qt.PointingHandCursor)
        self.manual_btn.clicked.connect(self.open_manual_panel)
        self.main_layout.addWidget(self.manual_btn)


        # 4. Tabs
        self.tabs = QTabWidget()
        self.main_layout.addWidget(self.tabs)
        
        # Tab 2: Open Positions
        self.pos_tab = QWidget()
        pos_layout = QVBoxLayout(self.pos_tab)
        
        self.tree_pos = QTreeWidget()
        self.tree_pos.setHeaderLabels(["#", "Type", "Vol", "P/L", ""])
        self.tree_pos.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.tree_pos.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.tree_pos.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.tree_pos.header().setSectionResizeMode(3, QHeaderView.Stretch)
        self.tree_pos.header().setSectionResizeMode(4, QHeaderView.Fixed)
        self.tree_pos.setColumnWidth(4, 40)
        self.tree_pos.headerItem().setTextAlignment(0, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_pos.headerItem().setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_pos.headerItem().setTextAlignment(2, Qt.AlignRight | Qt.AlignVCenter)
        self.tree_pos.headerItem().setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
        self.tree_pos.setAlternatingRowColors(True)
        self.tree_pos.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree_pos.customContextMenuRequested.connect(self.show_context_menu)
        pos_layout.addWidget(self.tree_pos)
        
        # Summary Label for Open Positions Tab
        self.lbl_tab_open_pl = QLabel("Total P/L: $0.00")
        self.lbl_tab_open_pl.setAlignment(Qt.AlignRight)
        self.lbl_tab_open_pl.setStyleSheet("font-weight: bold; font-size: 12pt; color: #D8DEE9; margin-top: 5px; margin-bottom: 5px;")
        pos_layout.addWidget(self.lbl_tab_open_pl)
        
        # Combined Controls Frame (Compact)
        controls_frame = QFrame()
        controls_frame.setProperty("class", "Panel")
        controls_layout = QGridLayout(controls_frame)
        controls_layout.setContentsMargins(5, 5, 5, 5)
        controls_layout.setSpacing(5)

        # --- Row 0: SL/TP Controls ---
        self.spin_update_sl = QDoubleSpinBox()
        self.spin_update_sl.setDecimals(2)
        self.spin_update_sl.setRange(0, 99999)
        self.spin_update_sl.setPrefix("SL: ")
        self.spin_update_sl.setToolTip("Set to 0.00 to keep existing SL unchanged.")
        self.spin_update_sl.setValue(0)
        self.spin_update_sl.setFixedHeight(28)
        self.spin_update_sl.setButtonSymbols(QAbstractSpinBox.NoButtons)
        
        self.btn_copy_sl = QPushButton("📍")
        self.btn_copy_sl.setFixedSize(24, 28)
        self.btn_copy_sl.setCursor(Qt.PointingHandCursor)
        self.btn_copy_sl.setToolTip("Copy current price to SL")
        self.btn_copy_sl.clicked.connect(lambda: self.copy_price_to_field(self.spin_update_sl))
        
        sl_container = QWidget()
        sl_layout = QHBoxLayout(sl_container)
        sl_layout.setContentsMargins(0,0,0,0)
        sl_layout.setSpacing(2)
        sl_layout.addWidget(self.spin_update_sl)
        sl_layout.addWidget(self.btn_copy_sl)
        
        self.spin_update_tp = QDoubleSpinBox()
        self.spin_update_tp.setDecimals(2)
        self.spin_update_tp.setRange(0, 99999)
        self.spin_update_tp.setPrefix("TP: ")
        self.spin_update_tp.setToolTip("Set to 0.00 to keep existing TP unchanged.")
        self.spin_update_tp.setValue(0)
        self.spin_update_tp.setFixedHeight(28)
        self.spin_update_tp.setButtonSymbols(QAbstractSpinBox.NoButtons)
        
        self.btn_copy_tp = QPushButton("📍")
        self.btn_copy_tp.setFixedSize(24, 28)
        self.btn_copy_tp.setCursor(Qt.PointingHandCursor)
        self.btn_copy_tp.setToolTip("Copy current price to TP")
        self.btn_copy_tp.clicked.connect(lambda: self.copy_price_to_field(self.spin_update_tp))
        
        tp_container = QWidget()
        tp_layout = QHBoxLayout(tp_container)
        tp_layout.setContentsMargins(0,0,0,0)
        tp_layout.setSpacing(2)
        tp_layout.addWidget(self.spin_update_tp)
        tp_layout.addWidget(self.btn_copy_tp)
        
        self.btn_update_selected = QPushButton("Update TP/SL")
        self.btn_update_selected.setFixedHeight(28)
        self.btn_update_selected.setCursor(Qt.PointingHandCursor)
        self.btn_update_selected.clicked.connect(self.update_selected_sltp)
        self.btn_update_selected.setToolTip("Apply these SL/TP settings ONLY to the currently selected trade.")
        
        self.btn_update_manual = QPushButton("Update Man")
        self.btn_update_manual.setFixedHeight(28)
        self.btn_update_manual.setCursor(Qt.PointingHandCursor)
        self.btn_update_manual.clicked.connect(self.update_manual_sltp)
        self.btn_update_manual.setToolTip("Apply these SL/TP settings to ALL manual trades at once.")
        
        controls_layout.addWidget(sl_container, 0, 0)
        controls_layout.addWidget(tp_container, 0, 1)
        controls_layout.addWidget(self.btn_update_selected, 0, 2)
        controls_layout.addWidget(self.btn_update_manual, 0, 3)
        
        # --- Row 1: Bulk Actions ---
        self.close_combo = QComboBox()
        self.close_combo.addItems(["Close All Scalp", "Close All Swing", "Close All Manual", "Close Scalp Winners", "Close Scalp Losers"])
        self.close_combo.setFixedHeight(30)
        
        self.exec_btn = SafetyButton("HOLD TO EXECUTE")
        self.exec_btn.triggered.connect(self.execute_close_action)
        
        controls_layout.addWidget(self.close_combo, 1, 0, 1, 2)
        controls_layout.addWidget(self.exec_btn, 1, 2, 1, 2)
        
        pos_layout.addWidget(controls_frame)
        
        # Auto-fill SL/TP on selection
        self.tree_pos.itemSelectionChanged.connect(self.on_trade_selected)

        self.tabs.addTab(self.pos_tab, "Open")

        # Tab 3: History
        self.hist_tab = QWidget()
        hist_layout = QVBoxLayout(self.hist_tab)
        
        self.tree_hist = QTreeWidget()
        self.tree_hist.setHeaderLabels(["#", "Type", "Vol", "P/L",""])
        self.tree_hist.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.tree_hist.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.tree_hist.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.tree_hist.header().setSectionResizeMode(3, QHeaderView.Stretch)
        self.tree_hist.headerItem().setTextAlignment(0, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_hist.headerItem().setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_hist.headerItem().setTextAlignment(2, Qt.AlignRight | Qt.AlignVCenter)
        self.tree_hist.headerItem().setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
        self.tree_hist.setAlternatingRowColors(True)
        hist_layout.addWidget(self.tree_hist)
        
        self.total_profit_label = QLabel("Bal: $0.00 | P/L: $0.00")
        self.total_profit_label.setAlignment(Qt.AlignRight)
        self.total_profit_label.setStyleSheet("font-weight: bold; font-size: 10pt;")
        hist_layout.addWidget(self.total_profit_label)
        
        perf_btn_layout = QHBoxLayout()
        
        self.btn_export_perf = QPushButton("Export Perf CSV")
        self.btn_export_perf.clicked.connect(self.export_performance_csv)
        perf_btn_layout.addWidget(self.btn_export_perf)
        
        self.btn_clear_perf = QPushButton("Clear Perf CSV")
        self.btn_clear_perf.clicked.connect(self.clear_performance_csv)
        perf_btn_layout.addWidget(self.btn_clear_perf)
        
        hist_layout.addLayout(perf_btn_layout)
        
        self.tabs.addTab(self.hist_tab, "History")

        # Tab 4: Logs
        self.log_tab = QWidget()
        log_layout = QVBoxLayout(self.log_tab)
        
        self.tree_log = QTreeWidget()
        self.tree_log.setHeaderLabels(["Time", "Ticket", "Type", "Details"])
        self.tree_log.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.tree_log.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.tree_log.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.tree_log.header().setSectionResizeMode(3, QHeaderView.ResizeToContents)
        self.tree_log.headerItem().setTextAlignment(0, Qt.AlignLeft | Qt.AlignVCenter)
        self.tree_log.headerItem().setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_log.headerItem().setTextAlignment(2, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_log.headerItem().setTextAlignment(3, Qt.AlignLeft | Qt.AlignVCenter)
        self.tree_log.setAlternatingRowColors(True)
        self.tree_log.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree_log.customContextMenuRequested.connect(self.show_log_context_menu)
        log_layout.addWidget(self.tree_log)
        
        btn_layout = QHBoxLayout()
        
        self.btn_export_logs = QPushButton("Export CSV")
        self.btn_export_logs.clicked.connect(self.export_logs_to_csv)
        btn_layout.addWidget(self.btn_export_logs)
        
        self.btn_clear_logs = QPushButton("Clear Logs")
        self.btn_clear_logs.clicked.connect(self.clear_logs)
        btn_layout.addWidget(self.btn_clear_logs)
        
        log_layout.addLayout(btn_layout)
        
        self.tabs.addTab(self.log_tab, "Logs")

        # 5. Status Bar (Footer)
        footer_frame = QFrame()
        footer_frame.setStyleSheet("background-color: #2E3440; border-top: 1px solid #434C5E; border-radius: 0px;")
        footer_layout = QHBoxLayout(footer_frame)
        footer_layout.setContentsMargins(4, 2, 4, 2)
        
        self.signal_indicator = SignalIndicator()
        self.status_label = QLabel("Connecting...")
        self.status_label.setStyleSheet("color: #D8DEE9; font-size: 8pt; font-weight: bold; padding: 2px;")
        
        self.ping_indicator = StatusCircle(size=10, color="#4C566A")
        self.lbl_latency = QLabel("Ping: -- ms")
        self.lbl_latency.setStyleSheet("color: #D8DEE9; font-size: 6pt;")
        
        footer_layout.addWidget(self.signal_indicator)
        footer_layout.addWidget(self.status_label)
        footer_layout.addSpacing(10)
        footer_layout.addWidget(self.ping_indicator)
        footer_layout.addWidget(self.lbl_latency)
        footer_layout.addStretch()
        
        self.main_layout.addWidget(footer_frame)

    def create_toggle(self, layout, label, key, row, col, tooltip=None):
        container = QWidget()
        h_layout = QHBoxLayout(container)
        h_layout.setContentsMargins(0,0,0,0)
        
        lbl = QLabel(label)
        cb = ToggleSwitch()
        cb.setChecked(CONFIG[key])
        def on_toggle(s, k=key):
            CONFIG[k] = bool(s)
        cb.stateChanged.connect(on_toggle)
        
        if tooltip:
            container.setToolTip(tooltip)
        
        h_layout.addWidget(lbl)
        h_layout.addStretch()
        h_layout.addWidget(cb)
        
        layout.addWidget(container, row, col)

    def open_advanced_settings(self):
        dlg = AdvancedSettingsDialog(self)
        dlg.exec()

    def open_manual_panel(self):
        self.manual_dialog.show()
        self.manual_dialog.activateWindow()

    def show_context_menu(self, pos):
        item = self.tree_pos.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #3B4252; color: #ECEFF4; } QMenu::item:selected { background-color: #88C0D0; color: #2E3440; }")
            close_action = menu.addAction("Close Position")
            action = menu.exec(self.tree_pos.mapToGlobal(pos))
            if action == close_action:
                self.close_selected(item)

    def on_trade_selected(self):
        items = self.tree_pos.selectedItems()
        if not items:
            return
            
        item = items[0]
        ticket_str = item.text(0)
        if not ticket_str.isdigit(): return
        
        positions = mt5.positions_get(ticket=int(ticket_str))
        if positions:
            self.spin_update_sl.setValue(positions[0].sl)
            self.spin_update_tp.setValue(positions[0].tp)

    def close_ticket(self, ticket):
        positions = mt5.positions_get(ticket=ticket)
        if positions:
            self._send_close_request(positions[0])

    def show_log_context_menu(self, pos):
        item = self.tree_log.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #3B4252; color: #ECEFF4; } QMenu::item:selected { background-color: #88C0D0; color: #2E3440; }")
            copy_action = menu.addAction("Copy Log")
            action = menu.exec(self.tree_log.mapToGlobal(pos))
            if action == copy_action:
                text = f"[{item.text(0)}] Ticket:{item.text(1)} Type:{item.text(2)} - {item.text(3)}"
                QApplication.clipboard().setText(text)

    def _send_close_request(self, pos):
        tick = mt5.symbol_info_tick(pos.symbol)
        if not tick: return
        
        type_close = mt5.ORDER_TYPE_SELL if pos.type == mt5.ORDER_TYPE_BUY else mt5.ORDER_TYPE_BUY
        price_close = tick.bid if pos.type == mt5.ORDER_TYPE_BUY else tick.ask
        
        req = {
            "action": mt5.TRADE_ACTION_DEAL,
            "symbol": pos.symbol,
            "position": pos.ticket,
            "volume": pos.volume,
            "type": type_close,
            "price": price_close,
            "magic": pos.magic,
            "comment": "GUI Close"
        }
        res = mt5.order_send(req)
        
        with state["lock"]:
            if res and res.retcode == mt5.TRADE_RETCODE_DONE:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": str(pos.ticket),
                    "type": "Manual Close",
                    "details": f"Closed {pos.volume} lots"
                })
            else:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": str(pos.ticket),
                    "type": "Close Fail",
                    "details": res.comment if res else "Unknown Error"
                })

    def close_selected(self, item):
        ticket = int(item.text(0))
        positions = mt5.positions_get(ticket=ticket)
        if positions: self._send_close_request(positions[0])

    def execute_close_action(self):
        action = self.close_combo.currentText()
        mode = ""
        if action == "Close All Scalp": mode = "scalp"
        elif action == "Close All Swing": mode = "swing"
        elif action == "Close All Manual": mode = "manual"
        elif action == "Close Scalp Winners": mode = "scalp_profit"
        elif action == "Close Scalp Losers": mode = "scalp_loss"
        
        if mode: self.close_bulk(mode)

    def _send_sltp_update(self, pos, new_sl, new_tp):
        symbol_info = mt5.symbol_info(pos.symbol)
        if not symbol_info: return
        
        tick_size = symbol_info.trade_tick_size
        digits = symbol_info.digits
        
        def normalize(val):
            return round(round(val / tick_size) * tick_size, digits)

        # If 0, keep existing
        sl = new_sl if new_sl > 0 else pos.sl
        tp = new_tp if new_tp > 0 else pos.tp
        
        sl = normalize(sl)
        tp = normalize(tp)
        
        if abs(sl - pos.sl) < tick_size and abs(tp - pos.tp) < tick_size:
            return

        req = {
            "action": mt5.TRADE_ACTION_SLTP,
            "position": pos.ticket,
            "sl": float(sl),
            "tp": float(tp),
            "symbol": pos.symbol
        }
        
        res = mt5.order_send(req)
        with state["lock"]:
            if res and res.retcode == mt5.TRADE_RETCODE_DONE:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": str(pos.ticket),
                    "type": "SL/TP Update",
                    "details": f"SL: {sl} TP: {tp}"
                })
            else:
                err = res.comment if res else "Unknown"
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": str(pos.ticket),
                    "type": "Update Fail",
                    "details": err
                })

    def update_manual_sltp(self):
        sl = self.spin_update_sl.value()
        tp = self.spin_update_tp.value()
        
        if sl == 0 and tp == 0:
            QMessageBox.warning(self, "Warning", "Please enter a SL or TP value.")
            return
        
        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        if not positions: return
        
        count = 0
        for pos in positions:
            if pos.magic == 0: # Manual
                self._send_sltp_update(pos, sl, tp)
                count += 1
        
        if count == 0:
             QMessageBox.information(self, "Info", "No manual trades found.")

    def update_selected_sltp(self):
        sl = self.spin_update_sl.value()
        tp = self.spin_update_tp.value()
        
        if sl == 0 and tp == 0:
            QMessageBox.warning(self, "Warning", "Please enter a SL or TP value.")
            return
        
        item = self.tree_pos.currentItem()
        if not item:
            QMessageBox.warning(self, "Warning", "No trade selected.")
            return
            
        ticket_str = item.text(0)
        if not ticket_str.isdigit(): return
        
        ticket = int(ticket_str)
        positions = mt5.positions_get(ticket=ticket)
        if positions:
            self._send_sltp_update(positions[0], sl, tp)

    def copy_price_to_field(self, field):
        tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
        if tick:
            field.setValue(tick.bid)

    def export_logs_to_csv(self):
        filename, _ = QFileDialog.getSaveFileName(self, "Export Logs", "logs_export.csv", "CSV Files (*.csv)")
        if not filename:
            return
            
        try:
            with open(filename, "w", newline="", encoding="utf-8") as f:
                writer = csv.writer(f)
                # Write Header
                headers = []
                for i in range(self.tree_log.columnCount()):
                    headers.append(self.tree_log.headerItem().text(i))
                writer.writerow(headers)
                
                # Write Rows
                root = self.tree_log.invisibleRootItem()
                for i in range(root.childCount()):
                    item = root.child(i)
                    row = []
                    for c in range(self.tree_log.columnCount()):
                        row.append(item.text(c))
                    writer.writerow(row)
            
            QMessageBox.information(self, "Export Successful", f"Logs exported to {filename}")
        except Exception as e:
            QMessageBox.critical(self, "Export Failed", f"Error exporting logs: {str(e)}")

    def clear_logs(self):
        self.tree_log.clear()

    def export_performance_csv(self):
        src_filename = "strategy_performance.csv"
        if not os.path.exists(src_filename):
            QMessageBox.information(self, "Export Failed", "No performance data found.")
            return

        filename, _ = QFileDialog.getSaveFileName(self, "Export Performance", "performance_export.csv", "CSV Files (*.csv)")
        if not filename:
            return
            
        try:
            shutil.copy2(src_filename, filename)
            QMessageBox.information(self, "Export Successful", f"Performance data exported to {filename}")
        except Exception as e:
            QMessageBox.critical(self, "Export Failed", f"Error exporting data: {str(e)}")

    def clear_performance_csv(self):
        src_filename = "strategy_performance.csv"
        if not os.path.exists(src_filename):
            QMessageBox.information(self, "Clear Failed", "No performance data found.")
            return

        msg = QMessageBox(self)
        msg.setWindowTitle("Confirm Clear")
        msg.setText("Are you sure you want to clear the performance history CSV?")
        msg.setIcon(QMessageBox.Warning)
        msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
        msg.setDefaultButton(QMessageBox.No)
        
        if msg.exec() == QMessageBox.Yes:
            try:
                os.remove(src_filename)
                QMessageBox.information(self, "Clear Successful", "Performance history cleared.")
            except Exception as e:
                QMessageBox.critical(self, "Clear Failed", f"Error clearing data: {str(e)}")

    def close_bulk(self, mode):
        # SafetyButton already handles the "Confirm" aspect via hold-to-trigger.
        # We can skip the popup for smoother UX, or keep it for "loss" only if strictly needed.
        # Given the requirement to remove popups for SafetyButton, we proceed directly.

        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        if not positions: return
        for pos in positions:
            if pos.magic not in [CONFIG["magic_number"], CONFIG["magic_number"]+1, 0]: continue
            
            is_scalp = pos.magic == CONFIG["magic_number"]
            is_swing = pos.magic == CONFIG["magic_number"] + 1
            is_manual = pos.magic == 0
            
            if mode == "scalp" and is_scalp: self._send_close_request(pos)
            elif mode == "swing" and is_swing: self._send_close_request(pos)
            elif mode == "manual" and is_manual: self._send_close_request(pos)
            elif mode == "scalp_profit" and is_scalp and pos.profit > 0: self._send_close_request(pos)
            elif mode == "scalp_loss" and is_scalp and pos.profit < 0: self._send_close_request(pos)

    @Slot(dict)
    def on_data_received(self, data: Dict[str, Any]):
        # Status
        txt = data.get("status_text", "")
        connected = data.get("connected", False)
        trade_allowed = data.get("trade_allowed", False)
        
        base_style = "font-size: 8pt; font-weight: bold; padding: 2px; border-radius: 3px;"
        if connected and not trade_allowed:
            self.status_label.setText("⚠️ AutoTrading OFF")
            self.status_label.setStyleSheet(f"color: #EBCB8B; background-color: #3B4252; {base_style}")
        else:
            self.status_label.setText(txt)
            if "Connected" in txt: self.status_label.setStyleSheet(f"color: #A3BE8C; background-color: #3B4252; {base_style}")
            elif any(x in txt for x in ["Error", "Disconnected", "Failed"]): self.status_label.setStyleSheet(f"color: #BF616A; background-color: #3B4252; {base_style}")
            else: self.status_label.setStyleSheet(f"color: #EBCB8B; background-color: #3B4252; {base_style}")
        
        # ATR
        atr = data.get("server_atr", 0.0)
        self.lbl_atr.setText(f"ATR: {atr:.2f}")
        
        # ATR Indicator Color
        threshold = CONFIG.get("atr_high_vol_threshold", 1.0)
        if atr > (threshold * 2.0):
            # Extreme Volatility: Blink Red/Yellow
            if self.blink_state:
                self.atr_indicator.set_color("#BF616A") # Red
            else:
                self.atr_indicator.set_color("#EBCB8B") # Yellow
        elif atr > threshold:
            self.atr_indicator.set_color("#BF616A") # Red (High Volatility)
        else:
            self.atr_indicator.set_color("#A3BE8C") # Green (Normal)
        
        # Update Footer
        ping_ms = data.get("ping", 0)
        self.lbl_latency.setText(f"Ping: {ping_ms} ms")
        if ping_ms < 100: self.lbl_latency.setStyleSheet("color: #A3BE8C; font-size: 9pt;")
        elif ping_ms < 300: self.lbl_latency.setStyleSheet("color: #EBCB8B; font-size: 9pt;")
        else: self.lbl_latency.setStyleSheet("color: #BF616A; font-size: 9pt;")
            
        # Price
        bid = data.get("bid", 0.0)
        ask = data.get("ask", 0.0)
        self.price_label.setText(f"{bid:.2f} / {ask:.2f}")
        
        if self.last_price > 0:
            if bid > self.last_price:
                self.price_label.setStyleSheet('font-family: "Consolas"; font-weight: bold; font-size: 20pt; color: #A3BE8C;')
            elif bid < self.last_price:
                self.price_label.setStyleSheet('font-family: "Consolas"; font-weight: bold; font-size: 20pt; color: #BF616A;')
        
        self.last_price = bid
        
        # Counts
        counts = data.get("counts", {"scalp": 0, "swing": 0, "manual": 0})
        sc, sw, mn = counts["scalp"], counts["swing"], counts["manual"]
        
        self.lbl_scalp_count.setText(f"Scalp: {sc}")
        self.lbl_swing_count.setText(f"Swing: {sw}")
        self.lbl_manual_count.setText(f"Manual: {mn}")
        self.has_active_trades = (sc > 0 or sw > 0 or mn > 0)
        
        # Dynamic Styling with Blink
        base_style = "font-family: Consolas; font-size: 10pt; font-weight: bold;"
        dim_style = "color: #4C566A; font-family: Consolas; font-size: 10pt;"
        
        if sc > 0:
            color = "#A3BE8C" if self.blink_state else "#B5D19E"
            self.lbl_scalp_count.setStyleSheet(f"color: {color}; {base_style}")
        else:
            self.lbl_scalp_count.setStyleSheet(dim_style)
            
        if sw > 0:
            color = "#88C0D0" if self.blink_state else "#81A1C1"
            self.lbl_swing_count.setStyleSheet(f"color: {color}; {base_style}")
        else:
            self.lbl_swing_count.setStyleSheet(dim_style)
            
        if mn > 0:
            color = "#EBCB8B" if self.blink_state else "#D08770"
            self.lbl_manual_count.setStyleSheet(f"color: {color}; {base_style}")
        else:
            self.lbl_manual_count.setStyleSheet(dim_style)
        
        # Positions Tree Optimization
        positions = data.get("positions", [])
        current_tickets = set()
        
        for pos in positions:
            ticket = pos["ticket"]
            current_tickets.add(ticket)
            
            # Update or Create
            if ticket in self.position_items:
                item = self.position_items[ticket]
            else:
                item = QTreeWidgetItem()
                self.tree_pos.addTopLevelItem(item)
                self.position_items[ticket] = item
                
                # Add Close Button
                btn_close = QPushButton("X")
                btn_close.setFixedSize(24, 20)
                btn_close.setCursor(Qt.PointingHandCursor)
                btn_close.setProperty("class", "Danger")
                btn_close.setStyleSheet("padding: 0px; font-size: 10px;")
                btn_close.clicked.connect(lambda _, t=ticket: self.close_ticket(t))
                self.tree_pos.setItemWidget(item, 4, btn_close)
                
                # Alignments
                for c in range(5):
                    align = Qt.AlignRight if c in [2,3] else Qt.AlignCenter
                    item.setTextAlignment(c, align | Qt.AlignVCenter)

            # Update Data
            t_type = pos["type"]
            if pos["magic"] == 0: t_type += " (M)"
            
            item.setText(0, str(ticket))
            item.setText(1, t_type)
            item.setText(2, str(pos["volume"]))
            item.setText(3, f"{pos['profit']:.2f}")
            
            color = QColor("#A3BE8C") if pos["profit"] >= 0 else QColor("#BF616A")
            if pos["magic"] == 0:
                item.setForeground(1, QBrush(QColor("#EBCB8B")))
            else:
                item.setForeground(1, QBrush(QColor("#D8DEE9")))
            item.setForeground(3, QBrush(color))

        # Remove closed positions
        tickets_to_remove = []
        for ticket in self.position_items:
            if ticket not in current_tickets:
                tickets_to_remove.append(ticket)
        
        for ticket in tickets_to_remove:
            item = self.position_items.pop(ticket)
            index = self.tree_pos.indexOfTopLevelItem(item)
            self.tree_pos.takeTopLevelItem(index)
        
        # Open P/L
        open_pl = data.get("open_pl", 0.0)
        color_hex = "#A3BE8C" if open_pl >= 0 else "#BF616A"
        self.lbl_open_pl.setText(f"${open_pl:.2f}")
        self.lbl_open_pl.setStyleSheet(f"font-size: 20pt; font-weight: bold; color: {color_hex};")
        self.lbl_tab_open_pl.setText(f"Total P/L: ${open_pl:.2f}")
        self.lbl_tab_open_pl.setStyleSheet(f"font-weight: bold; font-size: 12pt; color: {color_hex}; margin-top: 5px;")
        
        # History Tree
        # For simplicity, we clear history if count differs, or just rebuild.
        # Since history is append-only mostly, we could optimize, but clear/fill is safer for now given low frequency.
        self.tree_hist.clear()
        history = data.get("history", [])
        for h in history:
            item = QTreeWidgetItem([str(h["ticket"]), h["type"], str(h["volume"]), f"{h['profit']:.2f}"])
            color = QColor("#A3BE8C") if h["profit"] >= 0 else QColor("#BF616A")
            item.setForeground(3, QBrush(color))
            item.setTextAlignment(0, Qt.AlignCenter | Qt.AlignVCenter)
            item.setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
            item.setTextAlignment(2, Qt.AlignRight | Qt.AlignVCenter)
            item.setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
            self.tree_hist.addTopLevelItem(item)
        
        balance = data.get("balance", 0.0)
        total_pl_24h = data.get("total_pl_24h", 0.0)
        self.total_profit_label.setText(f"Bal: ${balance:.2f}  |  P/L (24h): ${total_pl_24h:.2f}")
        color_hex = "#A3BE8C" if total_pl_24h >= 0 else "#BF616A"
        self.total_profit_label.setStyleSheet(f"font-weight: bold; font-size: 10pt; color: {color_hex};")
        
        # Update Logs
        logs_to_add = data.get("new_logs", [])
        
        for log in logs_to_add:
            item = QTreeWidgetItem([log["time"], str(log["ticket"]), log["type"], log["details"]])
            item.setTextAlignment(0, Qt.AlignLeft | Qt.AlignVCenter)
            item.setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
            item.setTextAlignment(2, Qt.AlignCenter | Qt.AlignVCenter)
            item.setTextAlignment(3, Qt.AlignLeft | Qt.AlignVCenter)
            self.tree_log.insertTopLevelItem(0, item)
            # Limit log size in UI to 100 items
            if self.tree_log.topLevelItemCount() > 100:
                self.tree_log.takeTopLevelItem(100)
        
        if logs_to_add:
            self.tree_log.scrollToItem(self.tree_log.topLevelItem(0))

        # Flash Signal Indicator
        diff = time.time() - data.get("last_signal_ts", 0)
        if diff < 3.0: # Flash for 3 seconds
            # Blink rapidly (approx every 250ms)
            is_active = (int(diff * 4) % 2 == 0)
            self.signal_indicator.set_active(is_active)
        else:
            self.signal_indicator.set_active(False)