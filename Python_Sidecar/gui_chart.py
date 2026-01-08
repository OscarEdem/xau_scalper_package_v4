import os
import json
import time
import bisect
from datetime import datetime
from PySide6.QtWidgets import (QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,  # type: ignore
                               QLabel, QComboBox, QPushButton, QSizePolicy, QMenu, QFileDialog, QApplication, QToolTip, QMessageBox, QFrame)
from PySide6.QtCore import Qt, QByteArray, QRectF, QPointF, Signal # type: ignore
from PySide6.QtGui import QColor, QPainter, QBrush, QPen, QPainterPath, QFont, QPolygonF # type: ignore
from config import CONFIG, state, save_config
from gui_styles import GLOBAL_STYLESHEET
from gui_dialogs import ModernToast

class CandleChartWidget(QWidget):
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
            except Exception:
                continue

        delete_action = None
        if delete_rr_idx != -1 or delete_draw_idx != -1:
            delete_action = menu.addAction("Delete Object")

        action = menu.exec(event.globalPos())

        if action == copy_action:
            QApplication.clipboard().setText(f"{price:.2f}")
            ModernToast.show_message(self, f"Price {price:.2f} Copied", style="success")
        elif action == snap_action:
            self.save_snapshot()
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
        # Right Click to Delete R:R
        if event.button() == Qt.RightButton:
            y = event.y()
            x = event.x()
            
            # Find which RR drawing to delete (distance-based)
            delete_idx = -1
            tol = 10
            for i in range(len(self.rr_drawings) - 1, -1, -1):
                rr = self.rr_drawings[i]
                xs = self.get_x_from_time(rr.get('start_ts', 0))
                xe = self.get_x_from_time(rr.get('end_ts', 0))
                if xs > xe: xs, xe = xe, xs

                try:
                    y_entry = self.get_y_from_price(rr['entry'])
                    y_sl = self.get_y_from_price(rr['sl'])
                    y_tp = self.get_y_from_price(rr['tp'])
                except Exception:
                    continue

                d_entry = self._distance_point_to_segment(x, y, xs, y_entry, xe, y_entry)
                d_sl = self._distance_point_to_segment(x, y, xs, y_sl, xe, y_sl)
                d_tp = self._distance_point_to_segment(x, y, xs, y_tp, xe, y_tp)

                if min(d_entry, d_sl, d_tp) <= tol:
                    delete_idx = i
                    break

            if delete_idx != -1:
                self.rr_drawings.pop(delete_idx)
                self.save_rr_drawings()
                self.update()
                return

            # If not found in rr_drawings, check generic drawings (trend/hline/vline)
            d_delete = -1
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
                        # Distance from point to segment
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
                        if dist <= tol:
                            d_delete = i
                            break
                    elif t == 'hline':
                        yline = self.get_y_from_price(dr.get('y1', 0))
                        if abs(y - yline) <= tol:
                            d_delete = i
                            break
                    elif t == 'vline':
                        xline = self.get_x_from_time(dr.get('x1', 0))
                        if abs(x - xline) <= tol:
                            d_delete = i
                            break
                except Exception:
                    continue

            if d_delete != -1:
                self.drawings.pop(d_delete)
                self.save_drawings()
                self.update()
                return

            return # Nothing matched; finished right-click handling

        if event.button() == Qt.LeftButton:
            if event.x() > self.width() - 60:
                self.is_scaling = True
                self.last_scale_y = event.y()
                self.setCursor(Qt.SizeVerCursor)
            else:
                # If a drawing tool is active, start a new drawing
                if self.draw_tool in ("trend", "hline", "vline"):
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
                        self.rr_drag_idx = i
                        self.rr_drag_handle = 'width'
                        return

                    if self.is_near_line(y, rr['tp'], x, xs, xe):
                        self.rr_drag_idx = i
                        self.rr_drag_handle = 'tp'
                        return
                    elif self.is_near_line(y, rr['sl'], x, xs, xe):
                        self.rr_drag_idx = i
                        self.rr_drag_handle = 'sl'
                        return
                    elif self.is_near_line(y, rr['entry'], x, xs, xe):
                        self.rr_drag_idx = i
                        self.rr_drag_handle = 'entry'
                        self.rr_drag_offset = rr['entry'] - p_mouse
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
                if not self.crosshair_enabled:
                    self.setCursor(Qt.ClosedHandCursor)

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.LeftButton:
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
            
            self.rr_drag_idx = -1
            self.rr_drag_handle = None
            self.rr_drawing = False
            # Finalize generic drawing
            if self.drawing_temp is not None:
                # If end not set, use current mouse
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
                # Hover effect for R:R lines
                y = event.y()
                x = event.x()
                hover = False
                resize_hover = False
                for rr in self.rr_drawings:
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
                elif self.crosshair_enabled:
                    self.setCursor(Qt.ArrowCursor)
                else:
                    self.setCursor(Qt.ArrowCursor)

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
        painter.fillRect(self.rect(), QColor("#2E3440"))
        
        total = len(self.candles)
        if total == 0:
            painter.setPen(QColor("#D8DEE9"))
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
        painter.setPen(QPen(QColor("#3B4252"), 1, Qt.DotLine))
        steps = max(5, int(h / 40))
        for i in range(steps):
            p = min_price + (price_range * i / (steps - 1))
            y = to_y(p)
            painter.drawLine(0, int(y), int(chart_w), int(y))
            painter.setPen(QColor("#D8DEE9"))
            painter.drawText(int(chart_w) + 5, int(y) + 5, f"{p:.2f}")
            painter.setPen(QPen(QColor("#3B4252"), 1, Qt.DotLine))

        # X-Axis Grid (Time)
        if self.visible_count > 0:
            x_steps = max(5, int(chart_w / 100))
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
                idx = int(start_idx + i)
                if 0 <= idx < total:
                    cx = i * candle_w + candle_w / 2
                    painter.setPen(QPen(QColor("#3B4252"), 1, Qt.DotLine))
                    painter.drawLine(int(cx), 0, int(cx), int(h - 20))
                    
                    # use pixel->time mapping so fractional shifts (eg. W1) update labels
                    ts = self.get_time_from_x(cx)
                    dt_str = datetime.fromtimestamp(int(ts)).strftime(fmt)
                    painter.setPen(QColor("#D8DEE9"))
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
                    color = QColor("#A3BE8C") if is_bull else QColor("#BF616A")
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
                    color = QColor("#A3BE8C") if is_bull else QColor("#BF616A")
                    
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
            painter.setPen(QPen(QColor("#88C0D0"), 2))
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
                painter.setPen(QPen(QColor("#ECEFF4"), 1, Qt.SolidLine))
                painter.drawLine(int(x_start), int(y_entry), int(x_end), int(y_entry))
                
                # Resize Handle (Right Edge of Entry Line)
                painter.setBrush(QBrush(QColor("#ECEFF4")))
                painter.drawEllipse(QPointF(x_end, y_entry), 3, 3)
                
                # Labels
                painter.setPen(QColor("#ECEFF4"))
                risk_pips = risk * 10 if "XAU" in CONFIG["trade_symbol"] else risk * 10000 # Approx
                reward_pips = reward * 10 if "XAU" in CONFIG["trade_symbol"] else reward * 10000
                
                painter.drawText(int(x_start) + 5, int(y_sl) + (15 if is_long else -5), f"SL: {sl:.2f} (-{risk_pips:.1f} pips)")
                painter.drawText(int(x_start) + 5, int(y_tp) + (-5 if is_long else 15), f"TP: {tp:.2f} (+{reward_pips:.1f} pips)")
                
                mid_y = (y_entry + y_tp) / 2
                painter.drawText(int(x_start + width/2 - 20), int(mid_y), f"R:R 1:{ratio:.2f}")

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

            pen = QPen(QColor("#D08770"), 2, Qt.SolidLine)
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

        # Draw temp drawing if present
        if self.drawing_temp is not None:
            dt = self.drawing_temp
            try:
                tx1 = self.get_x_from_time(dt.get('x1', 0))
                tx2 = self.get_x_from_time(dt.get('x2', tx1))
                ty1 = to_y(dt.get('y1', 0))
                ty2 = to_y(dt.get('y2', 0))
            except Exception:
                tx1 = tx2 = ty1 = ty2 = 0
            painter.setPen(QPen(QColor("#88C0D0"), 1, Qt.DashLine))
            if dt.get('type') == 'trend':
                painter.drawLine(int(tx1), int(ty1), int(tx2), int(ty2))
            elif dt.get('type') == 'hline':
                painter.drawLine(0, int(ty1), int(w), int(ty1))
            elif dt.get('type') == 'vline':
                painter.drawLine(int(tx1), 0, int(tx1), int(h))


        # Draw Positions
        for pos in self.positions:
            py = to_y(pos['price_open'])
            is_buy = (pos['type'] == "BUY")
            color = QColor("#A3BE8C") if is_buy else QColor("#BF616A")
            painter.setPen(QPen(color, 1, Qt.DashLine))
            painter.drawLine(0, int(py), int(chart_w), int(py))
            painter.drawText(5, int(py) - 5, f"{pos['type']} {pos['volume']} @ {pos['price_open']:.2f}")
            
            # Draw SL
            if pos['sl'] > 0:
                y_sl = to_y(pos['sl'])
                painter.setPen(QPen(QColor("#BF616A"), 1, Qt.DashDotLine))
                painter.drawLine(0, int(y_sl), int(chart_w), int(y_sl))

                sl_diff = pos['sl'] - pos['price_open'] if is_buy else pos['price_open'] - pos['sl']
                sl_loss = sl_diff * pos['volume'] * self.contract_size
                painter.drawText(int(chart_w) - 90, int(y_sl) - 5, f"SL (${sl_loss:.2f})")

            # Draw TP
            if pos['tp'] > 0:
                y_tp = to_y(pos['tp'])
                painter.setPen(QPen(QColor("#A3BE8C"), 1, Qt.DashDotLine))
                painter.drawLine(0, int(y_tp), int(chart_w), int(y_tp))

                tp_diff = pos['tp'] - pos['price_open'] if is_buy else pos['price_open'] - pos['tp']
                tp_profit = tp_diff * pos['volume'] * self.contract_size
                painter.drawText(int(chart_w) - 90, int(y_tp) - 5, f"TP (${tp_profit:.2f})")

        # Draw Bid/Ask
        if self.bid > 0:
            y_bid = to_y(self.bid)
            color_bid = QColor("#BF616A")
            painter.setPen(QPen(color_bid, 1, Qt.DotLine))
            painter.drawLine(0, int(y_bid), int(w), int(y_bid))
            
            lbl_rect = QRectF(chart_w, y_bid - 10, right_margin, 20)
            painter.fillRect(lbl_rect, color_bid)
            painter.setPen(QColor("#ECEFF4"))
            painter.drawText(lbl_rect, Qt.AlignCenter, f"{self.bid:.2f}")

        if self.ask > 0:
            y_ask = to_y(self.ask)
            color_ask = QColor("#5E81AC")
            painter.setPen(QPen(color_ask, 1, Qt.DotLine))
            painter.drawLine(0, int(y_ask), int(w), int(y_ask))
            
            lbl_rect = QRectF(chart_w, y_ask - 10, right_margin, 20)
            painter.fillRect(lbl_rect, color_ask)
            painter.setPen(QColor("#ECEFF4"))
            painter.drawText(lbl_rect, Qt.AlignCenter, f"{self.ask:.2f}")
            
        # Current Price Line
        last = self.candles[-1] # Always show current price of latest candle
        curr_y = to_y(last['close'])
        painter.setPen(QPen(QColor("#88C0D0"), 1, Qt.DashLine))
        painter.drawLine(0, int(curr_y), int(chart_w), int(curr_y))
        
        # Current Price Label
        lbl_rect = QRectF(chart_w, curr_y - 10, right_margin, 20)
        painter.fillRect(lbl_rect, QColor("#88C0D0"))
        painter.setPen(QColor("#2E3440"))
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
            text_color = QColor("#A3BE8C") if is_bull else QColor("#BF616A")

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
                painter.setPen(QPen(QColor("#ECEFF4"), 1, Qt.DotLine))
                
                # Crosshair Lines
                painter.drawLine(mx, 0, mx, h)
                painter.drawLine(0, my, int(chart_w), my)
                
                # Price Label (Right Axis)
                draw_h = h - 20
                if draw_h > 0:
                    pct = (h - 10 - my) / draw_h
                    price = min_price + (pct * price_range)
                    lbl_rect = QRectF(chart_w, my - 10, right_margin, 20)
                    painter.fillRect(lbl_rect, QColor("#4C566A"))
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
                    painter.fillRect(t_rect, QColor("#4C566A"))
                    painter.drawText(t_rect, Qt.AlignCenter, dt_str)

    def draw_header(self, painter, w):
        # High-precision Price Display
        if self.bid > 0 and self.ask > 0:
            spread = self.ask - self.bid
            
            font = QFont("Consolas", 10)
            font.setBold(True)
            painter.setFont(font)
            
            # Background for header
            bg_rect = QRectF(10, 10, 300, 60)
            painter.setBrush(QBrush(QColor(46, 52, 64, 200)))
            painter.setPen(Qt.NoPen)
            painter.drawRoundedRect(bg_rect, 4, 4)
            
            painter.setPen(QColor("#D8DEE9"))
            painter.drawText(20, 30, f"{CONFIG['trade_symbol']} ({CONFIG.get('chart_timeframe', 'M1')})")
            
            painter.setPen(QColor("#A3BE8C"))
            painter.drawText(20, 50, f"Bid: {self.bid:.3f}")
            
            painter.setPen(QColor("#BF616A"))
            painter.drawText(140, 50, f"Ask: {self.ask:.3f}")
            
            painter.setPen(QColor("#EBCB8B"))
            painter.drawText(20, 65, f"Spread: {spread:.3f}")

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
            line_color = QColor("#A3BE8C") if profit >= 0 else QColor("#BF616A")
            line_color.setAlpha(150)
            painter.setPen(QPen(line_color, 1, Qt.DashLine))
            painter.drawLine(int(x_entry), int(y_entry), int(x_exit), int(y_exit))

            arrow_size = 8
            is_buy = trade.get('type') == 'BUY'

            # Draw entry arrow
            color = QColor("#A3BE8C") if is_buy else QColor("#BF616A")
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
            
            painter.setPen(QPen(QColor("#B48EAD"), 2, Qt.SolidLine)) # Purple
            
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
            color = QColor("#B48EAD")
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
                painter.setPen(QPen(QColor("#88C0D0"), 1, Qt.DashDotLine)) # Cyan
                painter.drawLine(0, int(y), int(w), int(y))
                painter.drawText(10, int(y)-5, "Avg BUY")
            
            if sells:
                avg_sell = sum(p['price_open'] * p['volume'] for p in sells) / sum(p['volume'] for p in sells)
                y = to_y(avg_sell)
                painter.setPen(QPen(QColor("#D08770"), 1, Qt.DashDotLine)) # Orange
                painter.drawLine(0, int(y), int(w), int(y))
                painter.drawText(10, int(y)-5, "Avg SELL")

class ChartWindow(QMainWindow):
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
        
        # Chart Toolbar
        toolbar_frame = QFrame()
        toolbar_frame.setFixedHeight(48)
        toolbar_frame.setStyleSheet("""
            QFrame { background-color: #2E3440; border-bottom: 1px solid #434C5E; }
            QLabel { font-weight: bold; color: #88C0D0; font-size: 9pt; }
            QPushButton {
                background-color: transparent;
                border: 1px solid transparent;
                border-radius: 4px;
                color: #D8DEE9;
                font-size: 10pt;
                padding: 4px;
                font-weight: bold;
            }
            QPushButton:hover { background-color: #3B4252; border: 1px solid #4C566A; }
            QPushButton:checked { background-color: #88C0D0; color: #2E3440; border: 1px solid #88C0D0; }
            QComboBox {
                background-color: #3B4252;
                border: 1px solid #4C566A;
                border-radius: 4px;
                padding: 4px 10px;
                color: #ECEFF4;
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
        line1.setStyleSheet("background-color: #434C5E; margin: 4px;")
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
        line2.setStyleSheet("background-color: #434C5E; margin: 4px;")
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
        line3.setStyleSheet("background-color: #434C5E; margin: 4px;")
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
        
        toolbar.addStretch()
        
        self.btn_clear = QPushButton("🗑️")
        self.btn_clear.setToolTip("Clear All Drawings")
        self.btn_clear.setFixedSize(32, 32)
        self.btn_clear.clicked.connect(self.on_clear_drawings)
        toolbar.addWidget(self.btn_clear)
        
        layout.addWidget(toolbar_frame)
        layout.addWidget(self.chart_widget)
        
        self.load_geometry()

    def update_data(self, candles, positions, bid, ask, contract_size, history):
        self.chart_widget.update_data(candles, positions, bid, ask, contract_size, history)

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
        if QMessageBox.question(self, "Confirm", "Clear all drawings?", QMessageBox.Yes | QMessageBox.No) == QMessageBox.Yes:
            self.chart_widget.clear_all_drawings()
            ModernToast.show_message(self, "All Drawings Cleared", style="info")

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
        super().closeEvent(event)