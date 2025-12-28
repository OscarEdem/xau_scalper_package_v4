import sys
import json
import csv
from PySide6.QtWidgets import (QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,  # type: ignore
                               QLabel, QLineEdit, QSpinBox, QDoubleSpinBox, QCheckBox, 
                               QPushButton, QComboBox, QTabWidget, QTreeWidget, QTreeWidgetItem, 
                               QFrame, QMessageBox, QGridLayout, QHeaderView, QSizePolicy, QAbstractSpinBox,
                               QDialog, QScrollArea, QDialogButtonBox, QFormLayout, QMenu, QFileDialog)
from PySide6.QtCore import Qt, QTimer, Property, QPropertyAnimation, QEasingCurve, QByteArray # type: ignore
from PySide6.QtGui import QColor, QPainter, QBrush # type: ignore

import MetaTrader5 as mt5 # type: ignore
from datetime import datetime, timedelta
import time

from config import CONFIG, state

class SignalIndicator(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(16, 16)
        self.color = QColor("#333333")

    def set_active(self, active):
        new_color = QColor("#00E5FF") if active else QColor("#333333")
        if self.color != new_color:
            self.color = new_color
            self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        painter.setBrush(QBrush(self.color))
        painter.setPen(Qt.NoPen)
        painter.drawEllipse(2, 2, 12, 12)

class ToggleSwitch(QCheckBox):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(44, 24)
        self.setCursor(Qt.PointingHandCursor)
        self._circle_position = 2
        self._bg_color = QColor("#333333")
        self._circle_color = QColor("#FFFFFF")
        self._active_color = QColor("#2979FF")
        
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
        self.setWindowTitle("Advanced Settings")
        self.resize(450, 600)
        if parent:
            self.setStyleSheet(parent.styleSheet())
        
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
        
        self.add_section("Trailing Stop")
        self.add_input("Scalp Start (pips)", "trailing_start_pips_scalp", float, "Profit in pips required to activate trailing stop.")
        self.add_input("Scalp Dist (pips)", "trailing_dist_pips_scalp", float, "Distance in pips to maintain from current price.")
        self.add_input("Scalp Step (pips)", "trailing_step_pips_scalp", float, "Minimum price movement in pips to update stop loss.")
        self.add_input("Swing Start (pips)", "trailing_start_pips_swing", float, "Profit in pips required to activate trailing stop.")
        self.add_input("Swing Dist (pips)", "trailing_dist_pips_swing", float, "Distance in pips to maintain from current price.")
        self.add_input("Swing Step (pips)", "trailing_step_pips_swing", float, "Minimum price movement in pips to update stop loss.")
        
        self.add_section("Stagnation / Timeouts")
        self.add_bool("Use Stagnation", "use_stagnation", "Enable partial closing of trades that stall.")
        self.add_input("Stag. Sec", "stagnation_sec", int, "Seconds before a trade is considered stagnant.")
        self.add_input("Time Mult", "stag_time_mult", float, "Multiplier for stagnation time on Swing trades.")

        scroll.setWidget(content)
        layout.addWidget(scroll)
        
        btns = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        layout.addWidget(btns)

    def add_section(self, title):
        lbl = QLabel(title)
        lbl.setStyleSheet("font-weight: bold; color: #2979FF; font-size: 11pt; margin-top: 10px;")
        self.form_layout.addRow(lbl)

    def add_input(self, label, key, dtype, tooltip=None):
        val = CONFIG.get(key, 0)
        step = 0.1 if dtype == float and ("mult" in key or "pct" in key) else 1.0
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

    def accept(self):
        msg = QMessageBox(self)
        msg.setWindowTitle("Confirm Changes")
        msg.setText("Are you sure you want to apply these advanced settings?")
        msg.setIcon(QMessageBox.Question)
        msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
        msg.setDefaultButton(QMessageBox.No)
        msg.setStyleSheet("QMessageBox { background-color: #121212; color: white; } QPushButton { background-color: #2979FF; color: white; padding: 5px; }")
        
        if msg.exec() == QMessageBox.Yes:
            for key, (widget, dtype) in self.inputs.items():
                if dtype == bool:
                    CONFIG[key] = widget.isChecked()
                else:
                    CONFIG[key] = widget.value()
            super().accept()

class DashboardGUI(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("XAU Scalper v4")
        self.resize(600, 800)
        self.setMinimumSize(450, 500)
        
        
        self.last_price = 0.0
        self.load_ui_state()
        self.blink_state = False
        self.has_active_trades = False
        
        # Dark Theme Styling
        self.setStyleSheet("""
            QMainWindow { background-color: #121212; color: #E0E0E0; }
            QWidget { background-color: #121212; color: #E0E0E0; font-family: "Segoe UI"; font-size: 10pt; }
            QFrame.Panel { background-color: #1E1E1E; border-radius: 8px; }
            QLabel { background-color: transparent; }
            QLabel.Header { font-weight: bold; color: #B0BEC5; font-size: 11pt; }
            QLabel.Status { font-weight: bold; font-size: 10pt; padding: 4px 8px; border-radius: 4px; background-color: #2D2D2D; }
            QLabel.Price { font-family: "Consolas"; font-weight: bold; font-size: 24pt; color: #FFFFFF; }
            QLineEdit, QSpinBox, QDoubleSpinBox { background-color: #2D2D2D; color: white; border: 1px solid #333; padding: 4px; border-radius: 4px; }
            QPushButton.SpinBtn { background-color: #3E3E3E; border: 1px solid #555; border-radius: 4px; font-weight: bold; font-size: 14px; color: #E0E0E0; }
            QPushButton.SpinBtn:hover { background-color: #505050; border-color: #2979FF; color: #FFFFFF; }
            QPushButton.SpinBtn:pressed { background-color: #2979FF; }
            QPushButton.Config { background-color: #1E1E1E; border: 1px solid #333; color: #B0BEC5; text-align: left; padding: 8px; }
            QPushButton.Config:hover { background-color: #2D2D2D; color: white; }
            QTabWidget::pane { border: 1px solid #2D2D2D; background: #1E1E1E; }
            QTabBar::tab { background: #1E1E1E; color: #E0E0E0; padding: 8px 16px; border-top-left-radius: 4px; border-top-right-radius: 4px; }
            QTabBar::tab:selected { background: #2979FF; color: white; }
            QTreeWidget { background-color: #2D2D2D; border: none; font-family: "Consolas"; font-size: 9pt; alternate-background-color: #333333; }
            QHeaderView::section { background-color: #1E1E1E; color: #E0E0E0; padding: 4px; border: none; font-weight: bold; }
            QPushButton { background-color: #2979FF; color: white; border: none; padding: 6px; border-radius: 4px; font-weight: bold; }
            QPushButton:hover { background-color: #448AFF; }
            QPushButton.Danger { background-color: #D32F2F; }
            QPushButton.Danger:hover { background-color: #EF5350; }
            QComboBox { background-color: #2D2D2D; color: white; border: 1px solid #333; padding: 5px; border-radius: 4px; }
            QComboBox::drop-down { border: none; }
        """)

        self.central_widget = QWidget()
        self.setCentralWidget(self.central_widget)
        self.main_layout = QVBoxLayout(self.central_widget)
        self.main_layout.setSpacing(15)
        self.main_layout.setContentsMargins(15, 15, 15, 15)

        self.input_widgets = {}
        self.setup_ui()
        
        # Timer for updates
        self.timer = QTimer()
        self.timer.timeout.connect(self.update_gui)
        self.timer.start(200)
        
        self.blink_timer = QTimer()
        self.blink_timer.timeout.connect(self.blink_labels)
        self.blink_timer.start(800) # Blink every 800ms

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
        msg.setStyleSheet("QMessageBox { background-color: #121212; color: white; } QPushButton { background-color: #2979FF; color: white; padding: 5px; }")
        
        if msg.exec() == QMessageBox.Yes:
            data = {
                "geometry": self.saveGeometry().toBase64().data().decode()
            }
            with open("ui_state.json", "w") as f:
                json.dump(data, f)
            super().closeEvent(event)
        else:
            event.ignore()

    def blink_labels(self):
        if self.has_active_trades:
            self.blink_state = not self.blink_state
            # Force update of styles in update_gui next cycle or trigger here
            # For simplicity, we rely on update_gui to read blink_state if we want complex logic,
            # but here we can just toggle opacity or color brightness if needed.
            # Actually, let's just let update_gui handle the styling based on blink_state.

    def setup_ui(self):
        # 1. Header
        header_frame = QFrame()
        header_frame.setProperty("class", "Panel")
        header_layout = QVBoxLayout(header_frame)
        
        top_row = QHBoxLayout()
        
        # Live Indicator & Status
        self.signal_indicator = SignalIndicator()
        self.status_label = QLabel("Connecting...")
        self.status_label.setProperty("class", "Status")
        
        top_row.addWidget(self.signal_indicator)
        top_row.addWidget(self.status_label)
        top_row.addStretch()
        
        # Counts (Small, Right aligned)
        self.lbl_scalp_count = QLabel("Scalp: 0")
        self.lbl_scalp_count.setStyleSheet("color: #757575; font-family: Consolas; font-size: 9pt;")
        top_row.addWidget(self.lbl_scalp_count)
        
        self.lbl_swing_count = QLabel("Swing: 0")
        self.lbl_swing_count.setStyleSheet("color: #757575; font-family: Consolas; font-size: 9pt;")
        top_row.addWidget(self.lbl_swing_count)
        
        header_layout.addLayout(top_row)
        
        # Big Price
        self.price_label = QLabel("-- / --")
        self.price_label.setProperty("class", "Price")
        self.price_label.setAlignment(Qt.AlignCenter)
        header_layout.addWidget(self.price_label)
        
        self.main_layout.addWidget(header_frame)

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
        
        self.main_layout.addWidget(toggles_frame)

        # 3. Collapsible Configuration
        self.config_btn = QPushButton("⚙ Configuration")
        self.config_btn.setProperty("class", "Config")
        self.config_btn.setCheckable(True)
        self.config_btn.clicked.connect(self.toggle_config)
        self.main_layout.addWidget(self.config_btn)

        self.settings_frame = QFrame()
        self.settings_frame.setProperty("class", "Panel")
        self.settings_frame.setVisible(False)
        settings_layout = QGridLayout(self.settings_frame)
        settings_layout.setColumnStretch(1, 1)
        
        self.create_input(settings_layout, "Signal Sym", "signal_symbol", 0)
        self.create_input(settings_layout, "Trade Sym", "trade_symbol", 1)
        self.create_input(settings_layout, "Scalp Lot", "fixed_lot_size", 2, is_float=True)
        self.create_input(settings_layout, "Swing Lot", "swing_lot_size", 3, is_float=True)
        self.create_input(settings_layout, "Max Entries", "max_entries", 4, is_float=False)
        
        self.adv_btn = QPushButton("🛠️ Advanced Settings")
        self.adv_btn.clicked.connect(self.open_advanced_settings)
        settings_layout.addWidget(self.adv_btn, 5, 0, 1, 2)
        
        self.main_layout.addWidget(self.settings_frame)

        # 4. Tabs
        self.tabs = QTabWidget()
        self.main_layout.addWidget(self.tabs)
        
        # Tab 2: Open Positions
        self.pos_tab = QWidget()
        pos_layout = QVBoxLayout(self.pos_tab)
        
        self.tree_pos = QTreeWidget()
        self.tree_pos.setHeaderLabels(["#", "Type", "Vol", "P/L"])
        self.tree_pos.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.tree_pos.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.tree_pos.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.tree_pos.header().setSectionResizeMode(3, QHeaderView.Stretch)
        self.tree_pos.headerItem().setTextAlignment(0, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_pos.headerItem().setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
        self.tree_pos.headerItem().setTextAlignment(2, Qt.AlignRight | Qt.AlignVCenter)
        self.tree_pos.headerItem().setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
        self.tree_pos.setAlternatingRowColors(True)
        self.tree_pos.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree_pos.customContextMenuRequested.connect(self.show_context_menu)
        pos_layout.addWidget(self.tree_pos)
        
        # Actions
        action_frame = QFrame()
        action_frame.setProperty("class", "Panel")
        action_layout = QHBoxLayout(action_frame)
        action_layout.setContentsMargins(10, 10, 10, 10)
        action_layout.setSpacing(10)
        
        self.close_combo = QComboBox()
        self.close_combo.addItems(["Close All Scalp", "Close All Swing", "Close Scalp Winners", "Close Scalp Losers"])
        self.close_combo.setFixedHeight(32)
        action_layout.addWidget(self.close_combo, 1)
        
        self.exec_btn = QPushButton("Execute")
        self.exec_btn.setProperty("class", "Danger")
        self.exec_btn.setFixedSize(100, 32)
        self.exec_btn.setCursor(Qt.PointingHandCursor)
        self.exec_btn.clicked.connect(self.execute_close_action)
        action_layout.addWidget(self.exec_btn)
        
        pos_layout.addWidget(action_frame)
        
        self.lbl_open_pl = QLabel("Open P/L: $0.00")
        self.lbl_open_pl.setAlignment(Qt.AlignRight)
        self.lbl_open_pl.setStyleSheet("font-weight: bold; font-size: 10pt;")
        pos_layout.addWidget(self.lbl_open_pl)
        
        self.tabs.addTab(self.pos_tab, "Open")

        # Tab 3: History
        self.hist_tab = QWidget()
        hist_layout = QVBoxLayout(self.hist_tab)
        
        self.tree_hist = QTreeWidget()
        self.tree_hist.setHeaderLabels(["#", "Type", "Vol", "P/L"])
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
        footer_frame.setStyleSheet("background-color: #1E1E1E; border-top: 1px solid #333; border-radius: 0px;")
        footer_layout = QHBoxLayout(footer_frame)
        footer_layout.setContentsMargins(10, 4, 10, 4)
        
        self.lbl_conn_time = QLabel("Conn: --:--")
        self.lbl_conn_time.setStyleSheet("color: #757575; font-size: 9pt;")
        
        self.lbl_latency = QLabel("Ping: -- ms")
        self.lbl_latency.setStyleSheet("color: #757575; font-size: 9pt;")
        
        footer_layout.addWidget(self.lbl_conn_time)
        footer_layout.addStretch()
        footer_layout.addWidget(self.lbl_latency)
        
        self.main_layout.addWidget(footer_frame)

    def create_input(self, layout, label, key, row, is_float=False):
        layout.addWidget(QLabel(label), row, 0)
        
        val = CONFIG[key]
        if isinstance(val, (int, float)):
            if is_float:
                widget = ModernSpinBox(val, is_float=True, step=0.01)
            else:
                widget = ModernSpinBox(val, is_float=False, step=1)
            
            # Use a closure to capture key
            def on_change(v, k=key):
                CONFIG[k] = v
            widget.valueChanged.connect(on_change)
        else:
            widget = QLineEdit(str(val))
            def on_text_change(v, k=key):
                CONFIG[k] = v
            widget.textChanged.connect(on_text_change)
            
        layout.addWidget(widget, row, 1)
        self.input_widgets[key] = widget

    def create_toggle(self, layout, label, key, row, col):
        container = QWidget()
        h_layout = QHBoxLayout(container)
        h_layout.setContentsMargins(0,0,0,0)
        
        lbl = QLabel(label)
        cb = ToggleSwitch()
        cb.setChecked(CONFIG[key])
        def on_toggle(s, k=key):
            CONFIG[k] = bool(s)
        cb.stateChanged.connect(on_toggle)
        
        h_layout.addWidget(lbl)
        h_layout.addStretch()
        h_layout.addWidget(cb)
        
        layout.addWidget(container, row, col)

    def toggle_config(self):
        self.settings_frame.setVisible(self.config_btn.isChecked())

    def open_advanced_settings(self):
        dlg = AdvancedSettingsDialog(self)
        dlg.exec()

    def show_context_menu(self, pos):
        item = self.tree_pos.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #2D2D2D; color: white; } QMenu::item:selected { background-color: #2979FF; }")
            close_action = menu.addAction("Close Position")
            action = menu.exec(self.tree_pos.mapToGlobal(pos))
            if action == close_action:
                self.close_selected(item)

    def show_log_context_menu(self, pos):
        item = self.tree_log.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #2D2D2D; color: white; } QMenu::item:selected { background-color: #2979FF; }")
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
        elif action == "Close Scalp Winners": mode = "scalp_profit"
        elif action == "Close Scalp Losers": mode = "scalp_loss"
        
        if mode: self.close_bulk(mode)

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

    def close_bulk(self, mode):
        if "loss" in mode:
            msg = QMessageBox()
            msg.setIcon(QMessageBox.Warning)
            msg.setText("Are you sure you want to close losing trades?")
            msg.setWindowTitle("Confirm Close")
            msg.setStandardButtons(QMessageBox.Yes | QMessageBox.No)
            msg.setStyleSheet("QMessageBox { background-color: #121212; color: white; } QPushButton { background-color: #2979FF; color: white; padding: 5px; }")
            if msg.exec() != QMessageBox.Yes:
                return

        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        if not positions: return
        for pos in positions:
            if pos.magic not in [CONFIG["magic_number"], CONFIG["magic_number"]+1]: continue
            
            is_scalp = pos.magic == CONFIG["magic_number"]
            is_swing = pos.magic == CONFIG["magic_number"] + 1
            
            if mode == "scalp" and is_scalp: self._send_close_request(pos)
            elif mode == "swing" and is_swing: self._send_close_request(pos)
            elif mode == "scalp_profit" and is_scalp and pos.profit > 0: self._send_close_request(pos)
            elif mode == "scalp_loss" and is_scalp and pos.profit < 0: self._send_close_request(pos)

    def update_gui(self):
        # Status
        txt = state["status_text"]
        term = mt5.terminal_info()
        
        if term and not term.trade_allowed:
            self.status_label.setText("⚠️ AutoTrading OFF")
            self.status_label.setStyleSheet("color: #FFAB00; background-color: #2D2D2D; padding: 4px 8px; border-radius: 4px;")
        else:
            self.status_label.setText(txt)
            if "Connected" in txt: self.status_label.setStyleSheet("color: #00C853; background-color: #1E1E1E; padding: 4px 8px; border-radius: 4px;")
            elif any(x in txt for x in ["Error", "Disconnected", "Failed"]): self.status_label.setStyleSheet("color: #D32F2F; background-color: #1E1E1E; padding: 4px 8px; border-radius: 4px;")
            else: self.status_label.setStyleSheet("color: #FFAB00; background-color: #1E1E1E; padding: 4px 8px; border-radius: 4px;")
        
        # Update Footer
        if state.get("connection_time"):
            self.lbl_conn_time.setText(f"Conn: {state['connection_time'].strftime('%H:%M:%S')}")
        else:
            self.lbl_conn_time.setText("Conn: --:--")
            
        if term:
            ping_ms = term.ping_last // 1000
            self.lbl_latency.setText(f"Ping: {ping_ms} ms")
            if ping_ms < 100: self.lbl_latency.setStyleSheet("color: #00C853; font-size: 9pt;")
            elif ping_ms < 300: self.lbl_latency.setStyleSheet("color: #FFAB00; font-size: 9pt;")
            else: self.lbl_latency.setStyleSheet("color: #D32F2F; font-size: 9pt;")
            
        # Price
        tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
        if tick: 
            self.price_label.setText(f"{tick.bid:.2f} / {tick.ask:.2f}")
            
            if self.last_price > 0:
                if tick.bid > self.last_price:
                    self.price_label.setStyleSheet('font-family: "Consolas"; font-weight: bold; font-size: 24pt; color: #00C853;')
                elif tick.bid < self.last_price:
                    self.price_label.setStyleSheet('font-family: "Consolas"; font-weight: bold; font-size: 24pt; color: #D32F2F;')
            
            self.last_price = tick.bid
        
        # Counts
        pos = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        sc, sw = 0, 0
        if pos:
            for p in pos:
                if p.magic == CONFIG["magic_number"]: sc += 1
                elif p.magic == CONFIG["magic_number"]+1: sw += 1
        
        self.lbl_scalp_count.setText(f"Scalp: {sc}")
        self.lbl_swing_count.setText(f"Swing: {sw}")
        self.has_active_trades = (sc > 0 or sw > 0)
        
        # Dynamic Styling with Blink
        base_style = "font-family: Consolas; font-size: 9pt; font-weight: bold;"
        dim_style = "color: #757575; font-family: Consolas; font-size: 9pt;"
        
        if sc > 0:
            color = "#00C853" if self.blink_state else "#00E676" # Blink between two shades of green
            self.lbl_scalp_count.setStyleSheet(f"color: {color}; {base_style}")
        else:
            self.lbl_scalp_count.setStyleSheet(dim_style)
            
        if sw > 0:
            color = "#2979FF" if self.blink_state else "#448AFF" # Blink between two shades of blue
            self.lbl_swing_count.setStyleSheet(f"color: {color}; {base_style}")
        else:
            self.lbl_swing_count.setStyleSheet(dim_style)
        
        # Positions Tree
        self.tree_pos.clear()
        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        total_open_pl = 0.0
        if positions:
            for pos in positions:
                if pos.magic in [CONFIG["magic_number"], CONFIG["magic_number"]+1]:
                    total_open_pl += pos.profit
                    t_type = "BUY" if pos.type == mt5.ORDER_TYPE_BUY else "SELL"
                    item = QTreeWidgetItem([str(pos.ticket), t_type, str(pos.volume), f"{pos.profit:.2f}"])
                    color = QColor("#00C853") if pos.profit >= 0 else QColor("#D32F2F")
                    item.setForeground(3, QBrush(color))
                    item.setTextAlignment(0, Qt.AlignCenter | Qt.AlignVCenter)
                    item.setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
                    item.setTextAlignment(2, Qt.AlignRight | Qt.AlignVCenter)
                    item.setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
                    self.tree_pos.addTopLevelItem(item)
        
        color_hex = "#00C853" if total_open_pl >= 0 else "#D32F2F"
        self.lbl_open_pl.setText(f"Open P/L: ${total_open_pl:.2f}")
        self.lbl_open_pl.setStyleSheet(f"font-weight: bold; font-size: 10pt; color: {color_hex};")
        
        # History Tree
        self.tree_hist.clear()
        from_d = datetime.now() - timedelta(hours=24)
        deals = mt5.history_deals_get(from_d, datetime.now())
        total_pl = 0.0
        if deals:
            for d in sorted(deals, key=lambda x: x.time, reverse=True):
                if d.symbol == CONFIG["trade_symbol"] and d.magic in [CONFIG["magic_number"], CONFIG["magic_number"]+1]:
                    if d.entry == mt5.DEAL_ENTRY_OUT or d.entry == mt5.DEAL_ENTRY_INOUT:
                        total_pl += d.profit
                        t_type = "BUY" if d.type == mt5.ORDER_TYPE_BUY else "SELL"
                        item = QTreeWidgetItem([str(d.ticket), t_type, str(d.volume), f"{d.profit:.2f}"])
                        color = QColor("#00C853") if d.profit >= 0 else QColor("#D32F2F")
                        item.setForeground(3, QBrush(color))
                        item.setTextAlignment(0, Qt.AlignCenter | Qt.AlignVCenter)
                        item.setTextAlignment(1, Qt.AlignCenter | Qt.AlignVCenter)
                        item.setTextAlignment(2, Qt.AlignRight | Qt.AlignVCenter)
                        item.setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
                        self.tree_hist.addTopLevelItem(item)
        
        acct = mt5.account_info()
        balance = acct.balance if acct else 0.0
        self.total_profit_label.setText(f"Bal: ${balance:.2f}  |  P/L (24h): ${total_pl:.2f}")
        color_hex = "#00C853" if total_pl >= 0 else "#D32F2F"
        self.total_profit_label.setStyleSheet(f"font-weight: bold; font-size: 10pt; color: {color_hex};")
        
        # Update Logs
        logs_to_add = []
        with state["lock"]:
            if state.get("gui_logs"):
                logs_to_add = state["gui_logs"][:]
                state["gui_logs"] = []
        
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

        # Sync Signal Symbol
        if "signal_symbol" in self.input_widgets:
            widget = self.input_widgets["signal_symbol"]
            if widget.text() != CONFIG["signal_symbol"]:
                widget.setText(CONFIG["signal_symbol"])
        
        # Flash Signal Indicator
        diff = time.time() - state.get("last_signal_ts", 0)
        if diff < 3.0: # Flash for 3 seconds
            # Blink rapidly (approx every 250ms)
            is_active = (int(diff * 4) % 2 == 0)
            self.signal_indicator.set_active(is_active)
        else:
            self.signal_indicator.set_active(False)