from typing import Dict, Any
from datetime import datetime, timedelta
import urllib.request
import json
import MetaTrader5 as mt5 # type: ignore
from PySide6.QtCore import QObject, QTimer, Signal, Slot, QThread # type: ignore
from config import CONFIG, state
from mt5_interface import get_chart_data

class MT5DataWorker(QObject):
    """
    Handles all MT5 read operations in a separate thread to prevent UI freezing.
    """
    data_updated = Signal(dict)

    def __init__(self):
        super().__init__()
        self.timer = QTimer(self)
        self.timer.setInterval(300)  # Reduced to 2Hz to save resources
        self.timer.timeout.connect(self.fetch_data)
        self.loop_counter = 0
        self.history_cache = []
        self.total_pl_cache = 0.0
        self.last_valid_atr = 0.0
        self.chart_data_enabled = False

    @Slot()
    def start_working(self):
        self.timer.start()

    @Slot()
    def stop_working(self):
        self.timer.stop()

    @Slot(bool)
    def set_chart_data_enabled(self, enabled):
        self.chart_data_enabled = enabled

    def fetch_data(self):
        if not state.get("running", False):
            return
        self.loop_counter += 1

        data: Dict[str, Any] = {}
        
        # 1. Connection & Terminal Info
        term = mt5.terminal_info()
        if term is None:
            # Attempt to initialize if not connected
            if mt5.initialize():
                term = mt5.terminal_info()
                mt5.symbol_select(CONFIG["trade_symbol"], True)

        acct = mt5.account_info()
        
        data["connected"] = term.connected if term else False
        
        term_allowed = term.trade_allowed if term else False
        acct_allowed = acct.trade_expert if acct else False
        data["trade_allowed"] = term_allowed and acct_allowed
        data["ping"] = term.ping_last // 1000 if term else 0
        
        # 2. Price Data
        symbol = CONFIG["trade_symbol"]
        
        # Ensure symbol is selected in Market Watch (fixes 0.00 issue)
        if not mt5.symbol_info(symbol):
            mt5.symbol_select(symbol, True)
            
        symbol_info = mt5.symbol_info(symbol)
        tick = mt5.symbol_info_tick(symbol)
        if tick:
            data["bid"] = tick.bid
            data["ask"] = tick.ask
        else:
            data["bid"] = 0.0
            data["ask"] = 0.0
            
        # Fetch Daily Open for Price Change calculation
        d1_rates = mt5.copy_rates_from_pos(symbol, mt5.TIMEFRAME_D1, 0, 1)
        if d1_rates is not None and len(d1_rates) > 0:
            data["daily_open"] = float(d1_rates[0]['open'])
        else:
            data["daily_open"] = 0.0
        
        data["contract_size"] = symbol_info.trade_contract_size if symbol_info else 100.0

        # 3. Account Info
        acct = mt5.account_info()
        data["balance"] = acct.balance if acct else 0.0
        data["equity"] = acct.equity if acct else 0.0
        data["margin_used"] = acct.margin if acct else 0.0
        data["margin_mode"] = acct.margin_mode if acct else -1
        
        # 4. Positions
        positions = mt5.positions_get(symbol=symbol)
        orders = mt5.orders_get(symbol=symbol)
        pos_list = []
        counts = {"scalp": 0, "swing": 0, "manual": 0}
        total_open_pl = 0.0
        net_exposure = 0.0
        
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
                    
                    direction = 1 if p.type == mt5.ORDER_TYPE_BUY else -1
                    net_exposure += (p.volume * direction)
                    
                    pos_list.append({
                        "ticket": p.ticket,
                        "type": "BUY" if p.type == mt5.ORDER_TYPE_BUY else "SELL",
                        "volume": p.volume,
                        "profit": net_pl,
                        "magic": p.magic,
                        "sl": p.sl,
                        "tp": p.tp,
                        "symbol": p.symbol,
                        "price_open": p.price_open
                    })
        
        if orders:
            for o in orders:
                if o.magic == CONFIG["magic_number"]: counts["scalp"] += 1
                elif o.magic == CONFIG["magic_number"] + 1: counts["swing"] += 1
                elif o.magic == 0: counts["manual"] += 1
                
                t_map = {
                    mt5.ORDER_TYPE_BUY_LIMIT: "BUY LIMIT",
                    mt5.ORDER_TYPE_SELL_LIMIT: "SELL LIMIT",
                    mt5.ORDER_TYPE_BUY_STOP: "BUY STOP",
                    mt5.ORDER_TYPE_SELL_STOP: "SELL STOP",
                    mt5.ORDER_TYPE_BUY_STOP_LIMIT: "BUY STOP LIMIT",
                    mt5.ORDER_TYPE_SELL_STOP_LIMIT: "SELL STOP LIMIT"
                }
                
                pos_list.append({
                    "ticket": o.ticket,
                    "type": t_map.get(o.type, "ORDER"),
                    "volume": o.volume_current,
                    "profit": 0.0,
                    "magic": o.magic,
                    "sl": o.sl,
                    "tp": o.tp,
                    "symbol": o.symbol,
                    "price_open": o.price_open,
                    "is_pending": True
                })
        
        data["positions"] = pos_list
        data["counts"] = counts
        data["open_pl"] = total_open_pl
        data["net_exposure"] = net_exposure
        
        # 5. History (Last 24h)
        # Optimization: Throttle history fetching to once per second (every 5 ticks)
        if self.loop_counter % 5 == 0:
            from_d = datetime.now() - timedelta(hours=24)
            deals = mt5.history_deals_get(from_d, datetime.now())
            
            total_pl_24h = 0.0
            processed_trades = []
            
            if deals:
                deals_by_pos = {}
                for d in deals:
                    if d.symbol == symbol and d.magic in [CONFIG["magic_number"], CONFIG["magic_number"]+1, 0]:
                        deals_by_pos.setdefault(d.position_id, []).append(d)

                for pos_id, pos_deals in deals_by_pos.items():
                    entry_deal = next((d for d in pos_deals if d.entry == mt5.DEAL_ENTRY_IN), None)
                    exit_deal = next((d for d in pos_deals if d.entry == mt5.DEAL_ENTRY_OUT), None)

                    if entry_deal and exit_deal:
                        profit = sum(d.profit for d in pos_deals)
                        total_pl_24h += profit
                        
                        processed_trades.append({
                            "position_id": pos_id,
                            "entry_time": entry_deal.time,
                            "entry_price": entry_deal.price,
                            "exit_time": exit_deal.time,
                            "exit_price": exit_deal.price,
                            "volume": exit_deal.volume,
                            "profit": profit,
                            "type": "BUY" if entry_deal.type == mt5.ORDER_TYPE_BUY else "SELL",
                            "duration": exit_deal.time - entry_deal.time,
                        })
            
            # Sort by exit time descending for display
            processed_trades.sort(key=lambda x: x['exit_time'], reverse=True)
            self.history_cache = processed_trades
            self.total_pl_cache = total_pl_24h
        
        data["history"] = self.history_cache
        data["total_pl_24h"] = self.total_pl_cache
        
        # 6. Logs (Consume from state)
        with state["lock"]:
            if state.get("gui_logs"):
                data["log_updates"] = state["gui_logs"][:]
                state["gui_logs"] = []
            else:
                data["log_updates"] = []
                
        # 7. Status Text
        data["status_text"] = state.get("status_text", "")
        data["connection_time"] = state.get("connection_time")
        data["last_signal_ts"] = state.get("last_signal_ts", 0)
        
        # Priority 1: Calculate ATR locally from live market data
        local_atr = 0.0
        if data["connected"]:
            try:
                tf_str = CONFIG.get("atr_timeframe", "M5")
                tf_map = {
                    "M1": mt5.TIMEFRAME_M1, "M5": mt5.TIMEFRAME_M5, "M15": mt5.TIMEFRAME_M15,
                    "M30": mt5.TIMEFRAME_M30, "H1": mt5.TIMEFRAME_H1, "H4": mt5.TIMEFRAME_H4, "D1": mt5.TIMEFRAME_D1
                }
                tf = tf_map.get(tf_str, mt5.TIMEFRAME_M5)
                
                # Fetch last 20 candles to calculate 14-period ATR
                rates = mt5.copy_rates_from_pos(symbol, tf, 0, 20)
                if rates is not None and len(rates) >= 15:
                    tr_sum = 0.0
                    count = 0
                    # Calculate TR for the last 14 periods available
                    for i in range(len(rates) - 14, len(rates)):
                        h = float(rates[i]['high'])
                        l = float(rates[i]['low'])
                        pc = float(rates[i-1]['close'])
                        tr = max(h - l, abs(h - pc), abs(l - pc))
                        tr_sum += tr
                        count += 1
                    if count > 0:
                        local_atr = tr_sum / count
            except Exception:
                pass

        # Determine final ATR (Local > Server > Cache)
        server_atr = state.get("server_atr", 0.0)
        
        if local_atr > 0:
            data["server_atr"] = local_atr
            self.last_valid_atr = local_atr
        elif server_atr > 0:
            data["server_atr"] = server_atr
            self.last_valid_atr = server_atr
        elif self.last_valid_atr > 0:
            data["server_atr"] = self.last_valid_atr
        else:
            data["server_atr"] = 0.0

        latest = state.get("latest_signal")
        if latest:
            etype = latest.get("entryType", "?")
            eprice = latest.get("entryPrice", latest.get("price", 0.0))
            data["last_signal"] = f"{etype.upper()} {eprice}"
            data["last_signal_confidence"] = latest.get("convictionScore", latest.get("conviction", 0.0))
            data["last_signal_class"] = latest.get("classification", "Unknown")

        # 8. Chart Data
        if self.chart_data_enabled:
            tf = CONFIG.get("chart_timeframe", "M1")
            data["chart_data"] = get_chart_data(symbol, timeframe_str=tf, count=500)
        else:
            data["chart_data"] = []

        self.data_updated.emit(data)

class SignalHistoryWorker(QThread):
    data_received = Signal(dict)
    error_occurred = Signal(str)

    def __init__(self, page=1, limit=50):
        super().__init__()
        self.page = page
        self.limit = limit
        self.url = CONFIG["server_url"]

    def run(self):
        try:
            base = self.url.rstrip('/')
            endpoint = f"{base}/signals/paginated?page={self.page}&limit={self.limit}"
            
            req = urllib.request.Request(endpoint)
            req.add_header('User-Agent', 'XAUBot/4.0')
            
            with urllib.request.urlopen(req, timeout=10) as response:
                if response.status == 200:
                    raw = response.read().decode('utf-8')
                    data = json.loads(raw)
                    self.data_received.emit(data)
                else:
                    self.error_occurred.emit(f"HTTP Error: {response.status}")
        except Exception as e:
            self.error_occurred.emit(f"Connection Error: {str(e)}")