import sys
import os
import csv
import shutil
import json
import time
from typing import Dict, List, Any, Optional
from datetime import datetime, timedelta

from PySide6.QtWidgets import (QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,  # type: ignore
                               QLabel, QFrame, QMessageBox, QGridLayout, QHeaderView, 
                               QTreeWidget, QTreeWidgetItem, QPushButton, QTabWidget, QSplitter,
                               QFileDialog, QMenu, QDoubleSpinBox, QAbstractSpinBox, QComboBox, QSizePolicy,
                               QScrollArea)
from PySide6.QtCore import Qt, QThread, Slot, QTimer, QByteArray, Signal, QEvent # type: ignore
from PySide6.QtGui import QColor, QIcon, QAction, QBrush, QShortcut, QKeySequence, QFontMetrics # type: ignore
import MetaTrader5 as mt5 # type: ignore

from config import CONFIG, state, save_config
from gui_styles import GLOBAL_STYLESHEET
from mt5_interface import execute_trade
from gui_widgets import SignalIndicator, StatusCircle, SafetyButton, ToggleSwitch, ModernSpinBox, ATRGauge, PortfolioStats, StrategyChips, SessionPanel
from gui_dialogs import AdvancedSettingsDialog, ManualExecutionDialog, PositionModifyDialog, SignalDetailsDialog, ModernToast, CalculatorDialog, UnifiedSidePanel
from gui_chart import ChartWindow
from gui_workers import MT5DataWorker, SignalHistoryWorker

class DashboardGUI(QMainWindow):
    stop_worker_signal = Signal()
    toggle_chart_data_signal = Signal(bool)

    def __init__(self):
        super().__init__()
        self.setWindowTitle("XAU Scalper v4 - Dashboard")
        if os.path.exists("icon.ico"):
            self.setWindowIcon(QIcon("icon.ico"))
        self.resize(1100, 750)
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        # Data Worker Thread
        self.worker_thread = QThread()
        self.worker = MT5DataWorker()
        self.worker.moveToThread(self.worker_thread)
        
        self.worker.data_updated.connect(self.update_ui)
        self.worker_thread.started.connect(self.worker.start_working)
        self.stop_worker_signal.connect(self.worker.stop_working)
        self.toggle_chart_data_signal.connect(self.worker.set_chart_data_enabled)
        
        # UI Components
        self.chart_window = None
        self.manual_dialog = None
        self.calc_dialog = None
        self.settings_dialog = None
        self.position_items = {}
        self.blink_state = False
        self._is_closing = False
        self._last_history_sig = None
        self._last_signal_ts = 0
        
        self.load_ui_state()
        self.init_ui()
        
        self.blink_timer = QTimer()
        self.blink_timer.timeout.connect(self.blink_labels)
        self.blink_timer.start(800)
        
        # Start Worker
        self.worker_thread.start()

    def load_ui_state(self):
        try:
            with open("ui_state.json", "r") as f:
                data = json.load(f)
                geom = QByteArray.fromBase64(data.get("geometry", "").encode())
                self.restoreGeometry(geom)
                if data.get("state"):
                    self.restoreState(QByteArray.fromBase64(data.get("state").encode()))
                # Restore chart window if it was open
                if data.get("chart_open", False):
                    QTimer.singleShot(0, self.open_chart)
        except Exception:
            pass

    def init_ui(self):
        central = QWidget()
        self.setCentralWidget(central)
        
        root_layout = QVBoxLayout(central)
        root_layout.setSpacing(0)
        root_layout.setContentsMargins(0, 0, 0, 0)
        
        upper_area = QWidget()
        upper_layout = QHBoxLayout(upper_area)
        upper_layout.setSpacing(15)
        upper_layout.setContentsMargins(15, 15, 15, 15)
        
        root_layout.addWidget(upper_area)
        
        main_content = QWidget()
        main_layout = QVBoxLayout(main_content)
        main_layout.setSpacing(15)
        main_layout.setContentsMargins(0, 0, 0, 0)
        
        upper_layout.addWidget(main_content)
        
        # --- HEADER ---
        header_frame = QFrame()
        header_frame.setProperty("class", "Panel")
        header_frame.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
        header_frame.setStyleSheet("""
            QFrame[class="Panel"] { background-color: #1E1F20; border-radius: 8px; border: 1px solid #444746; }
            QLabel { color: #E3E3E3; }
        """)
        
        header_layout = QGridLayout(header_frame)
        header_layout.setContentsMargins(10, 2, 10, 2)
        header_layout.setHorizontalSpacing(20)
        header_layout.setVerticalSpacing(0)
        
        # --- SECTION 1: Market Data (Left) ---
        # Row 0: Session Panel (Sidebar Toggle via Ctrl+B)
        h_sec1_r0 = QHBoxLayout()
        
        self.shortcut_sidebar = QShortcut(QKeySequence("Ctrl+B"), self)
        self.shortcut_sidebar.activated.connect(self.toggle_sidebar)
        
        self.session_panel = SessionPanel()
        h_sec1_r0.addWidget(self.session_panel)
        h_sec1_r0.addStretch()
        header_layout.addLayout(h_sec1_r0, 0, 0)
        
        # Row 1: Symbol & Delta
        h_sec1_r1 = QHBoxLayout()
        self.lbl_symbol = QLabel(CONFIG["trade_symbol"])
        self.lbl_symbol.setStyleSheet("font-size: 16pt; font-weight: 900; color: #A8C7FA;")
        self.lbl_symbol.setToolTip("Trading Symbol")
        self.lbl_delta = QLabel("Δ: --")
        self.lbl_delta.setStyleSheet("font-size: 9pt; font-weight: bold; color: #E3E3E3; padding-top: 4px;")
        self.lbl_delta.setToolTip("Price Change")
        h_sec1_r1.addWidget(self.lbl_symbol)
        h_sec1_r1.addSpacing(8)
        h_sec1_r1.addWidget(self.lbl_delta)
        h_sec1_r1.addStretch()
        header_layout.addLayout(h_sec1_r1, 1, 0)

        # Row 2: Mid Price
        self.lbl_mid = QLabel("--")
        self.lbl_mid.setStyleSheet("font-size: 22pt; font-weight: bold; color: #F1F1F1; font-family: Consolas;")
        self.lbl_mid.setToolTip("Current Mid Price")
        header_layout.addWidget(self.lbl_mid, 2, 0)

        # Row 3: Bid/Ask & Spread
        h_sec1_r3 = QHBoxLayout()
        self.lbl_bidask = QLabel("B:-- / A:--")
        self.lbl_bidask.setStyleSheet("font-size: 9pt; color: #81C995; font-family: Consolas;")
        self.lbl_bidask.setToolTip("Bid / Ask Prices")
        self.lbl_spread = QLabel("Spr: --")
        self.lbl_spread.setStyleSheet("font-size: 9pt; color: #F28B82; font-weight: 600; margin-left: 8px;")
        self.lbl_spread.setToolTip("Current Spread")
        h_sec1_r3.addWidget(self.lbl_bidask)
        h_sec1_r3.addWidget(self.lbl_spread)
        h_sec1_r3.addStretch()
        header_layout.addLayout(h_sec1_r3, 3, 0)
        
        # --- SECTION 2: P/L & Account (Center) ---
        v_sec2 = QVBoxLayout()
        v_sec2.setSpacing(2)
        v_sec2.setAlignment(Qt.AlignCenter)
        
        self.lbl_open_pl = QLabel("$0.00")
        self.lbl_open_pl.setAlignment(Qt.AlignCenter)
        self.lbl_open_pl.setStyleSheet("font-size: 28pt; font-weight: bold; color: #F1F1F1;")
        self.lbl_open_pl.setToolTip("Total Floating P/L")
        self.lbl_account = QLabel("Balance: $0.00 | Equity: $0.00")
        self.lbl_account.setAlignment(Qt.AlignCenter)
        self.lbl_account.setStyleSheet("font-size: 9pt; color: #FDD663; font-weight: 600;")
        self.lbl_account.setToolTip("Account Balance | Equity")
        
        self.portfolio_stats = PortfolioStats()
        
        v_sec2.addWidget(self.lbl_open_pl)
        v_sec2.addWidget(self.lbl_account)
        v_sec2.addWidget(self.portfolio_stats)
        header_layout.addLayout(v_sec2, 0, 1, 4, 1)
        
        # --- SECTION 3: Stats & Controls (Right) ---
        # Row 0: ATR & Status
        h_sec3_r0 = QHBoxLayout()
        h_sec3_r0.addStretch()
        self.atr_gauge = ATRGauge()
        self.atr_gauge.setToolTip("ATR Volatility Meter")
        h_sec3_r0.addWidget(self.atr_gauge)
        header_layout.addLayout(h_sec3_r0, 0, 2)

        # Row 3: Mode & Buttons
        h_sec3_r3 = QHBoxLayout()
        self.strategy_chips = StrategyChips()
        self.strategy_chips.setToolTip("Active Strategy Modes")
        h_sec3_r3.addWidget(self.strategy_chips)
        h_sec3_r3.addStretch()
        
        self.btn_chart = QPushButton("📈")
        self.btn_chart.setFixedSize(32, 32)
        self.btn_chart.setStyleSheet("QPushButton { font-size: 16px; background-color: #2D2E31; border-radius: 6px; border: 1px solid #444746; } QPushButton:hover { background-color: #444746; }")
        self.btn_chart.setToolTip("Open Chart Window")
        self.btn_chart.clicked.connect(self.open_chart)

        self.btn_settings = QPushButton("⚙")
        self.btn_settings.setFixedSize(32, 32)
        self.btn_settings.setStyleSheet("QPushButton { font-size: 16px; background-color: #2D2E31; border-radius: 6px; border: 1px solid #444746; } QPushButton:hover { background-color: #444746; }")
        self.btn_settings.setToolTip("Open Configuration Settings")
        self.btn_settings.clicked.connect(self.open_settings)
        
        h_sec3_r3.addSpacing(10)
        h_sec3_r3.addWidget(self.btn_chart)
        h_sec3_r3.addSpacing(10)
        h_sec3_r3.addWidget(self.btn_settings)
        header_layout.addLayout(h_sec3_r3, 3, 2)
        
        # Column Stretches
        header_layout.setColumnStretch(0, 2)
        header_layout.setColumnStretch(1, 3)
        header_layout.setColumnStretch(2, 2)
        
        main_layout.addWidget(header_frame)

        # --- MAIN CONTENT (Splitter) ---
        splitter = QSplitter(Qt.Horizontal)
        splitter.setHandleWidth(2)
        splitter.setChildrenCollapsible(False)
        
        # Left Sidebar (Controls)
        self.sidebar = QFrame()
        self.sidebar.setProperty("class", "Panel")
        sidebar_layout = QVBoxLayout(self.sidebar)
        sidebar_layout.setSpacing(0)
        sidebar_layout.setContentsMargins(0, 0, 0, 0)
        
        sidebar_layout.setSpacing(6)
        sidebar_layout.setContentsMargins(6, 6, 6, 6)
        

        lbl_strat = QLabel("Strategy Toggles")
        lbl_strat.setProperty("class", "SubHeader")
        lbl_strat.setAlignment(Qt.AlignCenter)
        # moved into the right-side Controls tab
        
        # Toggles
        toggles_frame = QFrame()
        toggles_frame.setProperty("class", "Panel")
        toggles_frame.setStyleSheet("background-color: #252628; border-radius: 8px; border: 1px solid #5F6368;")
        toggles_layout = QGridLayout(toggles_frame)
        self.sidebar.setMinimumWidth(190)
        self.sidebar.setMaximumWidth(300)
        self.sidebar.setSizePolicy(QSizePolicy.Preferred, QSizePolicy.Expanding)
        toggles_layout.setVerticalSpacing(6)
        toggles_layout.setHorizontalSpacing(6)
        toggles_layout.setContentsMargins(6, 6, 6, 6)
        toggles_layout.setColumnStretch(0, 1)
        toggles_layout.setColumnStretch(1, 1)
        
        self.create_toggle(toggles_layout, "Scalp", "scalp_mode", 0, 0, "Enable Scalping Strategy")
        self.create_toggle(toggles_layout, "Swing", "swing_mode", 0, 1, "Enable Swing Strategy")
        self.create_toggle(toggles_layout, "Tr. Scalp", "use_trailing_scalp", 1, 0, "Enable Trailing Stop for Scalp Trades")
        self.create_toggle(toggles_layout, "Tr. Swing", "use_trailing_swing", 1, 1, "Enable Trailing Stop for Swing Trades")
        self.create_toggle(toggles_layout, "Force Mkt", "force_market", 2, 0, "Force Market Execution (Disable Limit Orders)")
        self.create_toggle(toggles_layout, "Man. Mgmt", "manage_manual", 2, 1)

        lbl_stats = QLabel("Active Positions")
        lbl_stats.setProperty("class", "SubHeader")
        lbl_stats.setAlignment(Qt.AlignCenter)
        # moved into the right-side Controls tab

        # Counts
        counts_frame = QFrame()
        counts_frame.setProperty("class", "Panel")
        counts_frame.setStyleSheet("background-color: #252628; border-radius: 8px; border: 1px solid #5F6368;")
        counts_layout = QGridLayout(counts_frame)
        counts_layout.setSpacing(6)
        counts_layout.setContentsMargins(6, 6, 6, 6)
        
        self.lbl_scalp_count = QLabel("Scalp: 0")
        self.lbl_swing_count = QLabel("Swing: 0")
        self.lbl_manual_count = QLabel("Manual: 0")
        # use compact font for counts to reduce horizontal width
        self.lbl_scalp_count.setStyleSheet("font-size:9pt;")
        self.lbl_swing_count.setStyleSheet("font-size:9pt;")
        self.lbl_manual_count.setStyleSheet("font-size:9pt;")
        self.lbl_scalp_count.setToolTip("Active Scalp Positions")
        self.lbl_swing_count.setToolTip("Active Swing Positions")
        self.lbl_manual_count.setToolTip("Active Manual Positions")
        
        counts_layout.addWidget(self.lbl_scalp_count, 0, 0)
        counts_layout.addWidget(self.lbl_swing_count, 0, 1)
        counts_layout.addWidget(self.lbl_manual_count, 1, 0, 1, 2, Qt.AlignLeft)

        lbl_mgmt = QLabel("Trade Management")
        lbl_mgmt.setProperty("class", "SubHeader")
        lbl_mgmt.setAlignment(Qt.AlignCenter)
        # moved into the right-side Controls tab

        # Controls Frame (Moved from Open Tab)
        controls_frame = QFrame()
        controls_frame.setProperty("class", "Panel")
        controls_frame.setStyleSheet("background-color: #252628; border-radius: 8px; border: 1px solid #5F6368;")
        controls_layout = QGridLayout(controls_frame)
        controls_layout.setContentsMargins(6, 6, 6, 6)
        controls_layout.setSpacing(6)

        self.spin_update_sl = QDoubleSpinBox()
        self.spin_update_sl.setDecimals(2)
        self.spin_update_sl.setRange(0, 99999)
        self.spin_update_sl.setPrefix("SL: ")
        self.spin_update_sl.setValue(0)
        self.spin_update_sl.setFixedHeight(32)
        self.spin_update_sl.setButtonSymbols(QAbstractSpinBox.NoButtons)
        self.spin_update_sl.setToolTip("New Stop Loss Price")
        
        self.btn_copy_sl = QPushButton("📍")
        self.btn_copy_sl.setFixedSize(32, 32)
        self.btn_copy_sl.setToolTip("Copy Current Bid Price")
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
        self.spin_update_tp.setValue(0)
        self.spin_update_tp.setFixedHeight(32)
        self.spin_update_tp.setButtonSymbols(QAbstractSpinBox.NoButtons)
        self.spin_update_tp.setToolTip("New Take Profit Price")
        
        self.btn_copy_tp = QPushButton("📍")
        self.btn_copy_tp.setFixedSize(32, 32)
        self.btn_copy_tp.setToolTip("Copy Current Bid Price")
        self.btn_copy_tp.clicked.connect(lambda: self.copy_price_to_field(self.spin_update_tp))
        
        tp_container = QWidget()
        tp_layout = QHBoxLayout(tp_container)
        tp_layout.setContentsMargins(0,0,0,0)
        tp_layout.setSpacing(2)
        tp_layout.addWidget(self.spin_update_tp)
        tp_layout.addWidget(self.btn_copy_tp)
        
        self.btn_update_selected = QPushButton("Update Selected")
        self.btn_update_selected.setFixedHeight(34)
        self.btn_update_selected.setToolTip("Apply SL/TP to Selected Trade")
        self.btn_update_selected.clicked.connect(self.update_selected_sltp)
        
        self.btn_update_manual = QPushButton("Update Manual")
        self.btn_update_manual.setFixedHeight(34)
        self.btn_update_manual.setToolTip("Apply SL/TP to All Manual Trades")
        self.btn_update_manual.clicked.connect(self.update_manual_sltp)
        
        controls_layout.addWidget(sl_container, 0, 0)
        controls_layout.addWidget(tp_container, 0, 1)
        controls_layout.addWidget(self.btn_update_selected, 1, 0)
        controls_layout.addWidget(self.btn_update_manual, 1, 1)
        
        # Bulk Actions
        self.close_combo = QComboBox()
        self.close_combo.addItems(["Close All Scalp", "Close All Swing", "Close All Manual", "Close Scalp Winners", "Close Scalp Losers", "Close ALL Positions"])
        self.close_combo.setFixedHeight(32)
        self.close_combo.setToolTip("Select Bulk Action")
        
        self.exec_btn = SafetyButton("HOLD TO EXECUTE")
        self.exec_btn.setFixedHeight(32)
        self.exec_btn.setToolTip("Hold to Execute Selected Action")
        self.exec_btn.triggered.connect(self.execute_close_action)
        
        controls_layout.addWidget(self.close_combo, 2, 0, 1, 2)
        controls_layout.addWidget(self.exec_btn, 3, 0, 1, 2)

        # controls_frame and panic button will be moved into the right-side
        # unified panel as a separate tab (see below) so they are not added
        # to the left sidebar here.
        
        # Right Content (Tabs)
        self.tabs = QTabWidget()
        
        # Tab 1: Positions
        self.pos_tab = QWidget()
        pos_layout = QVBoxLayout(self.pos_tab)
        
        self.tree_positions = QTreeWidget()
        self.tree_positions.setHeaderLabels(["Ticket", "Symbol", "Type", "Vol", "Open", "SL", "TP", "Profit", ""])
        self.tree_positions.setToolTip("List of Open Positions")
        # Configure column resize modes: allow interactive resizing like Logs
        # Symbol column stretches to take remaining space; last column is fixed
        for i in range(self.tree_positions.columnCount()):
            if i == 1:
                self.tree_positions.header().setSectionResizeMode(i, QHeaderView.Stretch)
            elif i == 8:
                self.tree_positions.header().setSectionResizeMode(i, QHeaderView.Fixed)
                self.tree_positions.setColumnWidth(8, 40)
            else:
                self.tree_positions.header().setSectionResizeMode(i, QHeaderView.Interactive)
        self.tree_positions.setAlternatingRowColors(True)
        
        # Set default widths for better alignment
        self.tree_positions.setColumnWidth(0, 80)  # Ticket
        self.tree_positions.setColumnWidth(2, 60)  # Type
        self.tree_positions.setColumnWidth(3, 50)  # Vol
        self.tree_positions.setColumnWidth(4, 70)  # Open
        self.tree_positions.setColumnWidth(5, 70)  # SL
        self.tree_positions.setColumnWidth(6, 70)  # TP
        self.tree_positions.setColumnWidth(7, 70)  # Profit
        
        self.tree_positions.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree_positions.customContextMenuRequested.connect(self.show_context_menu)
        # Make headers interactive so user can drag to resize columns (like Logs)
        self.tree_positions.header().setSectionsClickable(True)
        self.tree_positions.header().setStretchLastSection(False)
        # Allow manual resizing for non-stretch columns
        for i in range(self.tree_positions.columnCount()):
            if i != 1 and i != 8:
                self.tree_positions.header().setSectionResizeMode(i, QHeaderView.Interactive)
        self.tree_positions.itemDoubleClicked.connect(self.on_position_dbl_click)
        self.tree_positions.itemSelectionChanged.connect(self.on_trade_selected)
        pos_layout.addWidget(self.tree_positions)
        # Ensure header labels align with the item text alignment for consistency
        header_item = self.tree_positions.headerItem()
        for c in range(self.tree_positions.columnCount()):
            if c in [3, 4, 5, 6, 7]:
                header_item.setTextAlignment(c, Qt.AlignRight | Qt.AlignVCenter)
            else:
                header_item.setTextAlignment(c, Qt.AlignCenter | Qt.AlignVCenter)
        
        self.lbl_tab_open_pl = QLabel("Total P/L: $0.00")
        self.lbl_tab_open_pl.setAlignment(Qt.AlignRight)
        self.lbl_tab_open_pl.setStyleSheet("font-weight: bold; font-size: 10pt; color: #E3E3E3; margin-top: 5px; margin-bottom: 5px;")
        self.lbl_tab_open_pl.setToolTip("Total P/L of Listed Positions")
        pos_layout.addWidget(self.lbl_tab_open_pl)
        
        self.tabs.addTab(self.pos_tab, "Open")
        
        # Tab 2: History
        self.hist_tab = QWidget()
        hist_layout = QVBoxLayout(self.hist_tab)
        
        # History Filter
        hist_filter_layout = QHBoxLayout()
        hist_filter_layout.setContentsMargins(0, 0, 0, 0)
        self.combo_hist_filter = QComboBox()
        self.combo_hist_filter.addItems(["All", "Profitable", "Losing"])
        self.combo_hist_filter.setFixedWidth(100)
        hist_filter_layout.addWidget(QLabel("Show:"))
        hist_filter_layout.addWidget(self.combo_hist_filter)
        hist_filter_layout.addStretch()
        hist_layout.addLayout(hist_filter_layout)
        
        self.tree_history = QTreeWidget()
        self.tree_history.setHeaderLabels(["Time", "Ticket", "Type", "Vol", "Duration", "Profit/Loss"])
        self.tree_history.setToolTip("Trade History (Last 24h)")
        self.tree_history.header().setSectionResizeMode(0, QHeaderView.Interactive)
        self.tree_history.header().setSectionResizeMode(1, QHeaderView.Interactive)
        self.tree_history.header().setSectionResizeMode(2, QHeaderView.Interactive)
        self.tree_history.header().setSectionResizeMode(3, QHeaderView.Interactive)
        self.tree_history.header().setSectionResizeMode(4, QHeaderView.Interactive)
        self.tree_history.header().setSectionResizeMode(5, QHeaderView.Stretch)
        self.tree_history.setAlternatingRowColors(True)
        self.tree_history.setColumnWidth(0, 130) # Time
        self.tree_history.setColumnWidth(1, 80)  # Ticket
        self.tree_history.setColumnWidth(2, 60)  # Type
        self.tree_history.setColumnWidth(3, 60)  # Vol
        self.tree_history.setColumnWidth(4, 70)  # Duration
        hist_layout.addWidget(self.tree_history)
        # Match header alignment to history item alignments
        hist_header = self.tree_history.headerItem()
        hist_align = {
            0: Qt.AlignCenter,
            1: Qt.AlignLeft,
            2: Qt.AlignCenter,
            3: Qt.AlignRight,
            4: Qt.AlignCenter,
            5: Qt.AlignRight,
        }
        for c in range(self.tree_history.columnCount()):
            a = hist_align.get(c, Qt.AlignCenter)
            hist_header.setTextAlignment(c, a | Qt.AlignVCenter)
        # Allow user to resize history columns interactively and add context menu similar to logs
        self.tree_history.header().setSectionsClickable(True)
        self.tree_history.header().setStretchLastSection(False)
        for i in [1,2,3,4]:
            self.tree_history.header().setSectionResizeMode(i, QHeaderView.Interactive)
        self.tree_history.header().setSectionResizeMode(5, QHeaderView.Stretch)
        self.tree_history.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree_history.customContextMenuRequested.connect(self.show_history_context_menu)
        
        self.total_profit_label = QLabel("Bal: $0.00 | P/L: $0.00")
        self.total_profit_label.setAlignment(Qt.AlignRight)
        self.total_profit_label.setStyleSheet("font-weight: bold; font-size: 10pt;")
        self.total_profit_label.setToolTip("Account Balance | 24h Profit/Loss")
        hist_layout.addWidget(self.total_profit_label)
        
        perf_btn_layout = QHBoxLayout()
        self.btn_export_perf = QPushButton("Export Perf CSV")
        self.btn_export_perf.setToolTip("Export Performance to CSV")
        self.btn_export_perf.clicked.connect(self.export_performance_csv)
        perf_btn_layout.addWidget(self.btn_export_perf)
        
        self.btn_clear_perf = QPushButton("Clear Perf CSV")
        self.btn_clear_perf.setToolTip("Clear Performance History")
        self.btn_clear_perf.clicked.connect(self.clear_performance_csv)
        perf_btn_layout.addWidget(self.btn_clear_perf)
        hist_layout.addLayout(perf_btn_layout)
        
        self.tabs.addTab(self.hist_tab, "History")
        
        # Tab 3: Logs
        self.log_tab = QWidget()
        log_layout = QVBoxLayout(self.log_tab)
        
        self.tree_logs = QTreeWidget()
        self.tree_logs.setHeaderLabels(["Time", "Ticket", "Type", "Details"])
        self.tree_logs.setToolTip("System Logs")
        self.tree_logs.setColumnWidth(0, 80)
        self.tree_logs.setColumnWidth(1, 80)
        self.tree_logs.setColumnWidth(2, 100)
        self.tree_logs.setAlternatingRowColors(True)
        self.tree_logs.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree_logs.customContextMenuRequested.connect(self.show_log_context_menu)
        log_layout.addWidget(self.tree_logs)
        # Align log headers sensibly with log item columns
        logs_header = self.tree_logs.headerItem()
        for c in range(self.tree_logs.columnCount()):
            if c in [0, 1, 2]:
                logs_header.setTextAlignment(c, Qt.AlignCenter | Qt.AlignVCenter)
            else:
                logs_header.setTextAlignment(c, Qt.AlignLeft | Qt.AlignVCenter)
        # Make logs header interactive too
        self.tree_logs.header().setSectionsClickable(True)
        self.tree_logs.header().setStretchLastSection(True)
        
        log_btn_layout = QHBoxLayout()
        self.btn_export_logs = QPushButton("Export CSV")
        self.btn_export_logs.setToolTip("Export Logs to CSV")
        self.btn_export_logs.clicked.connect(self.export_logs_to_csv)
        log_btn_layout.addWidget(self.btn_export_logs)
        
        self.btn_clear_logs = QPushButton("Clear Logs")
        self.btn_clear_logs.setToolTip("Clear Logs")
        self.btn_clear_logs.clicked.connect(self.clear_logs)
        log_btn_layout.addWidget(self.btn_clear_logs)
        log_layout.addLayout(log_btn_layout)
        
        self.tabs.addTab(self.log_tab, "Logs")
        
        # Tab 4: Signals
        self.sig_tab = QWidget()
        self.init_signals_tab()
        self.tabs.addTab(self.sig_tab, "Signals")
        
        splitter.addWidget(self.tabs)
        
        splitter.setStretchFactor(1, 1)
        main_layout.addWidget(splitter)
        
        # Footer
        footer_frame = QFrame()
        footer_frame.setFixedHeight(18)
        footer_frame.setStyleSheet("background-color: #131314; border-top: 1px solid #444746; border-radius: 0px;")
        footer_layout = QHBoxLayout(footer_frame)
        footer_layout.setContentsMargins(4, 0, 4, 0)
        
        self.signal_indicator = SignalIndicator()
        self.signal_indicator.setFixedSize(8, 8)
        self.signal_indicator.setToolTip("Signal Activity Indicator")
        self.status_label = QLabel("Connecting...")
        self.status_label.setStyleSheet("color: #E3E3E3; font-size: 7pt; font-weight: bold; padding: 0px;")
        self.status_label.setToolTip("Connection Status")
        
        self.ping_indicator = StatusCircle(size=6, color="#444746")
        self.ping_indicator.setToolTip("Latency Indicator")
        self.lbl_latency = QLabel("Ping: -- ms")
        self.lbl_latency.setStyleSheet("color: #E3E3E3; font-size: 6pt;")
        self.lbl_latency.setToolTip("Server Latency (ms)")
        
        footer_layout.addWidget(self.signal_indicator)
        footer_layout.addWidget(self.status_label)
        footer_layout.addSpacing(10)
        footer_layout.addWidget(self.ping_indicator)
        footer_layout.addWidget(self.lbl_latency)
        footer_layout.addStretch()
        
        # --- Right Sidebar (Unified Panel) ---
        self.side_panel = UnifiedSidePanel()
        # make sidebar a compact column that expands vertically
        self.side_panel.setFixedWidth(360)
        self.side_panel.setSizePolicy(QSizePolicy.Fixed, QSizePolicy.Expanding)
        upper_layout.addWidget(self.side_panel)
        
        root_layout.addWidget(footer_frame)

        # Move the existing controls_frame and panic button into a new
        # 'Controls' tab on the right unified panel so the control panel
        # feels integrated with the rest of the right-side tools.
        try:
            controls_tab = QWidget()
            ct_layout = QVBoxLayout(controls_tab)
            ct_layout.setContentsMargins(6, 6, 6, 6)
            ct_layout.setSpacing(6)
            # attach previously-created control widgets into the tab
            ct_layout.addWidget(lbl_strat)
            ct_layout.addWidget(toggles_frame)
            ct_layout.addWidget(lbl_stats)
            ct_layout.addWidget(counts_frame)
            # now add the trade management controls and panic button
            ct_layout.addWidget(lbl_mgmt)
            ct_layout.addWidget(controls_frame)
            ct_layout.addStretch()
            # Wrap controls in a scroll area to avoid horizontal scrolling
            scroll = QScrollArea()
            scroll.setWidgetResizable(True)
            scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
            scroll.setFrameShape(QFrame.NoFrame)
            scroll.setStyleSheet("background: transparent; border: none;")
            controls_tab.setStyleSheet("background: transparent;")
            scroll.setWidget(controls_tab)
            # Insert Controls as first tab
            self.side_panel.tabs.insertTab(0, scroll, "Controls")
            self.side_panel.tabs.setCurrentIndex(0)
        except Exception:
            # If something goes wrong keep running without crash
            pass
        
        # Aliases for compatibility
        self.manual_dialog = self.side_panel.trade_panel
        self.calc_dialog = self.side_panel.calc_panel
        

    def init_signals_tab(self):
        layout = QVBoxLayout(self.sig_tab)
        
        # Controls
        ctrl_layout = QHBoxLayout()
        self.btn_sig_refresh = QPushButton("Refresh")
        self.btn_sig_refresh.setFixedWidth(80)
        self.btn_sig_refresh.clicked.connect(lambda: self.load_signals(self.sig_page))
        
        self.combo_sig_filter = QComboBox()
        self.combo_sig_filter.addItems(["All", "Scalp", "Swing"])
        self.combo_sig_filter.setFixedWidth(80)
        self.combo_sig_filter.currentTextChanged.connect(lambda: self.render_signals())
        
        self.btn_sig_prev = QPushButton("< Prev")
        self.btn_sig_prev.setFixedWidth(80)
        self.btn_sig_prev.clicked.connect(self.prev_sig_page)
        
        self.lbl_sig_page = QLabel("Page 1")
        self.lbl_sig_page.setAlignment(Qt.AlignCenter)
        self.lbl_sig_page.setStyleSheet("font-weight: bold;")
        
        self.btn_sig_next = QPushButton("Next >")
        self.btn_sig_next.setFixedWidth(80)
        self.btn_sig_next.clicked.connect(self.next_sig_page)
        
        ctrl_layout.addWidget(self.btn_sig_refresh)
        ctrl_layout.addWidget(self.combo_sig_filter)
        ctrl_layout.addStretch()
        ctrl_layout.addWidget(self.btn_sig_prev)
        ctrl_layout.addWidget(self.lbl_sig_page)
        ctrl_layout.addWidget(self.btn_sig_next)
        
        layout.addLayout(ctrl_layout)
        
        # Tree
        self.tree_signals = QTreeWidget()
        self.tree_signals.setHeaderLabels(["Time", "ID", "Type", "Class", "Price", "Score"])
        self.tree_signals.setAlternatingRowColors(True)
        self.tree_signals.setColumnWidth(0, 130)
        self.tree_signals.setColumnWidth(1, 180)
        self.tree_signals.setColumnWidth(2, 60)
        self.tree_signals.setColumnWidth(3, 60)
        self.tree_signals.itemDoubleClicked.connect(self.on_signal_dbl_click)
        layout.addWidget(self.tree_signals)
        
        self.sig_page = 1
        self.sig_limit = 50
        self.current_signals_list = []
        
        # Initial Load
        QTimer.singleShot(2000, lambda: self.load_signals(1))

    def toggle_sidebar(self):
        self.sidebar.setVisible(not self.sidebar.isVisible())

    def create_toggle(self, layout, label, key, row, col, tooltip=None):
        container = QWidget()
        h_layout = QHBoxLayout(container)
        h_layout.setContentsMargins(0,0,0,0)
        
        lbl = QLabel(label)
        lbl.setStyleSheet("font-size:9pt;")
        lbl.setMaximumWidth(110)
        cb = ToggleSwitch()
        cb.setFixedWidth(44)
        cb.setChecked(CONFIG[key])
        def on_toggle(s, k=key):
            CONFIG[k] = bool(s)
            save_config()
        cb.stateChanged.connect(on_toggle)
        
        if tooltip:
            container.setToolTip(tooltip)
        
        h_layout.addWidget(lbl)
        h_layout.addStretch()
        h_layout.addWidget(cb)
        
        layout.addWidget(container, row, col)

    def _format_strategy_state(self):
        parts = []
        if CONFIG.get("scalp_mode"): parts.append("Scalp")
        if CONFIG.get("swing_mode"): parts.append("Swing")
        if CONFIG.get("manage_manual"): parts.append("Manual")
        if not parts: return "Idle"
        return ",".join(parts)

    def blink_labels(self):
        self.blink_state = not self.blink_state

    @Slot(dict)
    def update_ui(self, data):
        if self._is_closing:
            return

        # Safety check: ensure widgets are still valid before accessing them
        try:
            _ = self.lbl_scalp_count.width()
        except RuntimeError:
            return

        # 1. Status & Footer
        txt = data.get("status_text", "")
        connected = data.get("connected", False)
        trade_allowed = data.get("trade_allowed", False)
        
        # Elide text to prevent layout expansion
        fm = QFontMetrics(self.status_label.font())
        elided_txt = fm.elidedText(txt, Qt.ElideRight, 400)
        self.status_label.setToolTip(txt)
        
        base_style = "font-size: 7pt; font-weight: bold; padding: 0px; border-radius: 3px;"
        if connected and not trade_allowed:
            self.status_label.setText("⚠️ AutoTrading OFF")
            self.status_label.setStyleSheet(f"color: #FDD663; background-color: #1E1F20; {base_style}")
        else:
            self.status_label.setText(elided_txt)
            if "Connected" in txt: self.status_label.setStyleSheet(f"color: #81C995; background-color: #1E1F20; {base_style}")
            elif any(x in txt for x in ["Error", "Disconnected", "Failed"]): self.status_label.setStyleSheet(f"color: #F28B82; background-color: #1E1F20; {base_style}")
            else: self.status_label.setStyleSheet(f"color: #FDD663; background-color: #1E1F20; {base_style}")

        # ATR
        atr = data.get("server_atr", 0.0)
        base_threshold = CONFIG.get("atr_high_vol_threshold", 2.0)
        tf = CONFIG.get("atr_timeframe", "M5")
        
        # Scale threshold based on timeframe to keep gauge meaningful
        tf_mult = {
            "M1": 0.4, "M5": 1.0, "M15": 1.6, "M30": 3.0,
            "H1": 5.0, "H4": 12.0, "D1": 20.0
        }.get(tf, 1.0)
        
        self.atr_gauge.set_value(atr, base_threshold * tf_mult, tf)

        # Ping
        ping_ms = data.get("ping", 0)
        self.lbl_latency.setText(f"Ping: {ping_ms} ms")
        if ping_ms < 100: self.lbl_latency.setStyleSheet("color: #81C995; font-size: 9pt;")
        elif ping_ms < 300: self.lbl_latency.setStyleSheet("color: #FDD663; font-size: 9pt;")
        else: self.lbl_latency.setStyleSheet("color: #F28B82; font-size: 9pt;")

        # Counts
        counts = data.get("counts", {"scalp": 0, "swing": 0, "manual": 0})
        sc, sw, mn = counts["scalp"], counts["swing"], counts["manual"]
        self.lbl_scalp_count.setText(f"Scalp: {sc}")
        self.lbl_swing_count.setText(f"Swing: {sw}")
        self.lbl_manual_count.setText(f"Manual: {mn}")
        
        base_style = "font-family: Consolas; font-size: 10pt; font-weight: bold;"
        dim_style = "color: #444746; font-family: Consolas; font-size: 10pt;"
        
        self.lbl_scalp_count.setStyleSheet(f"color: {'#81C995' if self.blink_state else '#66BB6A'}; {base_style}" if sc > 0 else dim_style)
        self.lbl_swing_count.setStyleSheet(f"color: {'#A8C7FA' if self.blink_state else '#669DF6'}; {base_style}" if sw > 0 else dim_style)
        self.lbl_manual_count.setStyleSheet(f"color: {'#FDD663' if self.blink_state else '#F28B82'}; {base_style}" if mn > 0 else dim_style)
        
        # Open P/L (Header)
        open_pl = data.get("open_pl", 0.0)
        color_hex = "#81C995" if open_pl >= 0 else "#F28B82"
        self.lbl_open_pl.setText(f"${open_pl:.2f}")
        self.lbl_open_pl.setStyleSheet(f"font-size: 28pt; font-weight: bold; color: {color_hex}; font-family: 'Segoe UI', sans-serif;")

        # Account Info
        balance = data.get("balance", 0.0)
        equity = data.get("equity", 0.0)
        
        margin_mode = data.get("margin_mode", -1)
        mode_str = "Unknown"
        if margin_mode == mt5.ACCOUNT_MARGIN_MODE_RETAIL_HEDGING: mode_str = "Hedging"
        elif margin_mode == mt5.ACCOUNT_MARGIN_MODE_RETAIL_NETTING: mode_str = "Netting"
        elif margin_mode == mt5.ACCOUNT_MARGIN_MODE_EXCHANGE: mode_str = "Exchange"
        
        self.lbl_account.setText(f"Balance: ${balance:.2f} | Equity: ${equity:.2f}")
        self.lbl_account.setToolTip(f"Account Balance | Equity\nAccount Mode: {mode_str}")

        # Update live price & quote details if available
        bid = data.get("bid")
        ask = data.get("ask")
        if bid is not None and ask is not None:
            mid = (bid + ask) / 2.0
            self.lbl_mid.setText(f"{mid:.3f}")
            
            # Daily Change Calculation
            daily_open = data.get("daily_open", 0.0)
            if daily_open > 0:
                delta = mid - daily_open
                pct = (delta / daily_open) * 100.0
                sign = "+" if delta >= 0 else ""
                color = "#81C995" if delta >= 0 else "#F28B82"
                self.lbl_delta.setText(f"Δ: {sign}{delta:.2f} ({sign}{pct:.2f}%)")
                self.lbl_delta.setStyleSheet(f"font-size: 9pt; font-weight: bold; color: {color}; padding-top: 4px;")
            else:
                self.lbl_delta.setText("Δ: --")
            
            self.lbl_bidask.setText(f"B:{bid:.3f} / A:{ask:.3f}")
            self.lbl_bidask.setStyleSheet(f"font-size: 9pt; color: {color}; font-family: Consolas;")

            try:
                spread = (ask - bid)
                self.lbl_spread.setText(f"Spread: {spread:.3f}")
            except Exception:
                self.lbl_spread.setText("Spread: --")
        
        self.update_session_display()

        # Portfolio snapshot if provided
        net_exposure = data.get("net_exposure")
        margin_used = data.get("margin_used")
        if net_exposure is not None or margin_used is not None:
            ne = net_exposure if net_exposure is not None else 0.0
            mu = margin_used if margin_used is not None else 0.0
            self.portfolio_stats.update_data(ne, mu, equity)

        # Strategy state
        modes = []
        if CONFIG.get("scalp_mode"): modes.append("Scalp")
        if CONFIG.get("swing_mode"): modes.append("Swing")
        if CONFIG.get("manage_manual"): modes.append("Manual")
        self.strategy_chips.set_modes(modes)

        # 2. Logs
        log_updates = data.get("log_updates", [])
        if log_updates:
            self.tree_logs.setUpdatesEnabled(False)
            for log in log_updates:
                item = QTreeWidgetItem([
                    log.get("time", ""),
                    str(log.get("ticket", "")),
                    log.get("type", ""),
                    log.get("details", "")
                ])
                # Color coding
                ltype = log.get("type", "")
                if "Error" in ltype or "Fail" in ltype:
                    item.setForeground(2, QColor("#F28B82"))
                elif "Open" in ltype or "Exec" in ltype:
                    item.setForeground(2, QColor("#81C995"))
                elif "Signal" in ltype:
                    item.setForeground(2, QColor("#FDD663"))
                    
                self.tree_logs.insertTopLevelItem(0, item)
            
            # Limit log size
            while self.tree_logs.topLevelItemCount() > 100:
                self.tree_logs.takeTopLevelItem(100)
            self.tree_logs.setUpdatesEnabled(True)

        # 3. Positions
        positions = data.get("positions", [])
        current_tickets = set()
        
        self.tree_positions.setUpdatesEnabled(False)
        
        for p in positions:
            ticket = p["ticket"]
            current_tickets.add(ticket)
            
            if ticket in self.position_items:
                item = self.position_items[ticket]
            else:
                item = QTreeWidgetItem()
                self.tree_positions.addTopLevelItem(item)
                self.position_items[ticket] = item
                
                # Inline Close Button
                btn_close = QPushButton("X")
                btn_close.setFixedSize(24, 20)
                btn_close.setCursor(Qt.PointingHandCursor)
                btn_close.setProperty("class", "Danger")
                btn_close.setStyleSheet("padding: 0px; font-size: 10px;")
                btn_close.clicked.connect(lambda _, t=ticket: self.close_ticket(t))
                self.tree_positions.setItemWidget(item, 8, btn_close)
                
                for c in range(9):
                    # Numeric right, others (Ticket, Symbol, Type) center
                    if c in [3, 4, 5, 6, 7]:
                        align = Qt.AlignRight
                    else:
                        align = Qt.AlignCenter
                    item.setTextAlignment(c, align | Qt.AlignVCenter)

            t_type = p["type"]
            magic = p["magic"]
            if magic == 0: 
                t_type += " (M)"
            elif magic == CONFIG["magic_number"]:
                t_type += " (Sc)"
            elif magic == CONFIG["magic_number"] + 1:
                t_type += " (Sw)"
            
            item.setText(0, str(ticket))
            item.setText(1, p['symbol'])
            item.setText(2, t_type)
            item.setText(3, f"{p['volume']:.2f}")
            item.setText(4, f"{p['price_open']:.2f}")
            item.setText(5, f"{p['sl']:.2f}")
            item.setText(6, f"{p['tp']:.2f}")
            item.setText(7, f"{p['profit']:.2f}")
            
            if p.get("is_pending", False):
                item.setText(7, "-")
                item.setForeground(7, QBrush(QColor("#E3E3E3")))
                item.setForeground(2, QBrush(QColor("#FDD663"))) # Highlight pending type
            else:
                color = QColor("#81C995") if p["profit"] >= 0 else QColor("#F28B82")
                item.setForeground(7, QBrush(color))
            
            if magic == 0: 
                item.setForeground(1, QBrush(QColor("#FDD663")))
            elif magic == CONFIG["magic_number"]:
                item.setForeground(1, QBrush(QColor("#81C995")))
            elif magic == CONFIG["magic_number"] + 1:
                item.setForeground(1, QBrush(QColor("#A8C7FA")))

        # Remove closed
        for ticket in list(self.position_items.keys()):
            if ticket not in current_tickets:
                item = self.position_items.pop(ticket)
                index = self.tree_positions.indexOfTopLevelItem(item)
                self.tree_positions.takeTopLevelItem(index)
        
        self.tree_positions.setUpdatesEnabled(True)

        # Open P/L
        self.lbl_tab_open_pl.setText(f"Total P/L: ${open_pl:.2f}")
        self.lbl_tab_open_pl.setStyleSheet(f"font-weight: bold; font-size: 10pt; color: {color_hex}; margin-top: 5px;")

        # 4. History
        history = data.get("history", [])
        hist_filter = self.combo_hist_filter.currentText()
        
        # Optimization: Only rebuild history if data changed or minute changed (for relative time)
        top_ts = history[0]['exit_time'] if history else 0
        current_sig = (len(history), top_ts, hist_filter, int(time.time() // 60))
        
        if current_sig != self._last_history_sig:
            self.tree_history.setUpdatesEnabled(False)
            self.tree_history.clear()
            
            for h in history:
                if hist_filter == "Profitable" and h["profit"] < 0:
                    continue
                if hist_filter == "Losing" and h["profit"] >= 0:
                    continue
                
                ago = time.time() - h['exit_time']
                if ago < 60:
                    dt_str = "<1m ago"
                elif ago < 3600:
                    dt_str = f"{int(ago // 60)}m ago"
                else:
                    dt_str = f"{int(ago // 3600)}h ago"
                
                dur = h.get("duration", 0)
                if dur < 60:
                    dur_str = f"{dur}s"
                elif dur < 3600:
                    dur_str = f"{dur//60}m {dur%60}s"
                else:
                    dur_str = f"{dur//3600}h {(dur%3600)//60}m"

                item = QTreeWidgetItem([
                    dt_str,
                    str(h["position_id"]), 
                    h["type"],
                    f"{h['volume']:.2f}", 
                    dur_str,
                    f"{h['profit']:.2f}"
                ])
                
                # Tooltip for exact time
                item.setToolTip(0, datetime.fromtimestamp(h['exit_time']).strftime("%Y-%m-%d %H:%M:%S"))
                
                color = QColor("#81C995") if h["profit"] >= 0 else QColor("#F28B82")
                item.setForeground(5, QBrush(color))
                item.setTextAlignment(0, Qt.AlignLeft | Qt.AlignVCenter)
                item.setTextAlignment(1, Qt.AlignLeft | Qt.AlignVCenter)
                item.setTextAlignment(2, Qt.AlignCenter | Qt.AlignVCenter)
                item.setTextAlignment(3, Qt.AlignRight | Qt.AlignVCenter)
                item.setTextAlignment(4, Qt.AlignCenter | Qt.AlignVCenter)
                item.setTextAlignment(5, Qt.AlignRight | Qt.AlignVCenter)
                
                self.tree_history.addTopLevelItem(item)
            
            self.tree_history.setUpdatesEnabled(True)
            self._last_history_sig = current_sig
        
        balance = data.get("balance", 0.0)
        total_pl_24h = data.get("total_pl_24h", 0.0)
        self.total_profit_label.setText(f"Bal: ${balance:.2f}  |  P/L (24h): ${total_pl_24h:.2f}")
        self.total_profit_label.setStyleSheet(f"font-weight: bold; font-size: 10pt; color: {'#81C995' if total_pl_24h >= 0 else '#F28B82'};")

        # Signal Flash
        last_ts = data.get("last_signal_ts", 0)
        diff = time.time() - last_ts
        if diff < 3.0:
            self.signal_indicator.set_active(int(diff * 4) % 2 == 0)
        else:
            self.signal_indicator.set_active(False)
            
        # Strategy Chips Flash
        if last_ts > self._last_signal_ts:
            self._last_signal_ts = last_ts
            if diff < 5.0: # Only flash if signal is recent
                sig_class = data.get("last_signal_class", "").lower()
                if "scalp" in sig_class: self.strategy_chips.flash("Scalp")
                elif "swing" in sig_class: self.strategy_chips.flash("Swing")

        # 5. Pass data to Chart if open
        if self.chart_window and self.chart_window.isVisible():
            self.chart_window.update_data(
                data.get("chart_data", []),
                positions,
                data.get("bid", 0),
                data.get("ask", 0),
                data.get("contract_size", 100),
                data.get("history", [])
            )

    def update_session_display(self):
        now = datetime.utcnow()
        h = now.hour
        
        # Define sessions: Name, Start, End (UTC), Color
        sessions = [
            {"name": "SYD", "start": 21, "end": 6, "color": "#C58AF9"},
            {"name": "TOK", "start": 0, "end": 9, "color": "#FDD663"},
            {"name": "LON", "start": 8, "end": 17, "color": "#81C995"},
            {"name": "NY", "start": 13, "end": 22, "color": "#F28B82"}
        ]
        
        active = []
        for s in sessions:
            is_active = False
            if s["start"] < s["end"]:
                if s["start"] <= h < s["end"]: is_active = True
            else: # Crosses midnight
                if h >= s["start"] or h < s["end"]: is_active = True
                
            if is_active:
                t_end = now.replace(hour=s["end"], minute=0, second=0, microsecond=0)
                if s["start"] > s["end"] and h >= s["start"]:
                    t_end += timedelta(days=1)
                
                left = t_end - now
                active.append({"name": s["name"], "color": s["color"], "left": left})
        
        if not active:
            self.session_panel.set_data("QUIET", "--:--", "#444746")
            self.session_panel.setToolTip("No major market session active")
            return

        active.sort(key=lambda x: x["left"])
        primary = active[0]
        
        names = "/".join([x["name"] for x in active])
        
        # Check Overlaps for Disabled Status
        overlap_warn = ""
        is_tok_lon = 8 <= h < 9
        is_lon_ny = 13 <= h < 17
        
        if is_tok_lon:
            sc = CONFIG.get("session_scalp_overlap_tok_lon", True)
            sw = CONFIG.get("session_swing_overlap_tok_lon", True)
            if not sc and not sw: overlap_warn = "⛔"
            elif not sc: overlap_warn = "⚠️Sc"
            elif not sw: overlap_warn = "⚠️Sw"
        elif is_lon_ny:
            sc = CONFIG.get("session_scalp_overlap_lon_ny", True)
            sw = CONFIG.get("session_swing_overlap_lon_ny", True)
            if not sc and not sw: overlap_warn = "⛔"
            elif not sc: overlap_warn = "⚠️Sc"
            elif not sw: overlap_warn = "⚠️Sw"
            
        if overlap_warn:
            names += f" {overlap_warn}"
        
        times = []
        tooltips = []
        for s in active:
            left_sec = int(s["left"].total_seconds())
            hours = left_sec // 3600
            mins = (left_sec % 3600) // 60
            times.append(f"{hours}:{mins:02d}")
            tooltips.append(f"{s['name']} closes in {hours}h {mins}m")
            
        if overlap_warn:
            tooltips.append(f"Overlap Status: {overlap_warn} (Trading Restricted)")
            
        text = " / ".join(times)
        c = primary["color"]
        
        self.session_panel.set_data(names, text, c)
        self.session_panel.setToolTip("\n".join(tooltips))

    def open_chart(self):
        if not self.chart_window:
            self.chart_window = ChartWindow()
            self.chart_window.closed.connect(lambda: self.toggle_chart_data_signal.emit(False))
        
        self.toggle_chart_data_signal.emit(True)
        self.chart_window.show()
        self.chart_window.raise_()
        self.chart_window.activateWindow()

    def open_manual(self):
        self.side_panel.tabs.setCurrentIndex(0)
        
    def open_calculator(self):
        self.side_panel.tabs.setCurrentIndex(1)

    def open_settings(self):
        dlg = AdvancedSettingsDialog(self)
        if dlg.exec():
            # Settings saved in CONFIG via dialog
            pass

    def show_context_menu(self, pos):
        item = self.tree_positions.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #1E1F20; color: #E3E3E3; } QMenu::item:selected { background-color: #A8C7FA; color: #000000; }")
            copy_action = menu.addAction("Copy Row")
            close_action = menu.addAction("Close Position")
            action = menu.exec(self.tree_positions.mapToGlobal(pos))
            if action == copy_action:
                text = " | ".join([item.text(i) for i in range(self.tree_positions.columnCount())])
                QApplication.clipboard().setText(text)
                ModernToast.show_message(self, "Row Copied to Clipboard", style="success")
            elif action == close_action:
                self.close_ticket(int(item.text(0)))

    def show_log_context_menu(self, pos):
        item = self.tree_logs.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #1E1F20; color: #E3E3E3; } QMenu::item:selected { background-color: #A8C7FA; color: #000000; }")
            copy_action = menu.addAction("Copy Log")
            action = menu.exec(self.tree_logs.mapToGlobal(pos))
            if action == copy_action:
                text = f"[{item.text(0)}] Ticket:{item.text(1)} Type:{item.text(2)} - {item.text(3)}"
                QApplication.clipboard().setText(text)
                ModernToast.show_message(self, "Log Copied to Clipboard", style="success")

    def show_history_context_menu(self, pos):
        item = self.tree_history.itemAt(pos)
        if item:
            menu = QMenu(self)
            menu.setStyleSheet("QMenu { background-color: #1E1F20; color: #E3E3E3; } QMenu::item:selected { background-color: #A8C7FA; color: #000000; }")
            copy_action = menu.addAction("Copy Row")
            action = menu.exec(self.tree_history.mapToGlobal(pos))
            if action == copy_action:
                text = " | ".join([item.text(i) for i in range(self.tree_history.columnCount())])
                QApplication.clipboard().setText(text)
                ModernToast.show_message(self, "History Row Copied", style="success")

    def on_trade_selected(self):
        items = self.tree_positions.selectedItems()
        if not items: return
        ticket_str = items[0].text(0)
        if not ticket_str.isdigit(): return
        positions = mt5.positions_get(ticket=int(ticket_str))
        if positions:
            self.spin_update_sl.setValue(positions[0].sl)
            self.spin_update_tp.setValue(positions[0].tp)

    def copy_price_to_field(self, field):
        tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
        if tick: 
            field.setValue(tick.bid)
            ModernToast.show_message(self, f"Price Copied: {tick.bid}", style="info")

    def close_ticket(self, ticket):
        # Try closing position
        positions = mt5.positions_get(ticket=ticket)
        if positions: 
            self._send_close_request(positions[0])
            ModernToast.show_message(self, f"Close Request Sent: #{ticket}", style="info")
            return
            
        # Try cancelling order
        orders = mt5.orders_get(ticket=ticket)
        if orders:
            self._send_cancel_request(orders[0])
            ModernToast.show_message(self, f"Cancel Request Sent: #{ticket}", style="info")

    def _send_cancel_request(self, order):
        req = {
            "action": mt5.TRADE_ACTION_REMOVE,
            "order": order.ticket,
            "symbol": order.symbol
        }
        mt5.order_send(req)

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
        mt5.order_send(req)

    def execute_close_action(self):
        action = self.close_combo.currentText()
        mode = ""
        if action == "Close All Scalp": mode = "scalp"
        elif action == "Close All Swing": mode = "swing"
        elif action == "Close All Manual": mode = "manual"
        elif action == "Close Scalp Winners": mode = "scalp_profit"
        elif action == "Close Scalp Losers": mode = "scalp_loss"
        
        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        if not positions: return
        
        if action == "Close ALL Positions":
            for pos in positions:
                self._send_close_request(pos)
            ModernToast.show_message(self, "ALL POSITIONS CLOSED", style="error")
            return

        for pos in positions:
            is_scalp = pos.magic == CONFIG["magic_number"]
            is_swing = pos.magic == CONFIG["magic_number"] + 1
            is_manual = pos.magic == 0
            
            if mode == "scalp" and is_scalp: self._send_close_request(pos)
            elif mode == "swing" and is_swing: self._send_close_request(pos)
            elif mode == "manual" and is_manual: self._send_close_request(pos)
            elif mode == "scalp_profit" and is_scalp and pos.profit > 0: self._send_close_request(pos)
            elif mode == "scalp_loss" and is_scalp and pos.profit < 0: self._send_close_request(pos)
        ModernToast.show_message(self, f"Executed: {action}", style="info")

    def _send_sltp_update(self, pos, sl, tp):
        req = {"action": mt5.TRADE_ACTION_SLTP, "position": pos.ticket, "sl": float(sl), "tp": float(tp), "symbol": pos.symbol}
        mt5.order_send(req)

    def update_manual_sltp(self):
        sl = self.spin_update_sl.value()
        tp = self.spin_update_tp.value()
        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        if positions:
            for pos in positions:
                if pos.magic == 0: self._send_sltp_update(pos, sl, tp)
            ModernToast.show_message(self, "Manual Trades Updated", style="success")

    def update_selected_sltp(self):
        sl = self.spin_update_sl.value()
        tp = self.spin_update_tp.value()
        items = self.tree_positions.selectedItems()
        if items:
            ticket = int(items[0].text(0))
            positions = mt5.positions_get(ticket=ticket)
            if positions: 
                self._send_sltp_update(positions[0], sl, tp)
                ModernToast.show_message(self, f"Trade #{ticket} Updated", style="success")

    def export_logs_to_csv(self):
        filename, _ = QFileDialog.getSaveFileName(self, "Export Logs", "logs_export.csv", "CSV Files (*.csv)")
        if filename:
            with open(filename, "w", newline="", encoding="utf-8") as f:
                writer = csv.writer(f)
                writer.writerow(["Time", "Ticket", "Type", "Details"])
                root = self.tree_logs.invisibleRootItem()
                for i in range(root.childCount()):
                    item = root.child(i)
                    writer.writerow([item.text(0), item.text(1), item.text(2), item.text(3)])
            ModernToast.show_message(self, "Logs Exported Successfully", style="success")

    def clear_logs(self):
        self.tree_logs.clear()
        ModernToast.show_message(self, "Logs Cleared", style="info")

    def export_performance_csv(self):
        src = "strategy_performance.csv"
        if os.path.exists(src):
            filename, _ = QFileDialog.getSaveFileName(self, "Export Performance", "performance_export.csv", "CSV Files (*.csv)")
            if filename: 
                shutil.copy2(src, filename)
                ModernToast.show_message(self, "Performance Exported", style="success")

    def clear_performance_csv(self):
        if os.path.exists("strategy_performance.csv"):
            if QMessageBox.question(self, "Confirm", "Clear history?") == QMessageBox.Yes:
                os.remove("strategy_performance.csv")
                ModernToast.show_message(self, "History Cleared", style="info")

    def on_position_dbl_click(self, item, column):
        ticket = item.text(0)
        symbol = item.text(1)
        try:
            sl = float(item.text(5))
            tp = float(item.text(6))
        except ValueError:
            sl = 0.0
            tp = 0.0
            
        dlg = PositionModifyDialog(ticket, symbol, sl, tp, self)
        dlg.exec()

    def closeEvent(self, event):
        self._is_closing = True
        self.blink_timer.stop()
        try:
            self.worker.data_updated.disconnect(self.update_ui)
        except Exception:
            pass

        data = {
            "geometry": self.saveGeometry().toBase64().data().decode(),
            "state": self.saveState().toBase64().data().decode(),
            "chart_open": bool(self.chart_window and self.chart_window.isVisible())
        }
        with open("ui_state.json", "w") as f: json.dump(data, f)
        self.stop_worker_signal.emit()
        self.worker_thread.quit()
        self.worker_thread.wait()
        if self.chart_window:
            self.chart_window.close()
        super().closeEvent(event)

    def load_signals(self, page):
        self.btn_sig_refresh.setEnabled(False)
        self.status_label.setText(f"Loading Signals Page {page}...")
        
        self.worker_sig = SignalHistoryWorker(page, self.sig_limit)
        self.worker_sig.data_received.connect(self.on_signals_loaded)
        self.worker_sig.error_occurred.connect(self.on_signals_error)
        self.worker_sig.finished.connect(lambda: self.btn_sig_refresh.setEnabled(True))
        self.worker_sig.start()

    def on_signals_loaded(self, data):
        self.current_signals_list = data.get("signals", [])
        self.sig_page = data.get("page", 1)
        
        self.lbl_sig_page.setText(f"Page {self.sig_page}")
        self.render_signals()
        
        self.btn_sig_prev.setEnabled(self.sig_page > 1)
        self.btn_sig_next.setEnabled(len(self.current_signals_list) == self.sig_limit)
        self.status_label.setText("Signals Loaded")

    def render_signals(self):
        self.tree_signals.clear()
        filter_txt = self.combo_sig_filter.currentText().lower()
        
        for s in self.current_signals_list:
            s_class = s.get("classification", "").lower()
            if filter_txt != "all" and filter_txt not in s_class:
                continue

            ts = s.get("createdAt", 0)
            dt = datetime.fromtimestamp(ts).strftime("%Y-%m-%d %H:%M")
            
            # Format Type (Check for Limit)
            e_type = s.get("entryType", "").upper()
            rec_type = s.get("recommendedOrderType", "").lower()
            if "limit" in rec_type:
                e_type = f"LIMIT {e_type}"
            
            item = QTreeWidgetItem([
                dt,
                s.get("signalId", ""),
                e_type,
                s.get("classification", ""),
                str(s.get("entryPrice", "")),
                f"{s.get('convictionScore', 0):.1f}"
            ])
            
            # Color coding
            if "long" in s.get("entryType", ""):
                item.setForeground(2, QBrush(QColor("#81C995")))
            elif "short" in s.get("entryType", ""):
                item.setForeground(2, QBrush(QColor("#F28B82")))
                
            item.setData(0, Qt.UserRole, s) # Store full data
            self.tree_signals.addTopLevelItem(item)

    def on_signals_error(self, err):
        QMessageBox.warning(self, "Signal Error", f"Failed to load signals: {err}")
        self.status_label.setText("Signal Load Failed")

    def prev_sig_page(self):
        if self.sig_page > 1:
            self.load_signals(self.sig_page - 1)

    def next_sig_page(self):
        self.load_signals(self.sig_page + 1)

    def on_signal_dbl_click(self, item, col):
        data = item.data(0, Qt.UserRole)
        if data:
            dlg = SignalDetailsDialog(data, self)
            dlg.exec()