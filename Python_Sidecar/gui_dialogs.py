import os
import json
import time
from datetime import datetime, timedelta
import MetaTrader5 as mt5 # type: ignore
from PySide6.QtWidgets import (QDialog, QVBoxLayout, QScrollArea, QFrame, QWidget,  # type: ignore
                               QFormLayout, QLabel, QLineEdit, QDialogButtonBox, QComboBox,
                               QMessageBox, QGridLayout, QHBoxLayout, QPushButton, QTextEdit, QTabWidget, QApplication, QGraphicsOpacityEffect)
from PySide6.QtCore import Qt, QByteArray, QPointF, QPoint, QTimer, QPropertyAnimation, QEasingCurve # type: ignore
from PySide6.QtGui import QColor, QPainter, QBrush, QPen, QPolygonF # type: ignore
from config import CONFIG, DEFAULT_CONFIG, state, save_config
from gui_styles import GLOBAL_STYLESHEET
from gui_widgets import ModernSpinBox, ToggleSwitch

class ModernToast(QWidget):
    _active_toasts = []

    def __init__(self, parent, text, duration=2500, style="success"):
        super().__init__(parent)
        self.setWindowFlags(Qt.FramelessWindowHint | Qt.SubWindow)
        self.setAttribute(Qt.WA_TransparentForMouseEvents)
        self.setAttribute(Qt.WA_DeleteOnClose)
        self.setAttribute(Qt.WA_StyledBackground, True)
        
        # Colors & Icon
        if style == "success":
            bg, fg, icon = "#81C995", "#000000", "✓"
        elif style == "error":
            bg, fg, icon = "#F28B82", "#000000", "✕"
        else:
            bg, fg, icon = "#FDD663", "#000000", "!"

        self.setStyleSheet(f"""
            QWidget {{
                background-color: {bg};
                color: {fg};
                border-radius: 6px;
                border: 1px solid {fg}40;
            }}
            QLabel {{
                background-color: transparent;
                color: {fg};
                font-weight: bold;
                font-size: 13px;
                padding: 4px;
            }}
        """)
        
        layout = QHBoxLayout(self)
        layout.setContentsMargins(12, 8, 12, 8)
        layout.setSpacing(8)
        
        lbl_icon = QLabel(icon)
        lbl_text = QLabel(text)
        layout.addWidget(lbl_icon)
        layout.addWidget(lbl_text)
        
        self.adjustSize()
        
        # Animation Setup
        self.opacity_effect = QGraphicsOpacityEffect(self)
        self.setGraphicsEffect(self.opacity_effect)
        
        self.anim_opacity = QPropertyAnimation(self.opacity_effect, b"opacity")
        self.anim_opacity.setDuration(300)
        self.anim_opacity.setStartValue(0.0)
        self.anim_opacity.setEndValue(1.0)
        self.anim_opacity.setEasingCurve(QEasingCurve.OutCubic)
        self.anim_opacity.start()
        
        # Stack Management
        ModernToast._active_toasts.append(self)
        self.update_stack()
        
        # Slide Animation
        final_pos = self.pos()
        start_pos = QPoint(final_pos.x(), final_pos.y() + 20)
        self.move(start_pos)
        
        self.anim_pos = QPropertyAnimation(self, b"pos")
        self.anim_pos.setDuration(300)
        self.anim_pos.setStartValue(start_pos)
        self.anim_pos.setEndValue(final_pos)
        self.anim_pos.setEasingCurve(QEasingCurve.OutCubic)
        self.anim_pos.start()
            
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.fade_out)
        self.timer.start(duration)
        
        self.show()
        self.raise_()

    def closeEvent(self, event):
        if self in ModernToast._active_toasts:
            ModernToast._active_toasts.remove(self)
            self.update_stack()
        super().closeEvent(event)

    def update_stack(self):
        parent = self.parent()
        if not parent: return
        
        # Filter toasts for this parent
        my_toasts = [t for t in ModernToast._active_toasts if t.parent() == parent]
        
        base_y = parent.rect().height() - 50
        spacing = 10
        
        # Iterate backwards (newest at bottom)
        for toast in reversed(my_toasts):
            x = (parent.rect().width() - toast.width()) // 2
            y = base_y - toast.height()
            toast.move(x, y)
            base_y = y - spacing

    def fade_out(self):
        self.anim_opacity.setDirection(QPropertyAnimation.Backward)
        self.anim_opacity.finished.connect(self.close)
        self.anim_opacity.start()

    @staticmethod
    def show_message(parent, text, duration=2500, style="success"):
        if parent:
            ModernToast(parent, text, duration, style)

class AdvancedSettingsDialog(QDialog):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Configuration")
        self.resize(500, 550)
        # Stylesheet inherited from parent via global app style
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(10, 10, 10, 10)
        layout.setSpacing(10)
        
        self.tabs = QTabWidget()
        self.tabs.setStyleSheet("""
            QTabWidget::pane { border: 1px solid #444746; background: #1E1F20; border-radius: 4px; }
            QTabBar::tab { background: #131314; color: #C4C7C5; padding: 8px 20px; border-top-left-radius: 4px; border-top-right-radius: 4px; margin-right: 2px; }
            QTabBar::tab:selected { background: #A8C7FA; color: #000000; font-weight: bold; }
        """)
        layout.addWidget(self.tabs)
        
        self.inputs = {}
        
        # --- Tab 1: General ---
        self.tab_general, self.layout_general = self.create_scrollable_tab()
        
        self.add_input(self.layout_general, "Signal Symbol", "signal_symbol", str, "Symbol to listen for signals.")
        self.add_input(self.layout_general, "Trade Symbol", "trade_symbol", str, "Symbol to execute trades on.")
        self.add_separator(self.layout_general)
        self.add_input(self.layout_general, "Scalp Lot Size", "fixed_lot_size", float, "Lot size for Scalp trades.")
        self.add_input(self.layout_general, "Swing Lot Size", "swing_lot_size", float, "Lot size for Swing trades.")
        self.add_separator(self.layout_general)
        self.add_input(self.layout_general, "Max Entries", "max_entries", int, "Maximum number of entries per signal.")
        self.add_input(self.layout_general, "Min Conviction (%)", "min_conviction", float, "Minimum conviction score required to execute.")
        
        self.tabs.addTab(self.tab_general, "General")
        
        # --- Tab 2: Trailing ---
        self.tab_trailing, self.layout_trailing = self.create_scrollable_tab()
        
        self.add_header(self.layout_trailing, "Scalp Settings")
        self.add_input(self.layout_trailing, "Start (pips)", "trailing_start_pips_scalp", float, "Profit in pips required to activate trailing stop.")
        self.add_input(self.layout_trailing, "Dist (pips)", "trailing_dist_pips_scalp", float, "Distance in pips (Overridden if ATR Trailing is enabled).")
        self.add_input(self.layout_trailing, "Step (pips)", "trailing_step_pips_scalp", float, "Minimum price movement in pips to update stop loss.")
        
        self.add_separator(self.layout_trailing)
        self.add_header(self.layout_trailing, "Swing Settings")
        self.add_input(self.layout_trailing, "Start (pips)", "trailing_start_pips_swing", float, "Profit in pips required to activate trailing stop.")
        self.add_input(self.layout_trailing, "Dist (pips)", "trailing_dist_pips_swing", float, "Distance in pips (Overridden if ATR Trailing is enabled).")
        self.add_input(self.layout_trailing, "Step (pips)", "trailing_step_pips_swing", float, "Minimum price movement in pips to update stop loss.")
        
        self.tabs.addTab(self.tab_trailing, "Trailing")
        
        # --- Tab 3: ATR & Risk ---
        self.tab_atr, self.layout_atr = self.create_scrollable_tab()
        
        self.add_bool(self.layout_atr, "Use ATR Trailing", "use_atr_trailing", "Use Server ATR for trailing distance instead of fixed pips.")
        self.add_combo(self.layout_atr, "ATR TF (Scalp)", "atr_timeframe_scalp", ["M1", "M5", "M15", "M30", "H1", "H4", "D1"], "Timeframe for Scalp ATR.")
        
        self.add_combo(self.layout_atr, "ATR TF (Swing)", "atr_timeframe_swing", ["M1", "M5", "M15", "M30", "H1", "H4", "D1"], "Timeframe for Swing ATR.")
        
        self.add_separator(self.layout_atr)
        self.add_combo(self.layout_atr, "Gauge Timeframe", "atr_timeframe", ["M1", "M5", "M15", "M30", "H1", "H4", "D1"], "Timeframe used for Dashboard Gauge.")
        self.add_separator(self.layout_atr)
        self.add_input(self.layout_atr, "High Vol Threshold", "atr_high_vol_threshold", float, "Base ATR threshold (M5). Scales with timeframe.")
        self.add_input(self.layout_atr, "Chart R:R Ratio", "chart_rr_ratio", float, "Default Risk:Reward ratio for the Chart R/R tool.")
        
        self.tabs.addTab(self.tab_atr, "ATR / Risk")
        
        # --- Tab 4: Management ---
        self.tab_mgmt, self.layout_mgmt = self.create_scrollable_tab()
        
        self.add_bool(self.layout_mgmt, "Use Stagnation", "use_stagnation", "Enable partial closing of trades that stall.")
        self.add_input(self.layout_mgmt, "Stag. Sec", "stagnation_sec", int, "Seconds before a trade is considered stagnant.")
        self.add_input(self.layout_mgmt, "Time Mult", "stag_time_mult", float, "Multiplier for stagnation time on Swing trades.")
        self.add_bool(self.layout_mgmt, "Netting: Close Manual", "allow_manual_closure_on_netting", "Allow bot to close opposite Manual trades on Netting accounts.")
        
        self.tabs.addTab(self.tab_mgmt, "Management")
        
        # --- Tab 5: Sessions ---
        self.tab_sessions, self.layout_sessions = self.create_scrollable_tab()
        
        self.add_header(self.layout_sessions, "Scalp Sessions")
        self.add_bool(self.layout_sessions, "Sydney (21:00-06:00 UTC)", "session_scalp_syd")
        self.add_bool(self.layout_sessions, "Tokyo (00:00-09:00 UTC)", "session_scalp_tok")
        self.add_bool(self.layout_sessions, "London (08:00-17:00 UTC)", "session_scalp_lon")
        self.add_bool(self.layout_sessions, "New York (13:00-22:00 UTC)", "session_scalp_ny")
        self.add_bool(self.layout_sessions, "Overlap: TOK/LON (08:00-09:00 UTC)", "session_scalp_overlap_tok_lon")
        self.add_bool(self.layout_sessions, "Overlap: LON/NY (13:00-17:00 UTC)", "session_scalp_overlap_lon_ny")
        
        self.add_separator(self.layout_sessions)
        self.add_header(self.layout_sessions, "Swing Sessions")
        self.add_bool(self.layout_sessions, "Sydney (21:00-06:00 UTC)", "session_swing_syd")
        self.add_bool(self.layout_sessions, "Tokyo (00:00-09:00 UTC)", "session_swing_tok")
        self.add_bool(self.layout_sessions, "London (08:00-17:00 UTC)", "session_swing_lon")
        self.add_bool(self.layout_sessions, "New York (13:00-22:00 UTC)", "session_swing_ny")
        self.add_bool(self.layout_sessions, "Overlap: TOK/LON (08:00-09:00 UTC)", "session_swing_overlap_tok_lon")
        self.add_bool(self.layout_sessions, "Overlap: LON/NY (13:00-17:00 UTC)", "session_swing_overlap_lon_ny")
        
        self.add_separator(self.layout_sessions)
        self.add_header(self.layout_sessions, "Auto-Close at Session End")
        self.add_bool(self.layout_sessions, "Close Scalp at Sydney End (06:00 UTC)", "close_scalp_syd_end")
        self.add_bool(self.layout_sessions, "Close Scalp at Tokyo End (09:00 UTC)", "close_scalp_tok_end")
        self.add_bool(self.layout_sessions, "Close Scalp at London End (17:00 UTC)", "close_scalp_lon_end")
        self.add_bool(self.layout_sessions, "Close Scalp at NY End (22:00 UTC)", "close_scalp_ny_end")
        
        self.add_bool(self.layout_sessions, "Close Swing at Sydney End (06:00 UTC)", "close_swing_syd_end")
        self.add_bool(self.layout_sessions, "Close Swing at Tokyo End (09:00 UTC)", "close_swing_tok_end")
        self.add_bool(self.layout_sessions, "Close Swing at London End (17:00 UTC)", "close_swing_lon_end")
        self.add_bool(self.layout_sessions, "Close Swing at NY End (22:00 UTC)", "close_swing_ny_end")
        
        self.tabs.addTab(self.tab_sessions, "Sessions")
        
        btns = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        self.btn_reset = btns.addButton("Reset to Defaults", QDialogButtonBox.ResetRole)
        self.btn_reset.setCursor(Qt.PointingHandCursor)
        self.btn_reset.setToolTip("Restore all settings to their original default values.")
        self.btn_reset.clicked.connect(self.reset_defaults)
        
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        layout.addWidget(btns)

        self.load_geometry()

    def create_scrollable_tab(self):
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.NoFrame)
        scroll.setStyleSheet("QScrollArea { background: transparent; }")
        
        content = QWidget()
        content.setStyleSheet("background: transparent;")
        form = QFormLayout(content)
        form.setSpacing(15)
        form.setContentsMargins(20, 20, 20, 20)
        form.setLabelAlignment(Qt.AlignLeft)
        
        scroll.setWidget(content)
        layout.addWidget(scroll)
        
        return tab, form

    def add_header(self, layout, text):
        lbl = QLabel(text)
        lbl.setStyleSheet("font-weight: bold; color: #A8C7FA; font-size: 10pt; margin-top: 5px;")
        layout.addRow(lbl)

    def add_separator(self, layout):
        line = QFrame()
        line.setFrameShape(QFrame.HLine)
        line.setFrameShadow(QFrame.Sunken)
        line.setStyleSheet("background-color: #444746; margin-top: 5px; margin-bottom: 5px;")
        layout.addRow(line)

    def add_input(self, layout, label, key, dtype, tooltip=None):
        val = CONFIG.get(key, "" if dtype == str else 0)
        
        if dtype == str:
            widget = QLineEdit(str(val))
        else:
            step = 0.1 if dtype == float and ("mult" in key or "pct" in key or "ratio" in key) else 1.0
            if "lot" in key: step = 0.01
            widget = ModernSpinBox(val, is_float=(dtype == float), step=step)
            
        if tooltip:
            widget.setToolTip(tooltip)
        
        lbl = QLabel(label)
        if tooltip:
            lbl.setToolTip(tooltip)
        layout.addRow(lbl, widget)
        self.inputs[key] = (widget, dtype)

    def add_bool(self, layout, label, key, tooltip=None):
        val = CONFIG.get(key, False)
        widget = ToggleSwitch()
        widget.setChecked(val)
        if tooltip:
            widget.setToolTip(tooltip)
        
        lbl = QLabel(label)
        if tooltip:
            lbl.setToolTip(tooltip)
        layout.addRow(lbl, widget)
        self.inputs[key] = (widget, bool)

    def add_combo(self, layout, label, key, items, tooltip=None):
        val = CONFIG.get(key, items[0])
        widget = QComboBox()
        widget.addItems(items)
        widget.setCurrentText(str(val))
        if tooltip:
            widget.setToolTip(tooltip)
        
        lbl = QLabel(label)
        if tooltip:
            lbl.setToolTip(tooltip)
        layout.addRow(lbl, widget)
        self.inputs[key] = (widget, list)

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
                    elif dtype == list:
                        widget.setCurrentText(str(val))
                    elif dtype == str:
                        widget.setText(str(val))
                    else:
                        widget.setValue(val)
            ModernToast.show_message(self, "Settings restored to defaults", style="success")

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
                elif dtype == list:
                    CONFIG[key] = widget.currentText()
                elif dtype == str:
                    CONFIG[key] = widget.text()
                else:
                    CONFIG[key] = widget.value()
            save_config()
            super().accept()

    def load_geometry(self):
        try:
            if os.path.exists("settings_ui_state.json"):
                with open("settings_ui_state.json", "r") as f:
                    data = json.load(f)
                    geom = QByteArray.fromBase64(data.get("geometry", "").encode())
                    self.restoreGeometry(geom)
        except Exception:
            pass

    def closeEvent(self, event):
        data = {"geometry": self.saveGeometry().toBase64().data().decode()}
        try:
            with open("settings_ui_state.json", "w") as f:
                json.dump(data, f)
        except Exception:
            pass
        super().closeEvent(event)

class ManualExecutionDialog(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumWidth(320)
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        layout = QVBoxLayout(self)
        layout.setSpacing(10)
        layout.setContentsMargins(10, 10, 10, 10)
        
        # --- SETTINGS SECTION ---
        settings_frame = QFrame()
        settings_frame.setProperty("class", "Panel")
        settings_layout = QVBoxLayout(settings_frame)
        settings_layout.setSpacing(10)
        
        # Row 1: Volume & Count
        row1 = QHBoxLayout()
        
        vol_container = QWidget()
        vol_layout = QVBoxLayout(vol_container)
        vol_layout.setContentsMargins(0,0,0,0)
        vol_layout.setSpacing(2)
        vol_layout.addWidget(QLabel("Volume"))
        self.spin_man_vol = ModernSpinBox(CONFIG["fixed_lot_size"], is_float=True, step=0.01)
        self.spin_man_vol.input.setRange(0.01, 100.0)
        vol_layout.addWidget(self.spin_man_vol)
        
        count_container = QWidget()
        count_layout = QVBoxLayout(count_container)
        count_layout.setContentsMargins(0,0,0,0)
        count_layout.setSpacing(2)
        count_layout.addWidget(QLabel("Count"))
        self.spin_man_count = ModernSpinBox(1, is_float=False, step=1)
        self.spin_man_count.input.setRange(1, 100)
        count_layout.addWidget(self.spin_man_count)
        
        row1.addWidget(vol_container)
        row1.addWidget(count_container)
        settings_layout.addLayout(row1)
        
        # Row 1.5: Risk Calculator
        row_risk = QHBoxLayout()
        
        rr_container = QWidget()
        rr_layout = QVBoxLayout(rr_container)
        rr_layout.setContentsMargins(0,0,0,0)
        rr_layout.setSpacing(2)
        rr_layout.addWidget(QLabel("Reward Ratio"))
        self.spin_rr = ModernSpinBox(CONFIG.get("chart_rr_ratio", 1.5), is_float=True, step=0.1)
        self.spin_rr.input.setRange(0.1, 20.0)
        rr_layout.addWidget(self.spin_rr)
        
        calc_container = QWidget()
        calc_layout = QVBoxLayout(calc_container)
        calc_layout.setContentsMargins(0,0,0,0)
        calc_layout.setSpacing(2)
        calc_layout.addWidget(QLabel("Auto-Fill TP"))
        
        calc_btns = QHBoxLayout()
        calc_btns.setSpacing(4)
        self.btn_calc_long = QPushButton("Long")
        self.btn_calc_long.setCursor(Qt.PointingHandCursor)
        self.btn_calc_long.setToolTip("Calculate SL/TP for Long position")
        self.btn_calc_long.setStyleSheet("background-color: #81C995; color: #000000; font-weight: bold; padding: 2px;")
        self.btn_calc_long.clicked.connect(lambda: self.calc_sltp("long"))
        
        self.btn_calc_short = QPushButton("Short")
        self.btn_calc_short.setCursor(Qt.PointingHandCursor)
        self.btn_calc_short.setToolTip("Calculate SL/TP for Short position")
        self.btn_calc_short.setStyleSheet("background-color: #F28B82; color: #000000; font-weight: bold; padding: 2px;")
        self.btn_calc_short.clicked.connect(lambda: self.calc_sltp("short"))
        
        calc_btns.addWidget(self.btn_calc_long)
        calc_btns.addWidget(self.btn_calc_short)
        calc_layout.addLayout(calc_btns)
        
        row_risk.addWidget(rr_container)
        row_risk.addWidget(calc_container)
        
        settings_layout.addLayout(row_risk)
        
        # Row 2: SL & TP
        row2 = QHBoxLayout()
        
        sl_container = QWidget()
        sl_layout = QVBoxLayout(sl_container)
        sl_layout.setContentsMargins(0,0,0,0)
        sl_layout.setSpacing(2)
        sl_layout.addWidget(QLabel("Stop Loss"))
        self.spin_man_sl = ModernSpinBox(0, is_float=True, step=1.0)
        self.spin_man_sl.input.setRange(0, 1000000)
        sl_layout.addWidget(self.spin_man_sl)
        
        tp_container = QWidget()
        tp_layout = QVBoxLayout(tp_container)
        tp_layout.setContentsMargins(0,0,0,0)
        tp_layout.setSpacing(2)
        tp_layout.addWidget(QLabel("Take Profit"))
        self.spin_man_tp = ModernSpinBox(0, is_float=True, step=1.0)
        self.spin_man_tp.input.setRange(0, 1000000)
        tp_layout.addWidget(self.spin_man_tp)
        
        row2.addWidget(sl_container)
        row2.addWidget(tp_container)
        settings_layout.addLayout(row2)
        
        # Row 3: Price (Pending)
        price_container = QWidget()
        price_layout = QVBoxLayout(price_container)
        price_layout.setContentsMargins(0,0,0,0)
        price_layout.setSpacing(2)
        price_layout.addWidget(QLabel("Entry Price (Pending Only)"))
        
        p_inner = QHBoxLayout()
        self.spin_man_price = ModernSpinBox(0, is_float=True, step=0.1)
        self.spin_man_price.input.setRange(0, 1000000)
        self.spin_man_price.setToolTip("Entry Price. Required for Pending Orders. Leave 0 for Market execution.")
        
        self.btn_copy_price = QPushButton("📍")
        self.btn_copy_price.setFixedSize(32, 32)
        self.btn_copy_price.setCursor(Qt.PointingHandCursor)
        self.btn_copy_price.setToolTip("Copy Current Price")
        self.btn_copy_price.clicked.connect(self.copy_current_price)
        
        p_inner.addWidget(self.spin_man_price)
        p_inner.addWidget(self.btn_copy_price)
        price_layout.addLayout(p_inner)
        
        settings_layout.addWidget(price_container)
        
        layout.addWidget(settings_frame)
        
        # --- MARKET EXECUTION ---
        mkt_frame = QFrame()
        mkt_frame.setProperty("class", "Panel")
        mkt_layout = QVBoxLayout(mkt_frame)
        mkt_layout.setSpacing(10)
        
        mkt_label = QLabel("Market Execution")
        mkt_label.setAlignment(Qt.AlignCenter)
        mkt_label.setStyleSheet("font-weight: bold; color: #A8C7FA; margin-top: 5px;")
        mkt_layout.addWidget(mkt_label)
        
        btn_layout = QHBoxLayout()
        btn_layout.setSpacing(10)
        
        self.btn_buy = QPushButton("BUY")
        self.btn_buy.setProperty("class", "Success")
        self.btn_buy.setFixedHeight(45)
        self.btn_buy.setStyleSheet("font-size: 14pt; font-weight: 900; border-radius: 6px;")
        self.btn_buy.setCursor(Qt.PointingHandCursor)
        self.btn_buy.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_BUY))
        btn_layout.addWidget(self.btn_buy)
        
        self.btn_sell = QPushButton("SELL")
        self.btn_sell.setProperty("class", "Danger")
        self.btn_sell.setFixedHeight(45)
        self.btn_sell.setStyleSheet("font-size: 14pt; font-weight: 900; border-radius: 6px;")
        self.btn_sell.setCursor(Qt.PointingHandCursor)
        self.btn_sell.clicked.connect(lambda: self.execute_manual_trade(mt5.ORDER_TYPE_SELL))
        btn_layout.addWidget(self.btn_sell)
        
        mkt_layout.addLayout(btn_layout)
        layout.addWidget(mkt_frame)
        
        # --- PENDING EXECUTION ---
        pend_frame = QFrame()
        pend_frame.setProperty("class", "Panel")
        pend_layout = QVBoxLayout(pend_frame)
        pend_layout.setSpacing(10)
        
        pend_label = QLabel("Pending Orders")
        pend_label.setAlignment(Qt.AlignCenter)
        pend_label.setStyleSheet("font-weight: bold; color: #FDD663; margin-top: 10px;")
        pend_layout.addWidget(pend_label)
        
        pending_layout = QGridLayout()
        pending_layout.setSpacing(8)
        
        # Helper to create styled pending buttons
        def make_pend_btn(text, slot):
            b = QPushButton(text)
            b.setFixedHeight(30)
            b.setCursor(Qt.PointingHandCursor)
            b.setStyleSheet("background-color: #1E1F20; border: 1px solid #444746;")
            b.clicked.connect(slot)
            return b

        self.btn_buy_limit = make_pend_btn("Buy Limit", lambda: self.execute_manual_trade(mt5.ORDER_TYPE_BUY_LIMIT))
        self.btn_sell_limit = make_pend_btn("Sell Limit", lambda: self.execute_manual_trade(mt5.ORDER_TYPE_SELL_LIMIT))
        self.btn_buy_stop = make_pend_btn("Buy Stop", lambda: self.execute_manual_trade(mt5.ORDER_TYPE_BUY_STOP))
        self.btn_sell_stop = make_pend_btn("Sell Stop", lambda: self.execute_manual_trade(mt5.ORDER_TYPE_SELL_STOP))
        
        pending_layout.addWidget(self.btn_buy_limit, 0, 0)
        pending_layout.addWidget(self.btn_sell_limit, 0, 1)
        pending_layout.addWidget(self.btn_buy_stop, 1, 0)
        pending_layout.addWidget(self.btn_sell_stop, 1, 1)
        
        pend_layout.addLayout(pending_layout)
        layout.addWidget(pend_frame)
        layout.addStretch()

    def copy_current_price(self):
        tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
        if tick:
            self.spin_man_price.setValue(tick.bid)
            ModernToast.show_message(self, f"Price Copied: {tick.bid}", style="info")

    def calc_sltp(self, direction):
        symbol = CONFIG["trade_symbol"]
        tick = mt5.symbol_info_tick(symbol)
        if not tick: return
        
        sym_info = mt5.symbol_info(symbol)
        if not sym_info: return
        
        # Determine Entry Price (User input or Current Market)
        entry_price = self.spin_man_price.value()
        if entry_price <= 0:
            entry_price = tick.ask if direction == "long" else tick.bid
            
        rr = self.spin_rr.value()
        
        # Calculate distance from SL
        sl_price = self.spin_man_sl.value()
        if sl_price <= 0:
            ModernToast.show_message(self, "Set SL first to calc TP", style="error")
            return
            
        dist_price = abs(entry_price - sl_price)
        
        if direction == "long":
            # sl = entry_price - dist_price
            tp = entry_price + (dist_price * rr)
        else:
            # sl = entry_price + dist_price
            tp = entry_price - (dist_price * rr)
            
        self.spin_man_tp.setValue(round(tp, sym_info.digits))
        ModernToast.show_message(self, f"{direction.title()} TP Calculated", style="success")

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
                ModernToast.show_message(self, f"Executed: {t_str} {vol}", style="success")
            else:
                err = res.comment if res else "Unknown"
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": "-",
                    "type": "Exec Fail",
                    "details": err
                })
                ModernToast.show_message(self, f"Failed: {err}", style="error")

    def setup_from_signal(self, signal):
        self.spin_man_price.setValue(signal.get("entryPrice", 0.0))
        self.spin_man_sl.setValue(signal.get("slPrice", 0.0))
        self.spin_man_tp.setValue(signal.get("tp1Price", 0.0))
        
        vol = signal.get("suggestedPositionSize", 0.0)
        if vol > 0:
            self.spin_man_vol.setValue(vol)

class PositionModifyDialog(QDialog):
    def __init__(self, ticket, symbol, sl, tp, parent=None):
        super().__init__(parent)
        self.setWindowTitle(f"Modify Position #{ticket}")
        self.setWindowFlags(Qt.Window)
        self.resize(300, 200)
        
        self.ticket = int(ticket)
        self.symbol = symbol
        
        layout = QVBoxLayout(self)
        
        form = QFormLayout()
        
        self.spin_sl = ModernSpinBox(sl, is_float=True, step=1.0)
        self.spin_sl.input.setRange(0, 999999)
        self.spin_sl.setToolTip("Stop Loss Price")
        
        self.spin_tp = ModernSpinBox(tp, is_float=True, step=1.0)
        self.spin_tp.input.setRange(0, 999999)
        self.spin_tp.setToolTip("Take Profit Price")
        
        form.addRow("Stop Loss:", self.spin_sl)
        form.addRow("Take Profit:", self.spin_tp)
        
        layout.addLayout(form)
        
        btns = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        btns.accepted.connect(self.execute_modification)
        btns.rejected.connect(self.reject)
        layout.addWidget(btns)
        
        # Add Close Button
        self.btn_close = QPushButton("Close Position")
        self.btn_close.setProperty("class", "Danger")
        self.btn_close.clicked.connect(self.close_position)
        layout.addWidget(self.btn_close)

    def execute_modification(self):
        sl = self.spin_sl.value()
        tp = self.spin_tp.value()
        
        req = {
            "action": mt5.TRADE_ACTION_SLTP,
            "position": self.ticket,
            "symbol": self.symbol,
            "sl": float(sl),
            "tp": float(tp)
        }
        
        res = mt5.order_send(req)
        if res.retcode == mt5.TRADE_RETCODE_DONE:
            ModernToast.show_message(self, "Position Modified", style="success")
            QTimer.singleShot(800, self.accept)
        else:
            ModernToast.show_message(self, f"Error: {res.comment}", style="error")

    def close_position(self):
        tick = mt5.symbol_info_tick(self.symbol)
        if not tick:
            QMessageBox.warning(self, "Error", "Could not get current price.")
            return
            
        positions = mt5.positions_get(ticket=self.ticket)
        if not positions:
            QMessageBox.warning(self, "Error", "Position not found.")
            return
            
        pos = positions[0]
        price = tick.ask if pos.type == mt5.ORDER_TYPE_SELL else tick.bid
        type_close = mt5.ORDER_TYPE_BUY if pos.type == mt5.ORDER_TYPE_SELL else mt5.ORDER_TYPE_SELL
        
        req = {
            "action": mt5.TRADE_ACTION_DEAL,
            "position": self.ticket,
            "symbol": self.symbol,
            "volume": pos.volume,
            "type": type_close,
            "price": price,
            "magic": pos.magic,
            "comment": "Manual Close"
        }
        
        res = mt5.order_send(req)
        if res.retcode == mt5.TRADE_RETCODE_DONE:
            ModernToast.show_message(self, "Position Closed", style="success")
            QTimer.singleShot(800, self.accept)
        else:
            ModernToast.show_message(self, f"Error: {res.comment}", style="error")

class CalculatorDialog(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumHeight(200)
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        layout = QVBoxLayout(self)
        layout.setSpacing(15)
        layout.setContentsMargins(10, 10, 10, 10)
        
        # Input Frame
        input_frame = QFrame()
        input_frame.setProperty("class", "Panel")
        input_layout = QVBoxLayout(input_frame)
        input_layout.setSpacing(15)
        
        # Inputs
        form = QFormLayout()
        form.setSpacing(10)
        
        self.spin_lots = ModernSpinBox(CONFIG["fixed_lot_size"], is_float=True, step=0.01)
        self.spin_tp = ModernSpinBox(50.0, is_float=True, step=1.0)
        self.spin_sl = ModernSpinBox(30.0, is_float=True, step=1.0)
        
        form.addRow("Lot Size:", self.spin_lots)
        form.addRow("TP (Pips):", self.spin_tp)
        form.addRow("SL (Pips):", self.spin_sl)
        
        input_layout.addLayout(form)
        
        # Calculate Button
        self.btn_calc = QPushButton("Calculate")
        self.btn_calc.setProperty("class", "Success")
        self.btn_calc.setCursor(Qt.PointingHandCursor)
        self.btn_calc.clicked.connect(self.calculate)
        input_layout.addWidget(self.btn_calc)
        layout.addWidget(input_frame)
        
        # Results
        res_frame = QFrame()
        res_frame.setProperty("class", "Panel")
        res_layout = QVBoxLayout(res_frame)
        res_layout.setSpacing(5)
        
        self.lbl_profit = QLabel("Potential Profit: $0.00")
        self.lbl_profit.setStyleSheet("color: #81C995; font-weight: bold; font-size: 11pt;")
        self.lbl_profit.setAlignment(Qt.AlignCenter)
        
        self.lbl_loss = QLabel("Potential Loss: $0.00")
        self.lbl_loss.setStyleSheet("color: #F28B82; font-weight: bold; font-size: 11pt;")
        self.lbl_loss.setAlignment(Qt.AlignCenter)
        
        res_layout.addWidget(self.lbl_profit)
        res_layout.addWidget(self.lbl_loss)
        
        layout.addWidget(res_frame)
        layout.addStretch()
        
    def calculate(self):
        symbol = CONFIG["trade_symbol"]
        info = mt5.symbol_info(symbol)
        if not info:
            self.lbl_profit.setText("Error: Symbol info unavailable")
            self.lbl_loss.setText("")
            return
            
        lots = self.spin_lots.value()
        tp_pips = self.spin_tp.value()
        sl_pips = self.spin_sl.value()
        
        # Determine Pip Size logic
        point = info.point
        pip_size = 10 * point
        if "XAU" in symbol.upper() or "GOLD" in symbol.upper():
            pip_size = 0.1
        elif "JPY" in symbol.upper() and point > 0.001:
             pip_size = 0.01
             
        tick_size = info.trade_tick_size
        tick_value = info.trade_tick_value
        
        if tick_size == 0: 
            self.lbl_profit.setText("Error: Tick size 0")
            return

        # Profit = (Distance / TickSize) * TickValue * Volume
        
        # TP
        tp_dist = tp_pips * pip_size
        tp_val = (tp_dist / tick_size) * tick_value * lots
        
        # SL
        sl_dist = sl_pips * pip_size
        sl_val = (sl_dist / tick_size) * tick_value * lots
        
        self.lbl_profit.setText(f"Potential Profit: ${tp_val:.2f}")
        self.lbl_loss.setText(f"Potential Loss: -${sl_val:.2f}")

class SignalMiniChart(QWidget):
    def __init__(self, signal_data, parent=None):
        super().__init__(parent)
        self.setMinimumHeight(200)
        self.signal = signal_data
        self.candles = []
        
        # Zoom/Pan
        self.visible_count = 40
        self.scroll_offset = 0
        self.is_dragging = False
        self.last_drag_x = 0
        
        # Zoom Buttons (Overlay)
        self.btn_zoom_in = QPushButton("+", self)
        self.btn_zoom_in.setFixedSize(24, 24)
        self.btn_zoom_in.setCursor(Qt.PointingHandCursor)
        self.btn_zoom_in.setStyleSheet("background-color: rgba(46, 52, 64, 180); color: #E3E3E3; border: 1px solid #444746; border-radius: 4px; font-weight: bold;")
        self.btn_zoom_in.clicked.connect(self.zoom_in)
        self.btn_zoom_in.show()

        self.btn_zoom_out = QPushButton("-", self)
        self.btn_zoom_out.setFixedSize(24, 24)
        self.btn_zoom_out.setCursor(Qt.PointingHandCursor)
        self.btn_zoom_out.setStyleSheet("background-color: rgba(46, 52, 64, 180); color: #E3E3E3; border: 1px solid #444746; border-radius: 4px; font-weight: bold;")
        self.btn_zoom_out.clicked.connect(self.zoom_out)
        self.btn_zoom_out.show()
        
        QTimer.singleShot(50, self.fetch_data)

    def resizeEvent(self, event):
        # Position buttons top-right
        self.btn_zoom_out.move(self.width() - 30, 5)
        self.btn_zoom_in.move(self.width() - 60, 5)
        super().resizeEvent(event)

    def fetch_data(self):
        try:
            # Use local trade symbol to ensure we match Market Watch
            symbol = CONFIG.get("trade_symbol", self.signal.get("symbol", ""))
            ts = float(self.signal.get("createdAt", 0))
            classification = str(self.signal.get("classification", "scalp")).lower()
            
            if not symbol or ts <= 0: return
            
            # Determine timeframe based on classification
            if "scalp" in classification:
                timeframe = mt5.TIMEFRAME_M1
                interval_sec = 60
            elif "swing" in classification:
                timeframe = mt5.TIMEFRAME_H1
                interval_sec = 3600
            else:
                timeframe = mt5.TIMEFRAME_M5
                interval_sec = 300

            # Ensure MT5 connection
            if not mt5.terminal_info():
                mt5.initialize()
            
            # Ensure symbol is selected
            if not mt5.symbol_info(symbol):
                mt5.symbol_select(symbol, True)
            
            t_future = ts + (10 * interval_sec) # +10 candles
            date_from = datetime.fromtimestamp(t_future)
            
            rates = mt5.copy_rates_from(symbol, timeframe, date_from, 60)
            
            if rates is not None and len(rates) > 0:
                self.candles = rates
                self.update()
            else:
                # Fallback: Try selecting symbol and retry
                mt5.symbol_select(symbol, True)
                rates = mt5.copy_rates_from(symbol, timeframe, date_from, 60)
                if rates is not None and len(rates) > 0:
                    self.candles = rates
                    self.update()
        except Exception as e:
            print(f"MiniChart Error: {e}")

    def mousePressEvent(self, event):
        if event.button() == Qt.LeftButton:
            self.is_dragging = True
            self.last_drag_x = event.x()
            self.setCursor(Qt.ClosedHandCursor)

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.LeftButton:
            self.is_dragging = False
            self.setCursor(Qt.ArrowCursor)

    def mouseMoveEvent(self, event):
        if self.is_dragging:
            dx = event.x() - self.last_drag_x
            w = self.width()
            if self.visible_count > 0 and w > 0:
                candle_w = w / self.visible_count
                shift = int(dx / (candle_w if candle_w > 1 else 1))
                if shift != 0:
                    self.scroll_offset += shift
                    max_off = max(0, len(self.candles) - 5)
                    if self.scroll_offset < 0: self.scroll_offset = 0
                    if self.scroll_offset > max_off: self.scroll_offset = max_off
                    self.last_drag_x = event.x()
                    self.update()

    def wheelEvent(self, event):
        delta = event.angleDelta().y()
        step = 2
        self.visible_count = max(5, self.visible_count - step) if delta > 0 else min(len(self.candles), self.visible_count + step)
        self.update()

    def zoom_in(self):
        self.visible_count = max(5, self.visible_count - 5)
        self.update()

    def zoom_out(self):
        limit = len(self.candles) if len(self.candles) > 0 else 100
        self.visible_count = min(limit, self.visible_count + 5)
        self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing, False)
        painter.fillRect(self.rect(), QColor("#131314"))
        
        if len(self.candles) == 0:
            painter.setPen(QColor("#E3E3E3"))
            painter.drawText(self.rect(), Qt.AlignCenter, "No Chart Data Available")
            return
            
        w = self.width()
        h = self.height()
        
        total = len(self.candles)
        
        # Viewport
        end_idx = total - self.scroll_offset
        start_idx = end_idx - self.visible_count
        
        d_start = max(0, int(start_idx))
        d_end = min(total, int(end_idx))
        view_candles = self.candles[d_start:d_end]
        
        highs = [c['high'] for c in view_candles] if len(view_candles) > 0 else []
        lows = [c['low'] for c in view_candles] if len(view_candles) > 0 else []
        
        entry = float(self.signal.get("entryPrice", 0.0))
        sl = float(self.signal.get("slPrice", 0.0))
        tp1 = float(self.signal.get("tp1Price", 0.0))
        tp2 = float(self.signal.get("tp2Price", 0.0))
        
        levels = [l for l in [entry, sl, tp1, tp2] if l > 0]
        if levels:
            highs.append(max(levels))
            lows.append(min(levels))
            
        max_p = max(highs)
        min_p = min(lows)
        rng = max_p - min_p
        if rng == 0: rng = 1.0
        
        min_p -= rng * 0.1
        max_p += rng * 0.1
        rng = max_p - min_p
        
        def to_y(price):
            return h - ((price - min_p) / rng * h)
            
        candle_w = w / self.visible_count if self.visible_count > 0 else 0
        
        for i in range(self.visible_count):
            idx = int(start_idx + i)
            if 0 <= idx < total:
                c = self.candles[idx]
                cx = i * candle_w + candle_w / 2
                y_h = to_y(c['high'])
                y_l = to_y(c['low'])
                y_o = to_y(c['open'])
                y_c = to_y(c['close'])
                
                is_bull = c['close'] >= c['open']
                color = QColor("#81C995") if is_bull else QColor("#F28B82")
                
                painter.setPen(color)
                painter.drawLine(int(cx), int(y_h), int(cx), int(y_l))
                
                body_top = min(y_o, y_c)
                body_h = abs(y_o - y_c)
                if body_h < 1: body_h = 1
                
                painter.fillRect(int(cx - candle_w*0.4), int(body_top), int(candle_w*0.8), int(body_h), color)
                
                # Marker for Signal Candle
                signal_ts = self.signal.get("createdAt", 0)
                classification = str(self.signal.get("classification", "scalp")).lower()
                
                if "scalp" in classification:
                    interval_sec = 60
                elif "swing" in classification:
                    interval_sec = 3600
                else:
                    interval_sec = 300

                if c['time'] <= signal_ts < c['time'] + interval_sec:
                    painter.setBrush(QBrush(QColor("#FDD663")))
                    painter.setPen(Qt.NoPen)
                    arrow_size = 6
                    if "long" in self.signal.get("entryType", "").lower():
                        # Arrow UP below Low
                        p1, p2, p3 = QPointF(cx, y_l + 5), QPointF(cx - arrow_size, y_l + 15), QPointF(cx + arrow_size, y_l + 15)
                        painter.drawPolygon(QPolygonF([p1, p2, p3]))
                    else:
                        # Arrow DOWN above High
                        p1, p2, p3 = QPointF(cx, y_h - 5), QPointF(cx - arrow_size, y_h - 15), QPointF(cx + arrow_size, y_h - 15)
                        painter.drawPolygon(QPolygonF([p1, p2, p3]))

        def draw_level(price, color, label):
            if price <= 0: return
            y = to_y(price)
            pen = QPen(QColor(color), 1, Qt.DashLine)
            painter.setPen(pen)
            painter.drawLine(0, int(y), w, int(y))
            painter.drawText(5, int(y) - 2, f"{label} {price:.2f}")
            
        draw_level(entry, "#F1F1F1", "ENTRY")
        draw_level(sl, "#F28B82", "SL")
        draw_level(tp1, "#81C995", "TP1")
        if tp2 > 0: draw_level(tp2, "#81C995", "TP2")

class SignalDetailsDialog(QDialog):
    def __init__(self, signal_data, parent=None):
        super().__init__(parent)
        self.signal_data = signal_data
        self.setWindowTitle("Signal Details")
        self.resize(450, 380) # Make it more compact initially
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(10, 10, 10, 10)
        layout.setSpacing(8)
        
        # --- Header ---
        header_layout = QHBoxLayout()
        ts = signal_data.get("createdAt", 0)
        dt_str = datetime.fromtimestamp(ts).strftime("%Y-%m-%d %H:%M:%S")
        
        lbl_id = QLabel(f"ID: {signal_data.get('signalId', 'N/A')}")
        lbl_id.setStyleSheet("color: #A8C7FA; font-weight: bold;")
        lbl_time = QLabel(f"Time: {dt_str}")
        lbl_time.setStyleSheet("color: #E3E3E3;")
        
        header_layout.addWidget(lbl_id)
        header_layout.addStretch()
        header_layout.addWidget(lbl_time)
        layout.addLayout(header_layout)
        
        # --- Mini Chart ---
        self.chart = SignalMiniChart(signal_data)
        self.chart.setFixedHeight(180)
        layout.addWidget(self.chart)
        
        # --- Info Grid ---
        info_frame = QFrame()
        info_frame.setProperty("class", "Panel")
        info_layout = QGridLayout(info_frame)
        info_layout.setContentsMargins(8, 8, 8, 8)
        info_layout.setVerticalSpacing(4)
        info_layout.setHorizontalSpacing(10)
        
        e_type = signal_data.get("entryType", "").upper()
        type_color = "#81C995" if "LONG" in e_type else "#F28B82"
        
        # Row 0: Type, Price, Score
        info_layout.addWidget(QLabel("Type:"), 0, 0)
        lbl_type = QLabel(e_type)
        lbl_type.setStyleSheet(f"color: {type_color}; font-weight: bold;")
        info_layout.addWidget(lbl_type, 0, 1)
        
        info_layout.addWidget(QLabel("Price:"), 0, 2)
        info_layout.addWidget(QLabel(str(signal_data.get("entryPrice", 0.0))), 0, 3)
        
        info_layout.addWidget(QLabel("Score:"), 0, 4)
        lbl_score = QLabel(f"{signal_data.get('convictionScore', 0):.1f}%")
        lbl_score.setStyleSheet("color: #FDD663;")
        info_layout.addWidget(lbl_score, 0, 5)
        
        # Row 1: SL, TP1, TP2
        info_layout.addWidget(QLabel("SL:"), 1, 0)
        lbl_sl = QLabel(str(signal_data.get("slPrice", 0.0)))
        lbl_sl.setStyleSheet("color: #F28B82;")
        info_layout.addWidget(lbl_sl, 1, 1)
        
        info_layout.addWidget(QLabel("TP1:"), 1, 2)
        lbl_tp1 = QLabel(str(signal_data.get("tp1Price", 0.0)))
        lbl_tp1.setStyleSheet("color: #81C995;")
        info_layout.addWidget(lbl_tp1, 1, 3)
        
        info_layout.addWidget(QLabel("TP2:"), 1, 4)
        lbl_tp2 = QLabel(str(signal_data.get("tp2Price", 0.0)))
        lbl_tp2.setStyleSheet("color: #81C995;")
        info_layout.addWidget(lbl_tp2, 1, 5)
        
        # Row 2: Reason
        info_layout.addWidget(QLabel("Reason:"), 2, 0)
        lbl_reason = QLabel(str(signal_data.get("reason", "-")))
        lbl_reason.setWordWrap(True)
        lbl_reason.setStyleSheet("font-size: 9pt; color: #E3E3E3;")
        info_layout.addWidget(lbl_reason, 2, 1, 1, 5)
        
        layout.addWidget(info_frame)
        
        # --- Debug Info (Collapsible) ---
        debug_data = signal_data.get("debugInfo", {})
        if debug_data:
            self.btn_toggle_debug = QPushButton("Show Debug Info ▼")
            self.btn_toggle_debug.setCheckable(True)
            self.btn_toggle_debug.setChecked(False)
            self.btn_toggle_debug.setStyleSheet("text-align: left; font-weight: bold; padding: 4px; background-color: #2D2E31;")
            layout.addWidget(self.btn_toggle_debug)

            self.dbg_frame = QFrame()
            self.dbg_frame.setProperty("class", "Panel")
            self.dbg_frame.setVisible(False) # Initially hidden
            dbg_layout = QGridLayout(self.dbg_frame)
            dbg_layout.setSpacing(8)
            
            row, col = 0, 0
            for k, v in sorted(debug_data.items()):
                k_clean = k.replace("_", " ").title()
                dbg_layout.addWidget(QLabel(f"{k_clean}:"), row, col)
                val_lbl = QLabel(str(v))
                val_lbl.setStyleSheet("font-family: Consolas; color: #FDD663;")
                dbg_layout.addWidget(val_lbl, row, col + 1)
                
                col += 2
                if col >= 4:
                    col = 0
                    row += 1
            layout.addWidget(self.dbg_frame)

            self.btn_toggle_debug.clicked.connect(self.toggle_debug_info)
            
        layout.addStretch()
        
        # Footer
        btn_layout = QHBoxLayout()
        
        btn_raw = QPushButton("Copy JSON")
        btn_raw.setFixedWidth(80)
        btn_raw.setToolTip("Copy raw JSON to clipboard")
        btn_raw.clicked.connect(self.copy_json)
        
        btn_reexec = QPushButton("Re-Execute")
        btn_reexec.setToolTip("Open Manual Trade Panel with these parameters")
        btn_reexec.clicked.connect(self.on_re_execute)
        
        btn_close = QPushButton("Close")
        btn_close.clicked.connect(self.accept)
        
        btn_layout.addWidget(btn_raw)
        btn_layout.addWidget(btn_reexec)
        btn_layout.addStretch()
        btn_layout.addWidget(btn_close)
        
        layout.addLayout(btn_layout)

    def toggle_debug_info(self, checked):
        self.dbg_frame.setVisible(checked)
        self.btn_toggle_debug.setText("Hide Debug Info ▲" if checked else "Show Debug Info ▼")
        self.adjustSize()

    def copy_json(self):
        QApplication.clipboard().setText(json.dumps(self.signal_data, indent=4))
        ModernToast.show_message(self, "JSON Copied to Clipboard", style="success")

    def on_re_execute(self):
        parent = self.parent()
        if hasattr(parent, "open_manual"):
            parent.open_manual()
            if parent.manual_dialog:
                parent.manual_dialog.setup_from_signal(self.signal_data)
        self.accept()

class UnifiedSidePanel(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("UnifiedSidePanel")
        self.setAttribute(Qt.WA_StyledBackground, True)
        # Modern card-like background with subtle gradient and soft border
        self.setStyleSheet("""
            #UnifiedSidePanel {
                background-color: #1E1F20;
                border: 1px solid #444746;
                border-radius: 8px;
            }
            #UnifiedSidePanel QTabWidget::pane {
                background: transparent;
                border: none;
            }
            #UnifiedSidePanel QTabBar::tab {
                background: transparent;
                color: #C4C7C5;
                padding: 6px 8px;
                min-width: 60px;
                margin-right: 2px;
                border-radius: 6px;
                border: 1px solid transparent;
                font-size: 9pt;
            }
            #UnifiedSidePanel QTabBar::tab:selected {
                background: #A8C7FA;
                color: #000000;
                font-weight: 700;
                border: 1px solid rgba(168,199,250,0.12);
            }
            #UnifiedSidePanel QTabBar::tab:!selected:hover {
                background: rgba(67,76,94,0.25);
                border: 1px solid rgba(67,76,94,0.6);
            }
            /* Small card style for inner panels */
            #UnifiedSidePanel QFrame[class="Panel"] { background-color: #252628; border-radius: 8px; border: 1px solid #5F6368; }
        """)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(12, 12, 12, 12)
        layout.setSpacing(8)

        self.tabs = QTabWidget()

        self.trade_panel = ManualExecutionDialog()
        self.calc_panel = CalculatorDialog()

        self.tabs.addTab(self.trade_panel, "Trade")
        self.tabs.addTab(self.calc_panel, "Calculator")

        layout.addWidget(self.tabs)