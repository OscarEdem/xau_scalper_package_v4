import os
import json
import time
import bisect
from datetime import datetime
import MetaTrader5 as mt5 # type: ignore
from PySide6.QtWidgets import (QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,  # type: ignore
                               QLabel, QComboBox, QPushButton, QSizePolicy, QMenu, QFileDialog, QApplication, QToolTip, QMessageBox, QFrame, QDialog, QScrollArea, QColorDialog, QInputDialog)
from PySide6.QtCore import Qt, QByteArray, QRectF, QPointF, Signal # type: ignore
from PySide6.QtGui import QColor, QPainter, QBrush, QPen, QPainterPath, QFont, QPolygonF # type: ignore
from config import CONFIG, state, save_config
from gui_styles import GLOBAL_STYLESHEET
from gui_dialogs import ModernToast

class CandleChartWidget(QWidget):
    tool_finished = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.candles = []
        self.history = []
        self.positions = []
        self.bid = 0.0
        self.ask = 0.0
        self.setMinimumHeight(250)
        self.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Expanding)
        self.setCursor(Qt.CrossCursor)
        self.setMouseTracking(True)
        self.cursor_pos = None
        self.mode = "candle" # "candle" or "line"
        self.scroll_offset = 0
        self.visible_count = 80
        # Start with crosshair disabled by default (pointer cursor)
        self.crosshair_enabled = False
        self.is_dragging = False
        self.last_drag_x = 0
        self.last_drag_y = 0
        self.chart_shift = True
        self.scroll_offset = -int(self.visible_count * 0.2)
        self.vertical_zoom = 1.0
        self.is_scaling = False
        self.last_scale_y = 0
        self.contract_size = 100.0
        self.show_sr = False
        self.show_structure = False
        # Whether to draw small pivot dots when structure is shown
        self.show_pivot_dots = False
        self.show_be = False
        self.show_history = False
        self.history_hotspots = []
        self.drag_trade = None # { 'ticket': int, 'type': 'SL'|'TP'|'PRICE', 'current_val': float }
        self.magnet_mode = False
        
        # Risk/Reward Tool State
        self.rr_active = False
        self.rr_drawing = False
        self.rr_start_price = None
        self.rr_sl_price = None
        self.rr_start_time = None
        self.rr_ratio = CONFIG.get("chart_rr_ratio", 1.5)
        self.rr_drawings = [] # List of dicts: {'entry': float, 'sl': float, 'tp': float}
        self.rr_drag_idx = -1
        self.rr_drag_handle = None # 'entry', 'sl', 'tp'
        self.rr_drag_offset = 0.0
        self.measure_data = None
        # General drawing tools (trendlines, hline, vline)
        self.drawings = []  # List of dicts: {'type':'trend'|'h'|'v', 'x1':ts, 'y1':price, 'x2':ts, 'y2':price, 'style':{}}
        self.draw_tool = None  # 'trend', 'hline', 'vline' or None
        self.drawing_temp = None  # temporary drawing while dragging
        self.draw_drag_idx = -1
        self.load_drawings()
        self.last_min_price = 0.0
        self.last_price_range = 1.0
        self.last_h = 100
        self.load_rr_drawings()
        
    def update_data(self, candles, positions=None, bid=0.0, ask=0.0, contract_size=100.0, history=None):
        # Optimization: Check if update is actually needed to avoid excessive repaints
        need_update = False
        if not self.candles and candles: need_update = True
        elif self.candles and candles and (self.candles[-1]['time'] != candles[-1]['time'] or self.candles[-1]['close'] != candles[-1]['close']): need_update = True
        elif abs(self.bid - bid) > 1e-5 or abs(self.ask - ask) > 1e-5: need_update = True
        elif len(self.positions) != len(positions if positions else []): need_update = True
        elif self.history != history: need_update = True

        self.candles = candles
        self.positions = positions if positions else []
        self.bid = bid
        self.ask = ask
        self.contract_size = contract_size
        self.history = history if history else []
        
        if need_update or self.is_dragging or self.is_scaling:
            self.update()

    def load_rr_drawings(self):
        if os.path.exists("rr_drawings.json"):
            try:
                with open("rr_drawings.json", "r") as f:
                    self.rr_drawings = json.load(f)
                # Migration for legacy drawings (add timestamps)
                changed = False
                for rr in self.rr_drawings:
                    if 'start_ts' not in rr:
                        rr['start_ts'] = int(time.time())
                        rr['end_ts'] = int(time.time() + 1800) # +30 mins default
                        changed = True
                if changed: self.save_rr_drawings()
            except:
                self.rr_drawings = []

    def load_drawings(self):
        if os.path.exists("drawings.json"):
            try:
                with open("drawings.json", "r") as f:
                    self.drawings = json.load(f)
            except:
                self.drawings = []

    def save_drawings(self):
        try:
            with open("drawings.json", "w") as f:
                json.dump(self.drawings, f)
        except:
            pass

    def save_rr_drawings(self):
        try:
            with open("rr_drawings.json", "w") as f:
                json.dump(self.rr_drawings, f)
        except:
            pass

    def clear_all_drawings(self):
        self.drawings = []
        self.rr_drawings = []
        self.save_drawings()
        self.save_rr_drawings()
        self.update()

    def toggle_sr(self, state):
        self.show_sr = state
        self.update()

    def toggle_structure(self, state):
        self.show_structure = state
        self.update()

    def toggle_be(self, state):
        self.show_be = state
        self.update()

    def toggle_history(self, state):
        self.show_history = state
        self.update()
        
    def toggle_rr_tool(self, state):
        self.rr_active = state
        if not state:
            self.rr_start_price = None
            self.rr_start_time = None
        self.update()

    def toggle_magnet(self, state):
        self.magnet_mode = state
        self.update()

    def toggle_draw_tool(self, tool_name):
        # tool_name: 'trend', 'hline', 'vline' or None
        if self.draw_tool == tool_name:
            self.draw_tool = None
        else:
            self.draw_tool = tool_name
        # clear any temp
        self.drawing_temp = None
        self.update()

    def set_mode(self, mode):
        self.mode = mode
        self.update()

    def set_chart_shift(self, enabled):
        self.chart_shift = enabled
        if enabled:
            self.scroll_offset = -int(self.visible_count * 0.2)
        elif self.scroll_offset < 0:
            self.scroll_offset = 0
        self.update()

    def set_crosshair_mode(self, enabled):
        self.crosshair_enabled = enabled
        self.setCursor(Qt.CrossCursor if enabled else Qt.ArrowCursor)
        self.update()

    def reset_view(self):
        self.scroll_offset = 0
        self.vertical_zoom = 1.0
        self.update()

    def zoom_in(self):
        self.visible_count = max(10, self.visible_count - 5)
        self.update()

    def zoom_out(self):
        self.visible_count = min(500, self.visible_count + 5)
        self.update()

    def get_timeframe_seconds(self):
        tf = CONFIG.get("chart_timeframe", "M1")
        mapping = {"M1": 60, "M5": 300, "M15": 900, "M30": 1800, "H1": 3600, "H4": 14400, "D1": 86400}
        return mapping.get(tf, 60)

    def get_x_from_time(self, ts):
        if not self.candles: return 0
        
        times = [c['time'] for c in self.candles]
        if not times: return 0
        
        if ts > times[-1]:
            interval = self.get_timeframe_seconds()
            diff_sec = ts - times[-1]
            idx = len(self.candles) - 1 + (diff_sec / interval)
        elif ts < times[0]:
             interval = self.get_timeframe_seconds()
             diff_sec = times[0] - ts
             idx = -(diff_sec / interval)
        else:
            idx = bisect.bisect_left(times, ts)
            
        w = self.width() - 60
        candle_w = w / self.visible_count if self.visible_count > 0 else 0
        total = len(self.candles)
        end_idx = total - self.scroll_offset
        start_idx = end_idx - self.visible_count
        
        x = (idx - start_idx) * candle_w + candle_w / 2
        return x

    def get_time_from_x(self, x):
        w = self.width() - 60
        if w <= 0 or self.visible_count <= 0 or not self.candles:
            return int(time.time())
        
        candle_w = w / self.visible_count
        total = len(self.candles)
        end_idx = total - self.scroll_offset
        start_idx = end_idx - self.visible_count
        
        slot_idx = (x / candle_w) - 0.5
        idx = start_idx + slot_idx
        
        interval = self.get_timeframe_seconds()
        
        if idx < 0:
            return int(self.candles[0]['time'] + (idx * interval))
        elif idx >= len(self.candles):
            diff = idx - (len(self.candles) - 1)
            return int(self.candles[-1]['time'] + (diff * interval))
        else:
            i = int(round(idx))
            if 0 <= i < len(self.candles):
                return self.candles[i]['time']
            return int(self.candles[-1]['time'])

    def get_price_from_y(self, y):
        if self.last_h <= 20: return 0.0
        draw_h = self.last_h - 20
        # Inverse of: y = h - (pct * (h - 20)) - 10
        pct = (self.last_h - 10 - y) / draw_h
        return self.last_min_price + (pct * self.last_price_range)

    def get_y_from_price(self, price):
        if self.last_price_range == 0: return 0
        pct = (price - self.last_min_price) / self.last_price_range
        return self.last_h - (pct * (self.last_h - 20)) - 10

    def is_near_line(self, y_mouse, price_line, x_mouse=None, x_start=None, x_end=None):
        if price_line is None: return False
        y_line = self.get_y_from_price(price_line)
        
        # If x bounds provided, check them
        if x_mouse is not None and x_start is not None and x_end is not None:
            if not (x_start <= x_mouse <= x_end): return False
            
        return abs(y_mouse - y_line) < 8  # 8px tolerance

    def _distance_point_to_segment(self, px, py, x1, y1, x2, y2):
        # Euclidean distance from point (px,py) to segment (x1,y1)-(x2,y2)
        dx = x2 - x1
        dy = y2 - y1
        if dx == 0 and dy == 0:
            return ((px - x1) ** 2 + (py - y1) ** 2) ** 0.5
        t = ((px - x1) * dx + (py - y1) * dy) / (dx * dx + dy * dy)
        t = max(0.0, min(1.0, t))
        proj_x = x1 + t * dx
        proj_y = y1 + t * dy
        return ((px - proj_x) ** 2 + (py - proj_y) ** 2) ** 0.5

    def get_candle_at_x(self, x):
        w = self.width() - 60
        if w <= 0 or self.visible_count <= 0 or not self.candles:
            return None
        
        candle_w = w / self.visible_count
        total = len(self.candles)
        end_idx = total - self.scroll_offset
        start_idx = end_idx - self.visible_count
        
        slot_idx = int(x / candle_w)
        idx = int(start_idx + slot_idx)
        
        if 0 <= idx < total:
            return self.candles[idx]
        return None

    def get_snapped_price(self, event_pos):
        raw_price = self.get_price_from_y(event_pos.y())
        if not self.magnet_mode:
            return raw_price
            
        candle = self.get_candle_at_x(event_pos.x())
        if candle:
            dist_high = abs(candle['high'] - raw_price)
            dist_low = abs(candle['low'] - raw_price)
            if dist_high < dist_low:
                return candle['high']
            else:
                return candle['low']
        return raw_price

    def contextMenuEvent(self, event):
        menu = QMenu(self)

        # Copy Price
        price = self.get_price_from_y(event.y())
        copy_action = menu.addAction(f"Copy Price ({price:.2f})")

        # Snapshot
        snap_action = menu.addAction("Save Snapshot")

        # Check Measurement
        delete_measure = False
        if self.measure_data:
            dt = self.measure_data
            try:
                x1 = self.get_x_from_time(dt.get('x1', 0))
                raw_x2 = dt.get('x2')
                x2 = self.get_x_from_time(raw_x2 if raw_x2 is not None else dt.get('x1', 0))
                y1 = self.get_y_from_price(dt.get('y1', 0))
                raw_y2 = dt.get('y2')
                y2 = self.get_y_from_price(raw_y2 if raw_y2 is not None else dt.get('y1', 0))
                
                rect = QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized()
                if rect.contains(event.pos()):
                    delete_measure = True
            except:
                pass

        # Detect if right-click is near an RR drawing or generic drawing
        delete_rr_idx = -1
        tol = 10
        for i in range(len(self.rr_drawings) - 1, -1, -1):
            rr = self.rr_drawings[i]
            xs = self.get_x_from_time(rr.get('start_ts', 0))
            xe = self.get_x_from_time(rr.get('end_ts', 0))
            if xs > xe: xs, xe = xe, xs

            # compute pixel y for each line and distance to segments
            try:
                y_entry = self.get_y_from_price(rr['entry'])
                y_sl = self.get_y_from_price(rr['sl'])
                y_tp = self.get_y_from_price(rr['tp'])
            except Exception:
                continue

            d_entry = self._distance_point_to_segment(event.x(), event.y(), xs, y_entry, xe, y_entry)
            d_sl = self._distance_point_to_segment(event.x(), event.y(), xs, y_sl, xe, y_sl)
            d_tp = self._distance_point_to_segment(event.x(), event.y(), xs, y_tp, xe, y_tp)

            if min(d_entry, d_sl, d_tp) <= tol:
                delete_rr_idx = i
                break

        delete_draw_idx = -1
        tol = 8
        for i in range(len(self.drawings) - 1, -1, -1):
            dr = self.drawings[i]
            t = dr.get('type')
            try:
                if t == 'trend':
                    x1 = self.get_x_from_time(dr.get('x1', 0))
                    x2 = self.get_x_from_time(dr.get('x2', 0))
                    y1 = self.get_y_from_price(dr.get('y1', 0))
                    y2 = self.get_y_from_price(dr.get('y2', 0))
                    dx = x2 - x1
                    dy = y2 - y1
                    if dx == 0 and dy == 0:
                        dist = ((event.x() - x1)**2 + (event.y() - y1)**2) ** 0.5
                    else:
                        tproj = ((event.x() - x1) * dx + (event.y() - y1) * dy) / (dx*dx + dy*dy)
                        tproj = max(0.0, min(1.0, tproj))
                        px = x1 + tproj * dx
                        py = y1 + tproj * dy
                        dist = ((event.x() - px)**2 + (event.y() - py)**2) ** 0.5
                    if dist <= tol:
                        delete_draw_idx = i
                        break
                elif t == 'hline':
                    yline = self.get_y_from_price(dr.get('y1', 0))
                    if abs(event.y() - yline) <= tol:
                        delete_draw_idx = i
                        break
                elif t == 'vline':
                    xline = self.get_x_from_time(dr.get('x1', 0))
                    if abs(event.x() - xline) <= tol:
                        delete_draw_idx = i
                        break
                elif t == 'fib':
                    # Treat like trendline for diagonal selection
                    x1 = self.get_x_from_time(dr.get('x1', 0))
                    x2 = self.get_x_from_time(dr.get('x2', 0))
                    y1 = self.get_y_from_price(dr.get('y1', 0))
                    y2 = self.get_y_from_price(dr.get('y2', 0))
                    dx = x2 - x1
                    dy = y2 - y1
                    if dx == 0 and dy == 0:
                        dist = ((event.x() - x1)**2 + (event.y() - y1)**2) ** 0.5
                    else:
                        tproj = ((event.x() - x1) * dx + (event.y() - y1) * dy) / (dx*dx + dy*dy)
                        tproj = max(0.0, min(1.0, tproj))
                        px = x1 + tproj * dx
                        py = y1 + tproj * dy
                        dist = ((event.x() - px)**2 + (event.y() - py)**2) ** 0.5
                    if dist <= tol:
                        delete_draw_idx = i
                        break
                elif t == 'rect':
                    # Check if point inside rect
                    x1 = self.get_x_from_time(dr.get('x1', 0))
                    x2 = self.get_x_from_time(dr.get('x2', 0))
                    y1 = self.get_y_from_price(dr.get('y1', 0))
                    y2 = self.get_y_from_price(dr.get('y2', 0))
                    rect = QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized()
                    if rect.contains(event.pos()):
                        delete_draw_idx = i
                        break
            except Exception:
                continue

        # Lock/Unlock Actions
        lock_action = None
        unlock_action = None
        color_action = None
        text_action = None
        delete_action = None
        measure_action = None
        
        if delete_rr_idx != -1 or delete_draw_idx != -1:
            is_locked = False
            if delete_rr_idx != -1:
                is_locked = self.rr_drawings[delete_rr_idx].get('locked', False)
            else:
                is_locked = self.drawings[delete_draw_idx].get('locked', False)
            
            color_action = menu.addAction("Change Color")
            text_action = menu.addAction("Set Text")
            menu.addSeparator()
            
            if is_locked:
                unlock_action = menu.addAction("Unlock Object")
            else:
                lock_action = menu.addAction("Lock Object")
                delete_action = menu.addAction("Delete Object")
        
        if delete_measure:
            measure_action = menu.addAction("Delete Measurement")

        action = menu.exec(event.globalPos())

        if delete_measure and action == measure_action:
            self.measure_data = None
            self.update()
            return

        if action == copy_action:
            QApplication.clipboard().setText(f"{price:.2f}")
            ModernToast.show_message(self, f"Price {price:.2f} Copied", style="success")
        elif action == snap_action:
            self.save_snapshot()
        elif action == color_action:
            curr_color = QColor("#D08770")
            if delete_rr_idx != -1:
                c = self.rr_drawings[delete_rr_idx].get('color')
                if c: curr_color = QColor(c)
            else:
                c = self.drawings[delete_draw_idx].get('color')
                if c: curr_color = QColor(c)
            
            c = QColorDialog.getColor(curr_color, self, "Select Color")
            if c.isValid():
                if delete_rr_idx != -1: self.rr_drawings[delete_rr_idx]['color'] = c.name()
                else: self.drawings[delete_draw_idx]['color'] = c.name()
                self.save_drawings(); self.save_rr_drawings(); self.update()
        elif action == text_action:
            curr_text = ""
            if delete_rr_idx != -1: curr_text = self.rr_drawings[delete_rr_idx].get('text', "")
            else: curr_text = self.drawings[delete_draw_idx].get('text', "")
            
            text, ok = QInputDialog.getText(self, "Object Text", "Enter text:", text=curr_text)
            if ok:
                if delete_rr_idx != -1: self.rr_drawings[delete_rr_idx]['text'] = text
                else: self.drawings[delete_draw_idx]['text'] = text
                self.save_drawings(); self.save_rr_drawings(); self.update()
        elif action == lock_action:
            if delete_rr_idx != -1: self.rr_drawings[delete_rr_idx]['locked'] = True
            else: self.drawings[delete_draw_idx]['locked'] = True
            self.save_drawings(); self.save_rr_drawings()
        elif action == unlock_action:
            if delete_rr_idx != -1: self.rr_drawings[delete_rr_idx]['locked'] = False
            else: self.drawings[delete_draw_idx]['locked'] = False
            self.save_drawings(); self.save_rr_drawings()
        elif action == delete_action:
            if delete_rr_idx != -1:
                self.rr_drawings.pop(delete_rr_idx)
                self.save_rr_drawings()
            elif delete_draw_idx != -1:
                self.drawings.pop(delete_draw_idx)
                self.save_drawings()
            ModernToast.show_message(self, "Object Deleted", style="info")
            self.update()

    def mousePressEvent(self, event):
        if event.button() == Qt.LeftButton:
            if event.x() > self.width() - 60:
                self.is_scaling = True
                self.last_scale_y = event.y()
                self.setCursor(Qt.SizeVerCursor)
                return
            
            # Check for Trade Line Dragging (SL/TP/Pending Price)
            y = event.y()
            min_dist = 10
            target = None
            
            for pos in self.positions:
                # Check SL
                if pos['sl'] > 0:
                    y_sl = self.get_y_from_price(pos['sl'])
                    if abs(y - y_sl) < min_dist:
                        target = {'ticket': pos['ticket'], 'type': 'SL', 'current_val': pos['sl'], 'is_pending': pos.get('is_pending', False)}
                        min_dist = abs(y - y_sl)
                # Check TP
                if pos['tp'] > 0:
                    y_tp = self.get_y_from_price(pos['tp'])
                    if abs(y - y_tp) < min_dist:
                        target = {'ticket': pos['ticket'], 'type': 'TP', 'current_val': pos['tp'], 'is_pending': pos.get('is_pending', False)}
                        min_dist = abs(y - y_tp)
                # Check Entry (Pending Only)
                if pos.get('is_pending', False):
                    y_price = self.get_y_from_price(pos['price_open'])
                    if abs(y - y_price) < min_dist:
                        target = {'ticket': pos['ticket'], 'type': 'PRICE', 'current_val': pos['price_open'], 'is_pending': True}
                        min_dist = abs(y - y_price)
            
            if target:
                self.drag_trade = target
                self.setCursor(Qt.SizeVerCursor)
                return

            if self.draw_tool in ("trend", "hline", "vline", "fib", "rect", "measure"):
                if self.draw_tool == "measure":
                    self.measure_data = None
                    self.update()
                # If a drawing tool is active, start a new drawing
                self.drawing_temp = {'type': self.draw_tool, 'x1': self.get_time_from_x(event.x()), 'y1': self.get_snapped_price(event.pos()), 'x2': None, 'y2': None}
                return
            
            # Check for resizing existing R:R
            y = event.y()
            x = event.x()
            p_mouse = self.get_price_from_y(y)
            for i, rr in enumerate(self.rr_drawings):
                xs = self.get_x_from_time(rr.get('start_ts', 0))
                xe = self.get_x_from_time(rr.get('end_ts', 0))
                
                # Check Width Resize (Right Edge)
                y_min = min(self.get_y_from_price(rr['tp']), self.get_y_from_price(rr['sl']))
                y_max = max(self.get_y_from_price(rr['tp']), self.get_y_from_price(rr['sl']))
                if abs(x - xe) < 10 and y_min <= y <= y_max:
                    if rr.get('locked', False): return
                    self.rr_drag_idx = i
                    self.rr_drag_handle = 'width'
                    return

                if self.is_near_line(y, rr['tp'], x, xs, xe):
                    if rr.get('locked', False): return
                    self.rr_drag_idx = i
                    self.rr_drag_handle = 'tp'
                    return
                elif self.is_near_line(y, rr['sl'], x, xs, xe):
                    if rr.get('locked', False): return
                    self.rr_drag_idx = i
                    self.rr_drag_handle = 'sl'
                    return
                elif self.is_near_line(y, rr['entry'], x, xs, xe):
                    if rr.get('locked', False): return
                    self.rr_drag_idx = i
                    self.rr_drag_handle = 'entry'
                    self.rr_drag_offset = rr['entry'] - p_mouse
                    return
            
            # Check for Generic Drawing Drag
            tol = 8
            for i in range(len(self.drawings) - 1, -1, -1):
                dr = self.drawings[i]
                if dr.get('locked', False): continue
                
                t = dr.get('type')
                hit = False
                try:
                    if t == 'trend' or t == 'fib':
                        x1 = self.get_x_from_time(dr.get('x1', 0))
                        x2 = self.get_x_from_time(dr.get('x2', 0))
                        y1 = self.get_y_from_price(dr.get('y1', 0))
                        y2 = self.get_y_from_price(dr.get('y2', 0))
                        dx = x2 - x1
                        dy = y2 - y1
                        if dx == 0 and dy == 0:
                            dist = ((x - x1)**2 + (y - y1)**2) ** 0.5
                        else:
                            tproj = ((x - x1) * dx + (y - y1) * dy) / (dx*dx + dy*dy)
                            tproj = max(0.0, min(1.0, tproj))
                            px = x1 + tproj * dx
                            py = y1 + tproj * dy
                            dist = ((x - px)**2 + (y - py)**2) ** 0.5
                        if dist <= tol: hit = True
                    elif t == 'hline':
                        yline = self.get_y_from_price(dr.get('y1', 0))
                        if abs(y - yline) <= tol: hit = True
                    elif t == 'vline':
                        xline = self.get_x_from_time(dr.get('x1', 0))
                        if abs(x - xline) <= tol: hit = True
                    elif t == 'rect':
                        x1 = self.get_x_from_time(dr.get('x1', 0))
                        x2 = self.get_x_from_time(dr.get('x2', 0))
                        y1 = self.get_y_from_price(dr.get('y1', 0))
                        y2 = self.get_y_from_price(dr.get('y2', 0))
                        rect = QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized()
                        if rect.contains(event.pos()): hit = True
                except:
                    continue
                
                if hit:
                    self.draw_drag_idx = i
                    self.last_drag_x = event.x()
                    self.last_drag_y = event.y()
                    self.setCursor(Qt.SizeAllCursor)
                    return

            if self.rr_active:
                self.rr_drawing = True
                p = self.get_snapped_price(event.pos())
                self.rr_start_price = p
                self.rr_start_time = self.get_time_from_x(event.x())
                self.rr_sl_price = p
                self.update()
                return
            self.is_dragging = True
            self.last_drag_x = event.x()
            self.last_drag_y = event.y()
            if not self.crosshair_enabled:
                self.setCursor(Qt.ClosedHandCursor)

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.LeftButton:
            if self.drag_trade:
                # Execute Modification
                ticket = self.drag_trade['ticket']
                mod_type = self.drag_trade['type']
                new_val = self.drag_trade['current_val']
                is_pending = self.drag_trade['is_pending']
                
                # Find current values to preserve what wasn't changed
                target_pos = next((p for p in self.positions if p['ticket'] == ticket), None)
                
                if target_pos:
                    sl = new_val if mod_type == 'SL' else float(target_pos['sl'])
                    tp = new_val if mod_type == 'TP' else float(target_pos['tp'])
                    price = new_val if mod_type == 'PRICE' else float(target_pos['price_open'])
                    
                    req = {}
                    if is_pending:
                        req = {
                            "action": mt5.TRADE_ACTION_MODIFY,
                            "order": ticket,
                            "price": price,
                            "sl": sl,
                            "tp": tp
                        }
                    else:
                        req = {
                            "action": mt5.TRADE_ACTION_SLTP,
                            "position": ticket,
                            "sl": sl,
                            "tp": tp
                        }
                    
                    res = mt5.order_send(req)
                    if res.retcode == mt5.TRADE_RETCODE_DONE:
                        ModernToast.show_message(self, f"Modified #{ticket}", style="success")
                    else:
                        ModernToast.show_message(self, f"Failed: {res.comment}", style="error")
                
                self.drag_trade = None
                self.setCursor(Qt.ArrowCursor)
                self.update()
                return

            if self.rr_drawing and self.rr_start_price is not None and self.rr_sl_price is not None:
                # Finalize new R:R
                entry = self.rr_start_price
                sl = self.rr_sl_price
                risk = abs(entry - sl)
                is_long = sl < entry
                self.rr_ratio = CONFIG.get("chart_rr_ratio", 1.5)
                reward = risk * self.rr_ratio
                tp = entry + reward if is_long else entry - reward
                
                # Default width: 10 candles
                start_ts = self.rr_start_time if self.rr_start_time else self.get_time_from_x(event.x())
                interval = self.get_timeframe_seconds()
                end_ts = start_ts + (10 * interval)
                
                self.rr_drawings.append({'entry': entry, 'sl': sl, 'tp': tp, 'start_ts': start_ts, 'end_ts': end_ts})
                self.save_rr_drawings()
                self.rr_start_price = None # Reset temp
                self.rr_start_time = None
                
                # Deselect RR tool
                self.rr_active = False
                self.tool_finished.emit()
            
            self.rr_drag_idx = -1
            self.rr_drag_handle = None
            self.rr_drawing = False
            self.draw_drag_idx = -1
            # Finalize generic drawing
            if self.drawing_temp is not None:
                # If end not set, use current mouse
                if self.drawing_temp['type'] == 'measure':
                    if self.drawing_temp.get('x2') is None:
                        self.drawing_temp['x2'] = self.get_time_from_x(event.x())
                        self.drawing_temp['y2'] = self.get_snapped_price(event.pos())
                    self.measure_data = self.drawing_temp
                    self.drawing_temp = None
                    
                    # Deselect tool
                    self.draw_tool = None
                    self.tool_finished.emit()
                    self.update()
                    return

                if self.drawing_temp.get('x2') is None:
                    self.drawing_temp['x2'] = self.get_time_from_x(event.x())
                    self.drawing_temp['y2'] = self.get_snapped_price(event.pos())
                # Normalize for H/V lines: set appropriate coords
                if self.drawing_temp['type'] == 'hline':
                    # For H line, store only y (price); x1/x2 use full range marker (None ok)
                    self.drawing_temp['x1'] = 0
                    self.drawing_temp['x2'] = int(time.time() + 86400)  # far future
                    self.drawing_temp['y1'] = self.drawing_temp['y2']
                elif self.drawing_temp['type'] == 'vline':
                    self.drawing_temp['y1'] = 0
                    self.drawing_temp['y2'] = 999999
                    self.drawing_temp['x1'] = self.drawing_temp['x1']
                    self.drawing_temp['x2'] = self.drawing_temp['x1']

                self.drawings.append(self.drawing_temp)
                self.drawing_temp = None
                self.save_drawings()
                
                # Deselect tool
                self.draw_tool = None
                self.tool_finished.emit()
            self.is_dragging = False
            self.is_scaling = False
            self.setCursor(Qt.CrossCursor if self.crosshair_enabled else Qt.ArrowCursor)

    def wheelEvent(self, event):
        # Zoom Logic
        delta = event.angleDelta().y()
        step = 5 # Number of candles to add/remove per scroll click
        
        if delta > 0:
            # Zoom In (Show fewer candles)
            self.visible_count = max(10, self.visible_count - step)
        else:
            # Zoom Out (Show more candles)
            # Limit to 500 as that is the max fetched by the worker currently
            self.visible_count = min(500, self.visible_count + step)
            
        self.update()

    def mouseMoveEvent(self, event):
        self.cursor_pos = event.pos()

        # Check for history tooltips
        if self.show_history:
            for rect, trade in self.history_hotspots:
                if rect.contains(event.pos()):
                    profit = trade.get('profit', 0.0)
                    entry_dt = datetime.fromtimestamp(trade.get('entry_time', 0)).strftime('%H:%M:%S')
                    exit_dt = datetime.fromtimestamp(trade.get('exit_time', 0)).strftime('%H:%M:%S')
                    
                    tooltip_text = (f"P/L: ${profit:.2f}\n"
                                    f"Type: {trade.get('type', '')}\n"
                                    f"Entry: {entry_dt}\n"
                                    f"Exit: {exit_dt}")
                    
                    QToolTip.showText(event.globalPos(), tooltip_text, self, rect.toRect())
                    # Prioritize history tooltip over other hover effects
                    return
        
        # Hide if no hotspot found or history is off
        QToolTip.hideText()
        
        # Handle Dragging Existing R:R
        if self.rr_drag_idx >= 0 and self.rr_drag_idx < len(self.rr_drawings):
            p = self.get_snapped_price(event.pos())
            rr = self.rr_drawings[self.rr_drag_idx]
            
            if self.rr_drag_handle == 'sl':
                rr['sl'] = p
            elif self.rr_drag_handle == 'tp':
                rr['tp'] = p
            elif self.rr_drag_handle == 'width':
                t = self.get_time_from_x(event.x())
                if t > rr['start_ts']:
                    rr['end_ts'] = t
            elif self.rr_drag_handle == 'entry':
                # Move entire structure
                if not self.magnet_mode:
                    p += self.rr_drag_offset
                
                diff = p - rr['entry']
                rr['entry'] = p
                rr['sl'] += diff
                rr['tp'] += diff
                
                # Move time horizontally too?
                # For now, let's keep time fixed when moving vertically/entry, 
                # unless we want full drag. 
                # Let's keep it simple: Entry handle moves PRICE only. 
                # To move TIME, we'd need a center drag handle.
            
            self.save_rr_drawings()
            self.update()
            return
        
        # Handle Dragging Generic Drawing
        if self.draw_drag_idx != -1 and self.draw_drag_idx < len(self.drawings):
            dx = event.x() - self.last_drag_x
            dy = event.y() - self.last_drag_y
            self.last_drag_x = event.x()
            self.last_drag_y = event.y()
            
            dr = self.drawings[self.draw_drag_idx]
            t = dr.get('type')
            
            # Helper to shift time/price
            def shift_t(ts, pix_dx):
                return self.get_time_from_x(self.get_x_from_time(ts) + pix_dx)
            def shift_p(p, pix_dy):
                return self.get_price_from_y(self.get_y_from_price(p) + pix_dy)
            
            if t in ['trend', 'fib', 'rect']:
                dr['x1'] = shift_t(dr['x1'], dx); dr['x2'] = shift_t(dr['x2'], dx)
                dr['y1'] = shift_p(dr['y1'], dy); dr['y2'] = shift_p(dr['y2'], dy)
            elif t == 'hline': dr['y1'] = shift_p(dr['y1'], dy)
            elif t == 'vline': dr['x1'] = shift_t(dr['x1'], dx)
            
            self.save_drawings()
            self.update()
            return
            
        if self.drag_trade:
            self.drag_trade['current_val'] = self.get_snapped_price(event.pos())
            self.update()
            return

        # Update temp drawing while drawing tool active
        if self.drawing_temp is not None:
            # set x2/y2 live
            self.drawing_temp['x2'] = self.get_time_from_x(event.x())
            self.drawing_temp['y2'] = self.get_snapped_price(event.pos())
            self.update()
            return

        if self.rr_drawing and self.rr_active:
            p = self.get_snapped_price(event.pos())
            self.rr_sl_price = p
            self.update()
            return
        
        if not self.is_dragging and not self.is_scaling:
            if event.x() > self.width() - 60:
                self.setCursor(Qt.SizeVerCursor)
            else:
                # Hover effect for Trade Lines
                y = event.y()
                hover_trade = False
                for pos in self.positions:
                    if (pos['sl'] > 0 and abs(y - self.get_y_from_price(pos['sl'])) < 10) or \
                       (pos['tp'] > 0 and abs(y - self.get_y_from_price(pos['tp'])) < 10) or \
                       (pos.get('is_pending') and abs(y - self.get_y_from_price(pos['price_open'])) < 10):
                        hover_trade = True
                        break
                if hover_trade:
                    self.setCursor(Qt.SizeVerCursor)
                    return

                # Hover effect for R:R lines
                y = event.y()
                x = event.x()
                hover = False
                resize_hover = False
                for rr in self.rr_drawings:
                    if rr.get('locked', False): continue
                    xs = self.get_x_from_time(rr.get('start_ts', 0))
                    xe = self.get_x_from_time(rr.get('end_ts', 0))
                    
                    # Check resize handle
                    y_min = min(self.get_y_from_price(rr['tp']), self.get_y_from_price(rr['sl']))
                    y_max = max(self.get_y_from_price(rr['tp']), self.get_y_from_price(rr['sl']))
                    if abs(x - xe) < 10 and y_min <= y <= y_max:
                        resize_hover = True
                    elif (self.is_near_line(y, rr['entry'], x, xs, xe) or 
                          self.is_near_line(y, rr['sl'], x, xs, xe) or 
                          self.is_near_line(y, rr['tp'], x, xs, xe)):
                        hover = True
                
                if resize_hover:
                    self.setCursor(Qt.SizeHorCursor)
                elif hover:
                    self.setCursor(Qt.SizeVerCursor)
                else:
                    # Check generic hover
                    gen_hover = False
                    tol = 8
                    for dr in self.drawings:
                        if dr.get('locked', False): continue
                        t = dr.get('type')
                        try:
                            if t in ['trend', 'fib']:
                                x1 = self.get_x_from_time(dr.get('x1', 0)); x2 = self.get_x_from_time(dr.get('x2', 0))
                                y1 = self.get_y_from_price(dr.get('y1', 0)); y2 = self.get_y_from_price(dr.get('y2', 0))
                                dx = x2 - x1; dy = y2 - y1
                                if dx == 0 and dy == 0: dist = ((x - x1)**2 + (y - y1)**2) ** 0.5
                                else:
                                    tproj = ((x - x1) * dx + (y - y1) * dy) / (dx*dx + dy*dy)
                                    tproj = max(0.0, min(1.0, tproj))
                                    px = x1 + tproj * dx; py = y1 + tproj * dy
                                    dist = ((x - px)**2 + (y - py)**2) ** 0.5
                                if dist <= tol: gen_hover = True
                            elif t == 'hline':
                                if abs(y - self.get_y_from_price(dr.get('y1', 0))) <= tol: gen_hover = True
                            elif t == 'vline':
                                if abs(x - self.get_x_from_time(dr.get('x1', 0))) <= tol: gen_hover = True
                            elif t == 'rect':
                                x1 = self.get_x_from_time(dr.get('x1', 0)); x2 = self.get_x_from_time(dr.get('x2', 0))
                                y1 = self.get_y_from_price(dr.get('y1', 0)); y2 = self.get_y_from_price(dr.get('y2', 0))
                                if QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized().contains(event.pos()): gen_hover = True
                        except: continue
                    
                    if gen_hover: self.setCursor(Qt.SizeAllCursor)
                    elif self.crosshair_enabled: self.setCursor(Qt.ArrowCursor)
                    else: self.setCursor(Qt.ArrowCursor)

        if self.is_scaling:
            dy = self.last_scale_y - event.y()
            if dy != 0:
                factor = 1.0 + (abs(dy) * 0.005)
                if dy > 0:
                    self.vertical_zoom *= factor
                else:
                    self.vertical_zoom /= factor
                self.vertical_zoom = max(0.1, min(self.vertical_zoom, 50.0))
                self.last_scale_y = event.y()
                self.update()
            return

        if self.is_dragging:
            dx = event.x() - self.last_drag_x
            w = self.width() - 60
            if self.visible_count > 0 and w > 0:
                candle_w = w / self.visible_count
                # Sensitivity: 1 candle per candle_width pixels
                shift = int(dx / (candle_w if candle_w > 1 else 1))
                if shift != 0:
                    self.scroll_offset += shift
                    # Allow negative scroll (Shift Right)
                    min_off = -int(self.visible_count * 0.5)
                    if self.scroll_offset < min_off: self.scroll_offset = min_off
                    # Limit max scroll
                    max_off = max(0, len(self.candles) - 10)
                    if self.scroll_offset > max_off: self.scroll_offset = max_off
                    self.last_drag_x = event.x()
                    self.last_drag_y = event.y()
        
        self.update()

    def leaveEvent(self, event):
        self.cursor_pos = None
        self.update()

    def save_snapshot(self):
        filename, _ = QFileDialog.getSaveFileName(self, "Save Chart", f"chart_{int(time.time())}.png", "Images (*.png)")
        if filename:
            pixmap = self.grab()
            pixmap.save(filename)
            ModernToast.show_message(self, "Snapshot Saved", style="success")

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing, False) # Crisp lines
        
        # Background
        painter.fillRect(self.rect(), QColor("#131314"))
        
        total = len(self.candles)
        if total == 0:
            painter.setPen(QColor("#E3E3E3"))
            painter.drawText(self.rect(), Qt.AlignCenter, "Waiting for Chart Data...")
            return

        # Layout
        w = self.width()
        h = self.height()
        right_margin = 60
        chart_w = w - right_margin

        # Slice Candles for View
        end_idx = total - self.scroll_offset
        start_idx = end_idx - self.visible_count
        
        # Data for scaling
        d_start = max(0, int(start_idx))
        d_end = min(total, int(end_idx))
        view_candles = self.candles[d_start:d_end]

        # Calculate Scale
        if view_candles:
            highs = [c['high'] for c in view_candles]
            lows = [c['low'] for c in view_candles]
            max_price = max(highs)
            min_price = min(lows)
        elif total > 0:
            max_price = self.candles[-1]['high'] + 1.0
            min_price = self.candles[-1]['low'] - 1.0
        else:
            max_price = 1.0
            min_price = 0.0
            
        price_range = max_price - min_price
        if price_range == 0: price_range = 1.0
        
        if self.vertical_zoom != 1.0:
            mid = min_price + (price_range / 2)
            new_range = price_range / self.vertical_zoom
            min_price = mid - (new_range / 2)
            max_price = mid + (new_range / 2)
            price_range = new_range
            
        # Store for mouse interaction
        self.last_min_price = min_price
        self.last_price_range = price_range
        self.last_h = h
        
        candle_w = chart_w / self.visible_count if self.visible_count > 0 else 0
        
        # Helper: Price to Y
        def to_y(price):
            pct = (price - min_price) / price_range
            return h - (pct * (h - 20)) - 10 # 10px padding top/bottom

        # Grid & Labels
        # MT5-style Grid: Sparse dots, wider spacing
        grid_pen = QPen(QColor("#444746"), 1, Qt.CustomDashLine)
        grid_pen.setDashPattern([1, 5])
        painter.setPen(grid_pen)
        
        # Axis Separator Lines (MT5 Style)
        axis_pen = QPen(QColor("#444746"), 1, Qt.SolidLine)
        painter.setPen(axis_pen)
        painter.drawLine(int(chart_w), 0, int(chart_w), int(h - 20)) # Vertical Axis
        painter.drawLine(0, int(h - 20), int(chart_w), int(h - 20)) # Horizontal Axis
        painter.setPen(grid_pen)
        
        steps = max(5, int(h / 45))
        for i in range(steps):
            p = min_price + (price_range * i / (steps - 1))
            y = to_y(p)
            painter.drawLine(0, int(y), int(chart_w), int(y))
            # Tick mark
            painter.setPen(axis_pen)
            painter.drawLine(int(chart_w), int(y), int(chart_w) + 3, int(y))
            painter.setPen(QColor("#C4C7C5"))
            painter.drawText(int(chart_w) + 5, int(y) + 5, f"{p:.2f}")
            painter.setPen(grid_pen)

        # X-Axis Grid (Time)
        if self.visible_count > 0:
            x_steps = max(5, int(chart_w / 90))
            x_step_idx = max(1, self.visible_count // x_steps)
            
            tf = CONFIG.get("chart_timeframe", "M1")
            if tf in ["D1"]:
                fmt = "%Y-%m-%d"
                w_lbl = 80
            elif tf in ["H4"]:
                fmt = "%m-%d %H:%M"
                w_lbl = 70
            else:
                fmt = "%H:%M"
                w_lbl = 50

            for i in range(0, self.visible_count, x_step_idx):
                cx = i * candle_w + candle_w / 2
                painter.setPen(grid_pen)
                painter.drawLine(int(cx), 0, int(cx), int(h - 20))
                
                # use pixel->time mapping so fractional shifts (eg. W1) update labels
                ts = self.get_time_from_x(cx)
                dt_str = datetime.fromtimestamp(int(ts)).strftime(fmt)
                painter.setPen(QColor("#C4C7C5"))
                painter.drawText(QRectF(cx - (w_lbl/2), h - 20, w_lbl, 20), Qt.AlignCenter, dt_str)

        # Draw Candles
        body_w = max(1, candle_w * 0.8)
        
        # Volume Scaling
        max_vol = 1
        if view_candles:
             vols = [c.get('tick_volume', 0) for c in view_candles]
             if vols: max_vol = max(vols)
        
        vol_max_h = h * 0.15

        # Prepare Line Path if needed
        line_path = QPainterPath()
        first_point = True
        
        for i in range(self.visible_count):
            idx = int(start_idx + i)
            if 0 <= idx < total:
                c = self.candles[idx]
                cx = i * candle_w + candle_w / 2
                
                # Draw Volume
                vol = c.get('tick_volume', 0)
                if max_vol > 0:
                    vh = (vol / max_vol) * vol_max_h
                    is_bull = c['close'] >= c['open']
                    color = QColor("#81C995") if is_bull else QColor("#F28B82")
                    color.setAlpha(50)
                    painter.setBrush(QBrush(color))
                    painter.setPen(Qt.NoPen)
                    painter.drawRect(QRectF(cx - body_w/2, h - 20 - vh, body_w, vh))
            
                if self.mode == "line":
                    y = to_y(c['close'])
                    if first_point:
                        line_path.moveTo(cx, y)
                        first_point = False
                    else:
                        line_path.lineTo(cx, y)
                else:
                    # Candle Mode
                    y_high = to_y(c['high'])
                    y_low = to_y(c['low'])
                    y_open = to_y(c['open'])
                    y_close = to_y(c['close'])
                    
                    is_bull = c['close'] >= c['open']
                    color = QColor("#81C995") if is_bull else QColor("#F28B82")
                    
                    painter.setPen(color)
                    painter.drawLine(int(cx), int(y_high), int(cx), int(y_low))
                    
                    # Body
                    top = min(y_open, y_close)
                    height = abs(y_open - y_close)
                    if height < 1: height = 1
                    
                    rect = QRectF(cx - body_w/2, top, body_w, height)
                    painter.setBrush(QBrush(color))
                    painter.drawRect(rect)
        
        if self.mode == "line" and not first_point:
            painter.setPen(QPen(QColor("#A8C7FA"), 2))
            painter.setBrush(Qt.NoBrush)
            painter.drawPath(line_path)
            
        # Draw Technical Analysis Overlays
        self.draw_ta_overlays(painter, chart_w, h, min_price, price_range, start_idx, end_idx)
        
        # Draw Historic Trades
        self.draw_history(painter, chart_w, h, min_price, price_range, start_idx, end_idx)

        # Draw Risk/Reward Tool
        # Combine temp drawing with saved drawings
        drawings_to_render = self.rr_drawings[:]
        if self.rr_start_price is not None and self.rr_sl_price is not None:
            # Calculate temp TP for rendering
            entry = self.rr_start_price
            sl = self.rr_sl_price
            risk = abs(entry - sl)
            tp = entry # Default placeholder
            if risk > 0:
                is_long = sl < entry
                reward = risk * self.rr_ratio
                tp = entry + reward if is_long else entry - reward
            
            # Temp drawing uses current mouse x for width preview? 
            # Or just default width. Let's use default width for temp.
            start_ts = self.rr_start_time if self.rr_start_time else self.candles[-1]['time']
            end_ts = start_ts + (10 * self.get_timeframe_seconds())
            drawings_to_render.append({'entry': entry, 'sl': sl, 'tp': tp, 'start_ts': start_ts, 'end_ts': end_ts})

        for rr in drawings_to_render:
            entry = rr['entry']
            sl = rr['sl']
            tp = rr['tp']
            
            # Calculate X bounds
            x_start = self.get_x_from_time(rr.get('start_ts', 0))
            x_end = self.get_x_from_time(rr.get('end_ts', 0))
            width = x_end - x_start
            if width < 5: width = 5 # Min width
            
            risk = abs(entry - sl)
            reward = abs(entry - tp)
            
            if risk > 0:
                is_long = sl < entry # Direction based on SL relative to Entry
                
                # Calculate actual ratio
                ratio = reward / risk if risk > 0 else 0.0

                y_entry = to_y(entry)
                y_sl = to_y(sl)
                y_tp = to_y(tp)
                
                # Draw Risk Box (Red)
                r_rect = QRectF(x_start, min(y_entry, y_sl), width, abs(y_entry - y_sl))
                painter.setBrush(QBrush(QColor(191, 97, 106, 80))) # Red alpha
                painter.setPen(Qt.NoPen)
                painter.drawRect(r_rect)
                
                # Draw Reward Box (Green)
                g_rect = QRectF(x_start, min(y_entry, y_tp), width, abs(y_entry - y_tp))
                painter.setBrush(QBrush(QColor(163, 190, 140, 80))) # Green alpha
                painter.drawRect(g_rect)
                
                # Lines
                line_color = QColor(rr.get('color', "#ECEFF4"))
                painter.setPen(QPen(line_color, 1, Qt.SolidLine))
                painter.drawLine(int(x_start), int(y_entry), int(x_end), int(y_entry))
                
                # Resize Handle (Right Edge of Entry Line)
                painter.setBrush(QBrush(QColor("#ECEFF4")))
                painter.drawEllipse(QPointF(x_end, y_entry), 3, 3)
                
                # Labels
                painter.setPen(line_color)
                risk_pips = risk * 10 if "XAU" in CONFIG["trade_symbol"] else risk * 10000 # Approx
                reward_pips = reward * 10 if "XAU" in CONFIG["trade_symbol"] else reward * 10000
                
                painter.drawText(int(x_start) + 5, int(y_sl) + (15 if is_long else -5), f"SL: {sl:.2f} (-{risk_pips:.1f} pips)")
                painter.drawText(int(x_start) + 5, int(y_tp) + (-5 if is_long else 15), f"TP: {tp:.2f} (+{reward_pips:.1f} pips)")
                
                mid_y = (y_entry + y_tp) / 2
                painter.drawText(int(x_start + width/2 - 20), int(mid_y), f"R:R 1:{ratio:.2f}")
                
                if rr.get('text'):
                    tc = rr.get('text_color', "#E3E3E3")
                    painter.setPen(QColor(tc))
                    painter.drawText(int(x_start), int(y_entry) - 5, rr.get('text'))

        # Draw generic drawings (trendlines, hlines, vlines)
        for dr in self.drawings:
            try:
                t = dr.get('type')
                x1 = self.get_x_from_time(dr.get('x1', 0))
                x2 = self.get_x_from_time(dr.get('x2', 0))
                y1 = to_y(dr.get('y1', 0))
                y2 = to_y(dr.get('y2', 0))
            except Exception:
                continue

            color = QColor(dr.get('color', "#FDD663"))
            pen = QPen(color, 2, Qt.SolidLine)
            painter.setPen(pen)
            if t == 'trend':
                painter.drawLine(int(x1), int(y1), int(x2), int(y2))
                painter.drawEllipse(QPointF(x1, y1), 3, 3)
                painter.drawEllipse(QPointF(x2, y2), 3, 3)
            elif t == 'hline':
                painter.drawLine(0, int(y1), int(w), int(y1))
                painter.drawText(5, int(y1) - 5, f"H: {dr.get('y1'):.2f}")
            elif t == 'vline':
                painter.drawLine(int(x1), 0, int(x1), int(h))
                painter.drawText(int(x1) + 4, 15, f"T: {datetime.fromtimestamp(dr.get('x1')).strftime('%H:%M')}")
            elif t == 'fib':
                # Diagonal
                painter.setPen(QPen(color, 1, Qt.DashLine))
                painter.drawLine(int(x1), int(y1), int(x2), int(y2))
                # Levels
                levels = [0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.0]
                p_start = dr.get('y1', 0)
                p_end = dr.get('y2', 0)
                diff = p_end - p_start
                lx, rx = min(x1, x2), max(x1, x2)
                for lvl in levels:
                    p_lvl = p_start + (diff * lvl)
                    y_lvl = to_y(p_lvl)
                    color = QColor("#C4C7C5")
                    if lvl == 0.5: color = QColor("#FDD663")
                    elif lvl in [0.382, 0.618]: color = QColor("#81C995")
                    painter.setPen(QPen(color, 1, Qt.SolidLine))
                    painter.drawLine(int(lx), int(y_lvl), int(rx), int(y_lvl))
                    painter.drawText(int(rx) + 5, int(y_lvl) + 4, f"{lvl:.3f}")
            elif t == 'rect':
                rect = QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized()
                painter.setPen(QPen(color, 1, Qt.SolidLine))
                fill_color = QColor(color)
                fill_color.setAlpha(50)
                painter.setBrush(QBrush(fill_color))
                painter.drawRect(rect)
                painter.setBrush(Qt.NoBrush)
            
            if dr.get('text'):
                tc = dr.get('text_color', "#E3E3E3")
                painter.setPen(QColor(tc))
                if t == 'trend' or t == 'fib': painter.drawText(int((x1+x2)/2), int((y1+y2)/2) - 5, dr.get('text'))
                elif t == 'hline': painter.drawText(int(w) - 150, int(y1) - 5, dr.get('text'))
                elif t == 'vline': painter.drawText(int(x1) + 5, 45, dr.get('text'))
                elif t == 'rect': 
                    rect = QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized()
                    painter.drawText(rect, Qt.AlignCenter | Qt.TextWordWrap, dr.get('text'))

        # Draw temp drawing if present
        if self.drawing_temp is not None:
            dt = self.drawing_temp
            try:
                tx1 = self.get_x_from_time(dt.get('x1', 0))
                raw_x2 = dt.get('x2')
                tx2 = self.get_x_from_time(raw_x2 if raw_x2 is not None else dt.get('x1', 0))
                ty1 = to_y(dt.get('y1', 0))
                raw_y2 = dt.get('y2')
                ty2 = to_y(raw_y2 if raw_y2 is not None else dt.get('y1', 0))
            except Exception:
                tx1 = tx2 = ty1 = ty2 = 0
            painter.setPen(QPen(QColor("#A8C7FA"), 1, Qt.DashLine))
            if dt.get('type') == 'trend':
                painter.drawLine(int(tx1), int(ty1), int(tx2), int(ty2))
            elif dt.get('type') == 'hline':
                painter.drawLine(0, int(ty1), int(w), int(ty1))
            elif dt.get('type') == 'vline':
                painter.drawLine(int(tx1), 0, int(tx1), int(h))
            elif dt.get('type') == 'fib':
                painter.drawLine(int(tx1), int(ty1), int(tx2), int(ty2))
                # Draw simple preview of 0, 0.5, 1 levels
                p_start, p_end = dt.get('y1', 0), dt.get('y2', 0)
                diff = p_end - p_start
                lx, rx = min(tx1, tx2), max(tx1, tx2)
                for lvl in [0.0, 0.5, 1.0]:
                    y_lvl = to_y(p_start + (diff * lvl))
                    painter.drawLine(int(lx), int(y_lvl), int(rx), int(y_lvl))
            elif dt.get('type') == 'rect':
                rect = QRectF(QPointF(tx1, ty1), QPointF(tx2, ty2)).normalized()
                painter.setPen(QPen(QColor("#669DF6"), 1, Qt.SolidLine))
                painter.setBrush(QBrush(QColor(102, 157, 246, 50)))
                painter.drawRect(rect)
                painter.setBrush(Qt.NoBrush)
            elif dt.get('type') == 'measure':
                self.draw_measure_tool(painter, tx1, ty1, tx2, ty2, dt)


        # Draw Positions
        for pos in self.positions:
            # Determine values (override if dragging)
            draw_price = pos['price_open']
            draw_sl = pos['sl']
            draw_tp = pos['tp']
            
            if self.drag_trade and self.drag_trade['ticket'] == pos['ticket']:
                if self.drag_trade['type'] == 'PRICE': draw_price = self.drag_trade['current_val']
                elif self.drag_trade['type'] == 'SL': draw_sl = self.drag_trade['current_val']
                elif self.drag_trade['type'] == 'TP': draw_tp = self.drag_trade['current_val']

            py = to_y(draw_price)
            is_buy = ("BUY" in pos['type'])
            is_pending = pos.get('is_pending', False)
            
            if is_pending:
                color = QColor("#FDD663") # Yellow for Pending
                pen_style = Qt.DashDotLine
            else:
                color = QColor("#81C995") if is_buy else QColor("#F28B82")
                pen_style = Qt.DashLine
            
            painter.setPen(QPen(color, 1, pen_style))
            painter.drawLine(0, int(py), int(chart_w), int(py))
            painter.drawText(5, int(py) - 5, f"{pos['type']} {pos['volume']} @ {draw_price:.2f}")
            
            # Draw SL
            if draw_sl > 0:
                y_sl = to_y(draw_sl)
                painter.setPen(QPen(QColor("#F28B82"), 1, Qt.DashDotLine))
                painter.drawLine(0, int(y_sl), int(chart_w), int(y_sl))

                sl_diff = draw_sl - draw_price if is_buy else draw_price - draw_sl
                sl_loss = sl_diff * pos['volume'] * self.contract_size
                painter.drawText(int(chart_w) - 90, int(y_sl) - 5, f"SL (${sl_loss:.2f})")

            # Draw TP
            if draw_tp > 0:
                y_tp = to_y(draw_tp)
                painter.setPen(QPen(QColor("#81C995"), 1, Qt.DashDotLine))
                painter.drawLine(0, int(y_tp), int(chart_w), int(y_tp))

                tp_diff = draw_tp - draw_price if is_buy else draw_price - draw_tp
                tp_profit = tp_diff * pos['volume'] * self.contract_size
                painter.drawText(int(chart_w) - 90, int(y_tp) - 5, f"TP (${tp_profit:.2f})")

        # Draw Bid/Ask
        if self.bid > 0:
            y_bid = to_y(self.bid)
            color_bid = QColor("#F28B82")
            painter.setPen(QPen(color_bid, 1, Qt.DotLine))
            painter.drawLine(0, int(y_bid), int(w), int(y_bid))
            
            lbl_rect = QRectF(chart_w, y_bid - 10, right_margin, 20)
            painter.fillRect(lbl_rect, color_bid)
            painter.setPen(QColor("#000000"))
            painter.drawText(lbl_rect, Qt.AlignCenter, f"{self.bid:.2f}")

        if self.ask > 0:
            y_ask = to_y(self.ask)
            color_ask = QColor("#669DF6")
            painter.setPen(QPen(color_ask, 1, Qt.DotLine))
            painter.drawLine(0, int(y_ask), int(w), int(y_ask))
            
            lbl_rect = QRectF(chart_w, y_ask - 10, right_margin, 20)
            painter.fillRect(lbl_rect, color_ask)
            painter.setPen(QColor("#000000"))
            painter.drawText(lbl_rect, Qt.AlignCenter, f"{self.ask:.2f}")
            
        # Draw active measurement
        if self.measure_data is not None:
            dt = self.measure_data
            tx1 = self.get_x_from_time(dt.get('x1', 0))
            raw_x2 = dt.get('x2')
            tx2 = self.get_x_from_time(raw_x2 if raw_x2 is not None else dt.get('x1', 0))
            ty1 = self.get_y_from_price(dt.get('y1', 0))
            raw_y2 = dt.get('y2')
            ty2 = self.get_y_from_price(raw_y2 if raw_y2 is not None else dt.get('y1', 0))
            self.draw_measure_tool(painter, tx1, ty1, tx2, ty2, dt)

        # Current Price Line
        last = self.candles[-1] # Always show current price of latest candle
        curr_y = to_y(last['close'])
        painter.setPen(QPen(QColor("#A8C7FA"), 1, Qt.DashLine))
        painter.drawLine(0, int(curr_y), int(chart_w), int(curr_y))
        
        # Current Price Label
        lbl_rect = QRectF(chart_w, curr_y - 10, right_margin, 20)
        painter.fillRect(lbl_rect, QColor("#A8C7FA"))
        painter.setPen(QColor("#000000"))
        painter.drawText(lbl_rect, Qt.AlignCenter, f"{last['close']:.2f}")
        
        # Draw Header Info
        self.draw_header(painter, chart_w)

        # OHLC Display
        target_candle = None
        if self.cursor_pos and self.cursor_pos.x() < chart_w:
            mx = self.cursor_pos.x()
            slot_idx = int(mx / candle_w) if candle_w > 0 else 0
            idx = int(start_idx + slot_idx)
            if 0 <= idx < total:
                target_candle = self.candles[idx]
        
        if target_candle is None and total > 0:
            target_candle = self.candles[-1]

        if target_candle:
            is_bull = target_candle['close'] >= target_candle['open']
            text_color = QColor("#81C995") if is_bull else QColor("#F28B82")

            ohlc_text = (f"O:{target_candle['open']:.2f} "
                         f"H:{target_candle['high']:.2f} "
                         f"L:{target_candle['low']:.2f} "
                         f"C:{target_candle['close']:.2f} "
                         f"V:{target_candle.get('tick_volume', 0)}")
            
            painter.save()
            painter.setPen(text_color)
            f = painter.font()
            f.setFamily("Consolas")
            f.setBold(True)
            painter.setFont(f)
            painter.drawText(10, 20, ohlc_text)
            painter.restore()

        # Crosshair
        if self.crosshair_enabled and self.cursor_pos:
            mx = self.cursor_pos.x()
            my = self.cursor_pos.y()
            
            if mx < chart_w:
                painter.setPen(QPen(QColor("#E3E3E3"), 1, Qt.DotLine))
                
                # Crosshair Lines
                painter.drawLine(mx, 0, mx, h)
                painter.drawLine(0, my, int(chart_w), my)
                
                # Price Label (Right Axis)
                draw_h = h - 20
                if draw_h > 0:
                    pct = (h - 10 - my) / draw_h
                    price = min_price + (pct * price_range)
                    lbl_rect = QRectF(chart_w, my - 10, right_margin, 20)
                    painter.fillRect(lbl_rect, QColor("#444746"))
                    painter.drawText(lbl_rect, Qt.AlignCenter, f"{price:.2f}")
                
                # Time Label (Bottom)
                slot_idx = int(mx / candle_w) if candle_w > 0 else 0
                data_idx = int(start_idx + slot_idx)
                if 0 <= data_idx < total:
                    # show precise time based on pixel position (handles W1 scrolling)
                    dt_str = datetime.fromtimestamp(int(self.get_time_from_x(mx))).strftime("%Y-%m-%d %H:%M")
                    
                    lbl_w = 110
                    lbl_x = max(0, min(mx - lbl_w / 2, chart_w - lbl_w))
                    
                    t_rect = QRectF(lbl_x, h - 20, lbl_w, 20)
                    painter.fillRect(t_rect, QColor("#444746"))
                    painter.drawText(t_rect, Qt.AlignCenter, dt_str)

    def draw_measure_tool(self, painter, x1, y1, x2, y2, dt):
        t1 = dt.get('x1')
        p1 = dt.get('y1')
        t2 = dt.get('x2')
        p2 = dt.get('y2')
        
        if t2 is None or p2 is None: return

        # Draw Box
        rect = QRectF(QPointF(x1, y1), QPointF(x2, y2)).normalized()
        painter.setBrush(QBrush(QColor(168, 199, 250, 40)))
        painter.setPen(QPen(QColor("#A8C7FA"), 1, Qt.DashLine))
        painter.drawRect(rect)
        
        # Draw Line
        painter.drawLine(int(x1), int(y1), int(x2), int(y2))
        
        # Stats
        dy = p2 - p1
        dx_sec = t2 - t1
        
        symbol = CONFIG.get("trade_symbol", "XAUUSD")
        is_xau = "XAU" in symbol or "GOLD" in symbol
        is_jpy = "JPY" in symbol
        
        if is_xau: pip_val = dy * 10
        elif is_jpy: pip_val = dy * 100
        else: pip_val = dy * 10000
        
        pct = (dy / p1) * 100 if p1 != 0 else 0
        interval = self.get_timeframe_seconds()
        bars = int(abs(dx_sec) / interval)
        
        abs_sec = abs(dx_sec)
        if abs_sec < 60: dur_str = f"{int(abs_sec)}s"
        elif abs_sec < 3600: dur_str = f"{int(abs_sec//60)}m"
        elif abs_sec < 86400: dur_str = f"{int(abs_sec//3600)}h {int((abs_sec%3600)//60)}m"
        else: dur_str = f"{int(abs_sec//86400)}d"
        
        text = f"{dy:+.2f} ({pip_val:+.1f} pips)\n{pct:+.2f}%\n{bars} bars, {dur_str}"
        
        fm = painter.fontMetrics()
        lines = text.split('\n')
        w_txt = max([fm.horizontalAdvance(l) for l in lines]) + 20
        h_txt = (fm.height() + 2) * len(lines) + 10
        
        cx, cy = (x1 + x2) / 2, (y1 + y2) / 2
        box_rect = QRectF(cx - w_txt/2, cy - h_txt/2, w_txt, h_txt)
        
        painter.setBrush(QBrush(QColor(30, 31, 32, 230)))
        painter.setPen(QPen(QColor("#C4C7C5"), 1))
        painter.drawRoundedRect(box_rect, 4, 4)
        painter.setPen(QColor("#E3E3E3"))
        painter.drawText(box_rect, Qt.AlignCenter, text)

    def draw_header(self, painter, w):
        # High-precision Price Display
        if self.bid > 0 and self.ask > 0:
            spread = self.ask - self.bid
            
            font = QFont("Consolas", 10)
            font.setBold(True)
            painter.setFont(font)
            
            # Background for header
            bg_rect = QRectF(10, 10, 300, 80)
            painter.setBrush(QBrush(QColor(30, 31, 32, 200)))
            painter.setPen(Qt.NoPen)
            painter.drawRoundedRect(bg_rect, 4, 4)
            
            painter.setPen(QColor("#E3E3E3"))
            painter.drawText(20, 30, f"{CONFIG['trade_symbol']} ({CONFIG.get('chart_timeframe', 'M1')})")
            
            painter.setPen(QColor("#81C995"))
            painter.drawText(20, 50, f"Bid: {self.bid:.3f}")
            
            painter.setPen(QColor("#F28B82"))
            painter.drawText(140, 50, f"Ask: {self.ask:.3f}")
            
            painter.setPen(QColor("#FDD663"))
            painter.drawText(20, 70, f"Spread: {spread:.3f}")

            # Candle Timer
            interval = self.get_timeframe_seconds()
            now = int(time.time())
            time_left = interval - (now % interval)
            mins = time_left // 60
            secs = time_left % 60
            
            painter.setPen(QColor("#A8C7FA"))
            painter.drawText(140, 70, f"Time: {mins:02d}:{secs:02d}")

    def draw_history(self, painter, w, h, min_p, p_range, start_idx, end_idx):
        if not self.show_history:
            self.history_hotspots.clear()
            return

        self.history_hotspots = [] # Reset on each paint

        def to_y(price):
            pct = (price - min_p) / p_range
            return h - (pct * (h - 20)) - 10
            
        for trade in self.history:
            entry_time = trade.get('entry_time', 0)
            exit_time = trade.get('exit_time', 0)
            
            # Optimization: Skip trades completely out of view
            start_ts = self.candles[int(start_idx)]['time'] if 0 <= int(start_idx) < len(self.candles) else 0
            end_ts = self.candles[min(len(self.candles)-1, int(end_idx))]['time'] if 0 <= int(end_idx) < len(self.candles) else float('inf')
            if not (start_ts <= entry_time <= end_ts or start_ts <= exit_time <= end_ts or (entry_time < start_ts and exit_time > end_ts)):
                continue

            # Check if time is within view
            if not self.candles: continue
            start_ts = self.candles[int(start_idx)]['time'] if 0 <= int(start_idx) < len(self.candles) else 0
            end_ts = self.candles[min(len(self.candles)-1, int(end_idx))]['time'] if 0 <= int(end_idx) < len(self.candles) else float('inf')
            if not (start_ts <= entry_time <= end_ts or start_ts <= exit_time <= end_ts or (entry_time < start_ts and exit_time > end_ts)):
                continue
                
            x_entry = self.get_x_from_time(entry_time)
            y_entry = to_y(trade.get('entry_price', 0))
            x_exit = self.get_x_from_time(exit_time)
            y_exit = to_y(trade.get('exit_price', 0))
            
            # Draw connecting line
            profit = trade.get('profit', 0.0)
            line_color = QColor("#81C995") if profit >= 0 else QColor("#F28B82")
            line_color.setAlpha(150)
            painter.setPen(QPen(line_color, 1, Qt.DashLine))
            painter.drawLine(int(x_entry), int(y_entry), int(x_exit), int(y_exit))

            arrow_size = 8
            is_buy = trade.get('type') == 'BUY'

            # Draw entry arrow
            color = QColor("#81C995") if is_buy else QColor("#F28B82")
            painter.setBrush(QBrush(color))
            painter.setPen(Qt.NoPen)
            
            p1 = QPointF(x_entry, y_entry - arrow_size/2) if is_buy else QPointF(x_entry, y_entry + arrow_size/2)
            p2 = QPointF(x_entry - arrow_size/2, y_entry + arrow_size/2) if is_buy else QPointF(x_entry - arrow_size/2, y_entry - arrow_size/2)
            p3 = QPointF(x_entry + arrow_size/2, y_entry + arrow_size/2) if is_buy else QPointF(x_entry + arrow_size/2, y_entry - arrow_size/2)
            painter.drawPolygon(QPolygonF([p1, p2, p3]))
            self.history_hotspots.append((QRectF(p2, p3).normalized(), trade))

            # Draw exit arrow (opposite direction)
            painter.setBrush(QBrush(line_color))
            p1 = QPointF(x_exit, y_exit + arrow_size/2) if is_buy else QPointF(x_exit, y_exit - arrow_size/2)
            p2 = QPointF(x_exit - arrow_size/2, y_exit - arrow_size/2) if is_buy else QPointF(x_exit - arrow_size/2, y_exit + arrow_size/2)
            p3 = QPointF(x_exit + arrow_size/2, y_exit - arrow_size/2) if is_buy else QPointF(x_exit + arrow_size/2, y_exit + arrow_size/2)
            painter.drawPolygon(QPolygonF([p1, p2, p3]))
            self.history_hotspots.append((QRectF(p2, p3).normalized(), trade))

    def draw_ta_overlays(self, painter, w, h, min_p, p_range, start_idx, end_idx):
        def to_y(price):
            pct = (price - min_p) / p_range
            return h - (pct * (h - 20)) - 10
        candle_w = w / self.visible_count if self.visible_count > 0 else 0

        # 2. Market Structure (ZigZag / CHoCH)
        if self.show_structure and len(self.candles) > 10:
            # Simplified ZigZag for visualization
            path = QPainterPath()
            started = False
            
            # Identify pivots (simple local min/max over 5 bars)
            scan_start = max(0, int(start_idx))
            scan_end = min(len(self.candles), int(end_idx))
            
            last_x, last_y = 0, 0
            
            painter.setPen(QPen(QColor("#C58AF9"), 2, Qt.SolidLine)) # Purple
            
            for i in range(scan_start, scan_end):
                # Check if this candle is a local extreme in a small window
                window = self.candles[max(0, i-3):min(len(self.candles), i+4)]
                if not window: continue
                
                c = self.candles[i]
                is_high = c['high'] == max(x['high'] for x in window)
                is_low = c['low'] == min(x['low'] for x in window)
                
                cx = (i - start_idx) * candle_w + candle_w/2
                
                if is_high:
                    y = to_y(c['high'])
                    if not started:
                        path.moveTo(cx, y)
                        started = True
                    else:
                        path.lineTo(cx, y)
                    # Mark High (optional small dot)
                    if self.show_pivot_dots:
                        painter.drawEllipse(QPointF(cx, y), 2, 2)
                    
                elif is_low:
                    y = to_y(c['low'])
                    if not started:
                        path.moveTo(cx, y)
                        started = True
                    else:
                        path.lineTo(cx, y)
                    # Mark Low (optional small dot)
                    if self.show_pivot_dots:
                        painter.drawEllipse(QPointF(cx, y), 2, 2)
            
            painter.setBrush(Qt.NoBrush)
            color = QColor("#C58AF9")
            color.setAlpha(150)
            painter.setPen(QPen(color, 1.5))
            painter.drawPath(path)

        # 3. Break Even Lines (Avg Entry)
        if self.show_be and self.positions:
            buys = [p for p in self.positions if p['type'] == "BUY"]
            sells = [p for p in self.positions if p['type'] == "SELL"]
            
            if buys:
                avg_buy = sum(p['price_open'] * p['volume'] for p in buys) / sum(p['volume'] for p in buys)
                y = to_y(avg_buy)
                painter.setPen(QPen(QColor("#A8C7FA"), 1, Qt.DashDotLine)) # Blue
                painter.drawLine(0, int(y), int(w), int(y))
                painter.drawText(10, int(y)-5, "Avg BUY")
            
            if sells:
                avg_sell = sum(p['price_open'] * p['volume'] for p in sells) / sum(p['volume'] for p in sells)
                y = to_y(avg_sell)
                painter.setPen(QPen(QColor("#F28B82"), 1, Qt.DashDotLine)) # Red
                painter.drawLine(0, int(y), int(w), int(y))
                painter.drawText(10, int(y)-5, "Avg SELL")

class DrawingManagerDialog(QDialog):
    def __init__(self, chart_widget, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Manage Drawings")
        self.resize(350, 400)
        self.chart = chart_widget
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(10, 10, 10, 10)
        layout.setSpacing(10)
        
        self.scroll = QScrollArea()
        self.scroll.setWidgetResizable(True)
        self.scroll.setStyleSheet("background-color: #1E1F20; border: 1px solid #444746; border-radius: 4px;")
        
        self.container = QWidget()
        self.container.setStyleSheet("background-color: #1E1F20;")
        self.list_layout = QVBoxLayout(self.container)
        self.list_layout.setAlignment(Qt.AlignTop)
        self.list_layout.setSpacing(5)
        self.list_layout.setContentsMargins(5, 5, 5, 5)
        
        self.scroll.setWidget(self.container)
        layout.addWidget(self.scroll)
        
        self.refresh_list()
        
        btn_close = QPushButton("Close")
        btn_close.clicked.connect(self.accept)
        layout.addWidget(btn_close)

    def refresh_list(self):
        while self.list_layout.count():
            child = self.list_layout.takeAt(0)
            if child.widget():
                child.widget().deleteLater()
        
        has_items = False
        
        # Measurement Tool
        if self.chart.measure_data:
            has_items = True
            self.add_row("Measurement Tool", False, lambda: None, self.delete_measure, lockable=False)

        # R:R Drawings
        for i, rr in enumerate(self.chart.rr_drawings):
            has_items = True
            start_ts = rr.get('start_ts', 0)
            dt_str = datetime.fromtimestamp(start_ts).strftime("%H:%M")
            is_locked = rr.get('locked', False)
            self.add_row(f"R:R Tool ({dt_str})", is_locked, 
                         lambda _, idx=i: self.toggle_lock_rr(idx),
                         lambda _, idx=i: self.delete_rr(idx),
                         obj=rr, draw_type='rr', index=i)
            
        # Generic Drawings
        for i, dr in enumerate(self.chart.drawings):
            has_items = True
            t = dr.get('type', 'Unknown').title()
            ts = dr.get('x1', 0)
            dt_str = datetime.fromtimestamp(ts).strftime("%H:%M") if ts > 0 else ""
            is_locked = dr.get('locked', False)
            self.add_row(f"{t} {dt_str}", is_locked,
                         lambda _, idx=i: self.toggle_lock_drawing(idx),
                         lambda _, idx=i: self.delete_drawing(idx),
                         obj=dr, draw_type='drawing', index=i)
            
        if not has_items:
            lbl = QLabel("No drawings found.")
            lbl.setAlignment(Qt.AlignCenter)
            lbl.setStyleSheet("color: #444746; font-style: italic; margin-top: 20px;")
            self.list_layout.addWidget(lbl)

    def add_row(self, text, is_locked, lock_callback, delete_callback, lockable=True, obj=None, draw_type=None, index=None):
        row = QFrame()
        row.setStyleSheet("background-color: #2D2E31; border-radius: 4px;")
        l = QHBoxLayout(row)
        l.setContentsMargins(10, 5, 10, 5)
        
        lbl = QLabel(text)
        lbl.setStyleSheet("color: #E3E3E3; border: none; font-weight: bold;")
        
        if obj is not None:
            # Object Color Button
            btn_color = QPushButton()
            btn_color.setFixedSize(24, 24)
            btn_color.setCursor(Qt.PointingHandCursor)
            c = obj.get('color', "#FDD663")
            btn_color.setStyleSheet(f"background-color: {c}; border: 1px solid #444746; border-radius: 4px;")
            btn_color.setToolTip("Change Object Color")
            btn_color.clicked.connect(lambda: self.change_color(draw_type, index))
            
            # Text Button
            btn_text = QPushButton("T")
            btn_text.setFixedSize(24, 24)
            btn_text.setCursor(Qt.PointingHandCursor)
            btn_text.setStyleSheet("background-color: #444746; color: #E3E3E3; border: none; border-radius: 4px; font-weight: bold;")
            btn_text.setToolTip("Set Text")
            btn_text.clicked.connect(lambda: self.change_text(draw_type, index))

            # Text Color Button
            btn_tc = QPushButton("A")
            btn_tc.setFixedSize(24, 24)
            btn_tc.setCursor(Qt.PointingHandCursor)
            tc = obj.get('text_color', "#E3E3E3")
            btn_tc.setStyleSheet(f"background-color: #444746; color: {tc}; border: none; border-radius: 4px; font-weight: bold;")
            btn_tc.setToolTip("Change Text Color")
            btn_tc.clicked.connect(lambda: self.change_text_color(draw_type, index))

        if lockable:
            btn_lock = QPushButton("🔒" if is_locked else "🔓")
            btn_lock.setFixedSize(32, 32)
            btn_lock.setCursor(Qt.PointingHandCursor)
            btn_lock.setToolTip("Unlock" if is_locked else "Lock")
            btn_lock.setStyleSheet("background-color: transparent; border: none; font-size: 18px;")
            btn_lock.clicked.connect(lock_callback)

        btn = QPushButton("✕")
        btn.setFixedSize(24, 24)
        btn.setCursor(Qt.PointingHandCursor)
        btn.setStyleSheet("background-color: #F28B82; color: #000000; border-radius: 12px; font-weight: bold; border: none;")
        btn.clicked.connect(delete_callback)
        
        l.addWidget(lbl)
        l.addStretch()
        if obj is not None:
            l.addWidget(btn_color)
            l.addWidget(btn_text)
            l.addWidget(btn_tc)
        if lockable:
            l.addWidget(btn_lock)
        l.addWidget(btn)
        self.list_layout.addWidget(row)

    def change_color(self, draw_type, index):
        target = self.chart.rr_drawings if draw_type == 'rr' else self.chart.drawings
        if 0 <= index < len(target):
            curr = target[index].get('color', "#FDD663")
            c = QColorDialog.getColor(QColor(curr), self, "Select Object Color")
            if c.isValid():
                target[index]['color'] = c.name()
                if draw_type == 'rr': self.chart.save_rr_drawings()
                else: self.chart.save_drawings()
                self.chart.update()
                self.refresh_list()

    def change_text(self, draw_type, index):
        target = self.chart.rr_drawings if draw_type == 'rr' else self.chart.drawings
        if 0 <= index < len(target):
            curr = target[index].get('text', "")
            text, ok = QInputDialog.getText(self, "Set Text", "Enter text:", text=curr)
            if ok:
                target[index]['text'] = text
                if draw_type == 'rr': self.chart.save_rr_drawings()
                else: self.chart.save_drawings()
                self.chart.update()
                self.refresh_list()

    def change_text_color(self, draw_type, index):
        target = self.chart.rr_drawings if draw_type == 'rr' else self.chart.drawings
        if 0 <= index < len(target):
            curr = target[index].get('text_color', "#E3E3E3")
            c = QColorDialog.getColor(QColor(curr), self, "Select Text Color")
            if c.isValid():
                target[index]['text_color'] = c.name()
                if draw_type == 'rr': self.chart.save_rr_drawings()
                else: self.chart.save_drawings()
                self.chart.update()
                self.refresh_list()

    def toggle_lock_rr(self, index):
        if 0 <= index < len(self.chart.rr_drawings):
            self.chart.rr_drawings[index]['locked'] = not self.chart.rr_drawings[index].get('locked', False)
            self.chart.save_rr_drawings()
            self.chart.update()
            self.refresh_list()

    def toggle_lock_drawing(self, index):
        if 0 <= index < len(self.chart.drawings):
            self.chart.drawings[index]['locked'] = not self.chart.drawings[index].get('locked', False)
            self.chart.save_drawings()
            self.chart.update()
            self.refresh_list()

    def delete_measure(self):
        self.chart.measure_data = None
        self.chart.update()
        self.refresh_list()

    def delete_rr(self, index):
        if 0 <= index < len(self.chart.rr_drawings):
            self.chart.rr_drawings.pop(index)
            self.chart.save_rr_drawings()
            self.chart.update()
            self.refresh_list()

    def delete_drawing(self, index):
        if 0 <= index < len(self.chart.drawings):
            self.chart.drawings.pop(index)
            self.chart.save_drawings()
            self.chart.update()
            self.refresh_list()

class ChartWindow(QMainWindow):
    closed = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Chart")
        self.resize(800, 600)
        self.setStyleSheet(GLOBAL_STYLESHEET)
        
        self.central_widget = QWidget()
        self.setCentralWidget(self.central_widget)
        layout = QVBoxLayout(self.central_widget)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        
        self.chart_widget = CandleChartWidget()
        self.chart_widget.tool_finished.connect(self.on_tool_finished)
        
        # Chart Toolbar
        toolbar_frame = QFrame()
        toolbar_frame.setFixedHeight(48)
        toolbar_frame.setStyleSheet("""
            QFrame { background-color: #131314; border-bottom: 1px solid #444746; }
            QLabel { font-weight: bold; color: #A8C7FA; font-size: 9pt; }
            QPushButton {
                background-color: transparent;
                border: 1px solid transparent;
                border-radius: 4px;
                color: #E3E3E3;
                font-size: 10pt;
                padding: 4px;
                font-weight: bold;
            }
            QPushButton:hover { background-color: #1E1F20; border: 1px solid #444746; }
            QPushButton:checked { background-color: #A8C7FA; color: #000000; border: 1px solid #A8C7FA; }
            QComboBox {
                background-color: #1E1F20;
                border: 1px solid #444746;
                border-radius: 4px;
                padding: 4px 10px;
                color: #F1F1F1;
                min-width: 60px;
            }
        """)
        
        toolbar = QHBoxLayout(toolbar_frame)
        toolbar.setContentsMargins(10, 4, 10, 4)
        toolbar.setSpacing(6)
        
        toolbar.addWidget(QLabel("TF:"))
        self.combo_tf = QComboBox()
        self.combo_tf.addItems(["M1", "M5", "M15", "M30", "H1", "H4", "D1"])
        self.combo_tf.setCurrentText(CONFIG.get("chart_timeframe", "M1"))
        self.combo_tf.currentTextChanged.connect(self.on_timeframe_changed)
        toolbar.addWidget(self.combo_tf)
        
        # Separator
        line1 = QFrame()
        line1.setFrameShape(QFrame.VLine)
        line1.setFrameShadow(QFrame.Sunken)
        line1.setStyleSheet("background-color: #444746; margin: 4px;")
        toolbar.addWidget(line1)
        
        self.btn_chart_mode = QPushButton("📈")
        self.btn_chart_mode.setToolTip("Switch to Line Chart")
        self.btn_chart_mode.setFixedSize(32, 32)
        self.btn_chart_mode.setCheckable(True)
        self.btn_chart_mode.clicked.connect(self.on_chart_mode_toggled)
        toolbar.addWidget(self.btn_chart_mode)
        
        self.btn_reset_view = QPushButton("↺")
        self.btn_reset_view.setToolTip("Reset Chart View")
        self.btn_reset_view.setFixedSize(32, 32)
        self.btn_reset_view.clicked.connect(self.chart_widget.reset_view)
        toolbar.addWidget(self.btn_reset_view)
        
        self.btn_shift = QPushButton("⇦")
        self.btn_shift.setToolTip("Shift End of Chart")
        self.btn_shift.setFixedSize(32, 32)
        self.btn_shift.setCheckable(True)
        self.btn_shift.setChecked(True)
        self.btn_shift.clicked.connect(self.on_shift_toggled)
        toolbar.addWidget(self.btn_shift)
        
        self.btn_zoom_in = QPushButton("➕")
        self.btn_zoom_in.setToolTip("Zoom In")
        self.btn_zoom_in.setFixedSize(32, 32)
        self.btn_zoom_in.clicked.connect(self.chart_widget.zoom_in)
        toolbar.addWidget(self.btn_zoom_in)
        
        self.btn_zoom_out = QPushButton("➖")
        self.btn_zoom_out.setToolTip("Zoom Out")
        self.btn_zoom_out.setFixedSize(32, 32)
        self.btn_zoom_out.clicked.connect(self.chart_widget.zoom_out)
        toolbar.addWidget(self.btn_zoom_out)
        
        self.btn_crosshair = QPushButton("✛")
        self.btn_crosshair.setToolTip("Toggle Crosshair")
        self.btn_crosshair.setFixedSize(32, 32)
        self.btn_crosshair.setCheckable(True)
        # Default: crosshair off (pointer); user can enable
        self.btn_crosshair.setChecked(False)
        self.btn_crosshair.setText("↗")
        self.btn_crosshair.setToolTip("Enable Crosshair")
        self.btn_crosshair.clicked.connect(self.on_crosshair_toggled)
        toolbar.addWidget(self.btn_crosshair)
        
        # Separator
        line2 = QFrame()
        line2.setFrameShape(QFrame.VLine)
        line2.setFrameShadow(QFrame.Sunken)
        line2.setStyleSheet("background-color: #444746; margin: 4px;")
        toolbar.addWidget(line2)
        
        self.btn_struct = QPushButton("Struct")
        self.btn_struct.setCheckable(True)
        self.btn_struct.setToolTip("Show Market Structure (ZigZag/CHoCH)")
        self.btn_struct.clicked.connect(self.chart_widget.toggle_structure)
        toolbar.addWidget(self.btn_struct)
        
        self.btn_be = QPushButton("BE")
        self.btn_be.setCheckable(True)
        self.btn_be.setToolTip("Show Break-Even / Average Entry Lines")
        self.btn_be.clicked.connect(self.chart_widget.toggle_be)
        toolbar.addWidget(self.btn_be)
        
        self.btn_hist = QPushButton("Hist")
        self.btn_hist.setCheckable(True)
        self.btn_hist.setToolTip("Show Trade History Arrows")
        self.btn_hist.clicked.connect(self.chart_widget.toggle_history)
        toolbar.addWidget(self.btn_hist)
        
        self.btn_rr = QPushButton("R/R")
        self.btn_rr.setCheckable(True)
        self.btn_rr.setToolTip(f"Risk/Reward Tool: Click & Drag to measure (1:{CONFIG.get('chart_rr_ratio', 1.5)} Ratio)")
        self.btn_rr.clicked.connect(self.chart_widget.toggle_rr_tool)
        toolbar.addWidget(self.btn_rr)
        
        self.btn_magnet = QPushButton("🧲")
        self.btn_magnet.setCheckable(True)
        self.btn_magnet.setToolTip("Magnet Mode: Snap to Candle High/Low")
        self.btn_magnet.clicked.connect(self.chart_widget.toggle_magnet)
        toolbar.addWidget(self.btn_magnet)

        # Separator
        line3 = QFrame()
        line3.setFrameShape(QFrame.VLine)
        line3.setFrameShadow(QFrame.Sunken)
        line3.setStyleSheet("background-color: #444746; margin: 4px;")
        toolbar.addWidget(line3)

        # Drawing tools
        self.btn_trend = QPushButton("Trend")
        self.btn_trend.setCheckable(True)
        self.btn_trend.setToolTip("Draw Trendline: click & drag two points")
        self.btn_trend.clicked.connect(lambda checked: self.chart_widget.toggle_draw_tool('trend' if checked else None))
        toolbar.addWidget(self.btn_trend)

        self.btn_hline = QPushButton("HLine")
        self.btn_hline.setCheckable(True)
        self.btn_hline.setToolTip("Draw Horizontal Line: click to place")
        self.btn_hline.clicked.connect(lambda checked: self.chart_widget.toggle_draw_tool('hline' if checked else None))
        toolbar.addWidget(self.btn_hline)

        self.btn_vline = QPushButton("VLine")
        self.btn_vline.setCheckable(True)
        self.btn_vline.setToolTip("Draw Vertical Line: click to place")
        self.btn_vline.clicked.connect(lambda checked: self.chart_widget.toggle_draw_tool('vline' if checked else None))
        toolbar.addWidget(self.btn_vline)
        
        self.btn_fib = QPushButton("Fib")
        self.btn_fib.setCheckable(True)
        self.btn_fib.setToolTip("Fibonacci Retracement")
        self.btn_fib.clicked.connect(lambda checked: self.chart_widget.toggle_draw_tool('fib' if checked else None))
        toolbar.addWidget(self.btn_fib)

        self.btn_rect = QPushButton("Rect")
        self.btn_rect.setCheckable(True)
        self.btn_rect.setToolTip("Rectangle / Zone")
        self.btn_rect.clicked.connect(lambda checked: self.chart_widget.toggle_draw_tool('rect' if checked else None))
        toolbar.addWidget(self.btn_rect)
        
        self.btn_measure = QPushButton("📏")
        self.btn_measure.setCheckable(True)
        self.btn_measure.setToolTip("Measure Tool: Click & Drag to measure pips/time")
        self.btn_measure.clicked.connect(lambda checked: self.chart_widget.toggle_draw_tool('measure' if checked else None))
        toolbar.addWidget(self.btn_measure)
        
        toolbar.addStretch()
        
        self.btn_clear = QPushButton("📋")
        self.btn_clear.setToolTip("Manage Drawings")
        self.btn_clear.setFixedSize(32, 32)
        self.btn_clear.clicked.connect(self.on_clear_drawings)
        toolbar.addWidget(self.btn_clear)
        
        layout.addWidget(toolbar_frame)
        layout.addWidget(self.chart_widget)
        
        self.load_geometry()

    def update_data(self, candles, positions, bid, ask, contract_size, history):
        self.chart_widget.update_data(candles, positions, bid, ask, contract_size, history)

    def on_tool_finished(self):
        self.btn_trend.setChecked(False)
        self.btn_hline.setChecked(False)
        self.btn_vline.setChecked(False)
        self.btn_fib.setChecked(False)
        self.btn_rect.setChecked(False)
        self.btn_measure.setChecked(False)
        self.btn_rr.setChecked(False)

    def on_timeframe_changed(self, text):
        CONFIG["chart_timeframe"] = text
        save_config()

    def on_chart_mode_toggled(self, checked):
        if checked:
            self.btn_chart_mode.setText("🕯")
            self.btn_chart_mode.setToolTip("Switch to Candle Chart")
            self.chart_widget.set_mode("line")
        else:
            self.btn_chart_mode.setText("📈")
            self.btn_chart_mode.setToolTip("Switch to Line Chart")
            self.chart_widget.set_mode("candle")

    def on_shift_toggled(self, checked):
        self.chart_widget.set_chart_shift(checked)

    def on_crosshair_toggled(self, checked):
        if checked:
            self.btn_crosshair.setText("✛")
            self.btn_crosshair.setToolTip("Disable Crosshair")
            self.chart_widget.set_crosshair_mode(True)
        else:
            self.btn_crosshair.setText("↗")
            self.btn_crosshair.setToolTip("Enable Crosshair")
            self.chart_widget.set_crosshair_mode(False)

    def on_clear_drawings(self):
        dlg = DrawingManagerDialog(self.chart_widget, self)
        dlg.exec()

    def load_geometry(self):
        try:
            if os.path.exists("chart_window_state.json"):
                with open("chart_window_state.json", "r") as f:
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
            with open("chart_window_state.json", "w") as f:
                json.dump(data, f)
        except Exception:
            pass
        self.closed.emit()
        super().closeEvent(event)