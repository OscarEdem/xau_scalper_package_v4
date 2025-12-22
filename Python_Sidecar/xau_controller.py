import MetaTrader5 as mt5 # type: ignore
import time
import threading
import websocket # type: ignore
import json
import tkinter as tk
from tkinter import ttk
from datetime import datetime, timedelta

# --- DEFAULT CONFIGURATION ---
CONFIG = {
    "server_url": "https://gold-ml-base-server.onrender.com",
    "signal_symbol": "XAUUSD",
    "trade_symbol": "XAUUSD",
    "magic_number": 1337,
    "update_interval": 1.0,
    
    # -- Mutable Settings (Controlled by GUI) --
    "fixed_lot_size": 0.01,
    "swing_lot_size": 0.01,
    "max_entries": 3,
    "scalp_mode": True,
    "swing_mode": True,
    "force_market": False,
    "use_trailing": True,
    
    # -- Filters --
    "max_spread_points": 162,
    "min_velocity_pips": 0.5,
    "velocity_lookback_sec": 10,
    
    # -- Stops --
    "sl_tighten_pips": 100.0,
    "trailing_start_pips": 20.0,
    "trailing_dist_pips": 70.0,
    "trailing_step_pips": 2.0,
    "be_trigger_pips": 15.0,
    "be_offset_pips": 1.0,
    "stagnation_sec": 60, 
    "stagnation_reduce_pct": 20.0,
    "use_stagnation": True,
    "use_dynamic_stag": True,
    "stag_time_mult": 2.0
}

# --- GLOBAL STATE ---
state = {
    "latest_signal": None,
    "last_processed_id": None,
    "server_atr": 0.0,
    "running": True,
    "status_text": "Initializing...",
    "lock": threading.Lock(),
    "last_manage_time": 0
}

# --- MT5 CONSTANTS (Fallback) ---
SYMBOL_FILLING_FOK = 1
SYMBOL_FILLING_IOC = 2

# =============================================================================
# HELPER FUNCTIONS
# =============================================================================
def get_point():
    info = mt5.symbol_info(CONFIG["trade_symbol"])
    return info.point if info else 0.01

def get_ask_bid():
    tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
    if tick is None:
        if mt5.symbol_select(CONFIG["trade_symbol"], True):
            tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
    if tick:
        return tick.ask, tick.bid
    return 0.0, 0.0

def calculate_velocity():
    now = datetime.now()
    past = now - timedelta(seconds=CONFIG["velocity_lookback_sec"])
    ticks = mt5.copy_ticks_range(CONFIG["trade_symbol"], past, now, mt5.COPY_TICKS_INFO)
    if ticks is None or len(ticks) == 0: return 0.0
    bids = [t[1] for t in ticks]
    return (max(bids) - min(bids)) / (get_point() * 10.0)

# =============================================================================
# TRADE EXECUTION
# =============================================================================
def execute_trade(signal):
    entry_type = signal.get("entryType", "none")
    classification = signal.get("classification", "scalp")
    reason = signal.get("reason", "")
    rec_type = signal.get("recommendedOrderType", "market")
    limit_price = float(signal.get("limitOrderPrice", 0.0))
    entry_price = float(signal.get("entryPrice", 0.0))
    # Fallback: Server always provides entryPrice, use it if limitOrderPrice is 0
    if limit_price == 0.0:
        limit_price = entry_price
    expiration_sec = signal.get("expirationSeconds")
    
    # Simple Filters
    ask, bid = get_ask_bid()
    if ask == 0.0 or bid == 0.0:
        print(f"[EXEC] Error: Invalid market price (0.0) for {CONFIG['trade_symbol']}. Check Market Watch.")
        return

    spread = (ask - bid) / get_point()
    if spread > CONFIG["max_spread_points"]:
        print(f"[FILTER] Spread too high: {spread}")
        return

    # Check Symbol Info for Filling Mode
    symbol_info = mt5.symbol_info(CONFIG["trade_symbol"])
    if symbol_info is None:
        print(f"[EXEC] Error: Symbol {CONFIG['trade_symbol']} not found")
        return

    # Prepare Order
    raw_sl = float(signal.get("slPrice", 0.0))
    raw_tp = float(signal.get("tp1Price", 0.0))
    
    # Default Market Execution
    is_long = "long" in entry_type
    mt5_type = mt5.ORDER_TYPE_BUY if is_long else mt5.ORDER_TYPE_SELL
    price = ask if is_long else bid
    action = mt5.TRADE_ACTION_DEAL
    
    # Limit Order Logic
    if not CONFIG["force_market"]:
        if "limit" in rec_type and is_long and ("long" in rec_type or "buy" in rec_type):
            mt5_type = mt5.ORDER_TYPE_BUY_LIMIT
            price = limit_price
            action = mt5.TRADE_ACTION_PENDING
        elif "limit" in rec_type and not is_long and ("short" in rec_type or "sell" in rec_type):
            mt5_type = mt5.ORDER_TYPE_SELL_LIMIT
            price = limit_price
            action = mt5.TRADE_ACTION_PENDING
    elif "limit" in rec_type:
        print(f"[EXEC] Force Market: Converting {rec_type} to Market Order")
    
    # Loop for Max Entries
    entries = 1 # Simplified for basic version
    
    # Determine Trade Parameters based on classification
    is_swing = "swing" in classification
    is_reentry = "reentry" in classification
    
    volume = CONFIG["swing_lot_size"] if is_swing else CONFIG["fixed_lot_size"]
    magic = CONFIG["magic_number"] + 1 if is_swing else CONFIG["magic_number"]
    
    if is_reentry:
        comment = "SwingRe"
    elif is_swing:
        comment = "PySwing"
    else:
        comment = "PyScalp"
    
    # Expiration Logic
    type_time = mt5.ORDER_TIME_GTC
    order_expiration = 0
    if action == mt5.TRADE_ACTION_PENDING and expiration_sec:
        type_time = mt5.ORDER_TIME_SPECIFIED
        order_expiration = int(time.time() + int(expiration_sec))

    # Normalize Price and Stops (Align to Tick Size)
    if symbol_info:
        tick_size = symbol_info.trade_tick_size
        digits = symbol_info.digits
        
        def normalize(val):
            return round(round(val / tick_size) * tick_size, digits)
            
        price = normalize(price)
        if raw_sl > 0: raw_sl = normalize(raw_sl)
        if raw_tp > 0: raw_tp = normalize(raw_tp)
        
        # Check Limit Order Validity (Convert to Market if price crossed)
        if action == mt5.TRADE_ACTION_PENDING:
            curr_ask, curr_bid = get_ask_bid()
            if mt5_type == mt5.ORDER_TYPE_BUY_LIMIT and price >= curr_ask:
                print(f"[EXEC] Limit Buy {price} >= Ask {curr_ask}. Converting to Market.")
                action = mt5.TRADE_ACTION_DEAL
                mt5_type = mt5.ORDER_TYPE_BUY
            elif mt5_type == mt5.ORDER_TYPE_SELL_LIMIT and price <= curr_bid:
                print(f"[EXEC] Limit Sell {price} <= Bid {curr_bid}. Converting to Market.")
                action = mt5.TRADE_ACTION_DEAL
                mt5_type = mt5.ORDER_TYPE_SELL
        
    print(f"[DEBUG] Sending Order: {action} | Price: {price} | SL: {raw_sl} | TP: {raw_tp}")

    # Determine Filling Mode dynamically
    filling_type = mt5.ORDER_FILLING_FOK
    if action == mt5.TRADE_ACTION_PENDING:
        filling_type = mt5.ORDER_FILLING_RETURN
    else:
        # Market Order
        if symbol_info.filling_mode & SYMBOL_FILLING_IOC:
            filling_type = mt5.ORDER_FILLING_IOC
        elif symbol_info.filling_mode & SYMBOL_FILLING_FOK:
            filling_type = mt5.ORDER_FILLING_FOK
        else:
            filling_type = mt5.ORDER_FILLING_IOC # Fallback

    request = {
        "action": action,
        "symbol": CONFIG["trade_symbol"],
        "volume": volume,
        "type": mt5_type,
        "price": price,
        "sl": raw_sl,
        "tp": raw_tp,
        "deviation": 20,
        "magic": magic,
        "comment": comment,
        "type_time": type_time,
        "expiration": order_expiration,
        "type_filling": filling_type,
    }
    
    res = mt5.order_send(request)
    if res and res.retcode == mt5.TRADE_RETCODE_DONE:
        print(f"[EXEC] {comment} Open: {entry_type} | Vol: {volume} | {reason}")
    else:
        print(f"[EXEC] Error: {res.comment if res else 'Unknown'}")

# =============================================================================
# TRADE MANAGEMENT
# =============================================================================
def manage_positions():
    if not CONFIG["use_trailing"]: return
    positions = mt5.positions_get(symbol=CONFIG["trade_symbol"], magic=CONFIG["magic_number"])
    if not positions: return
    
    point = get_point()
    
    for pos in positions:
        # Basic Trailing Logic
        if pos.type == mt5.ORDER_TYPE_BUY:
            dist = CONFIG["trailing_dist_pips"] * 10 * point
            new_sl = mt5.symbol_info_tick(CONFIG["trade_symbol"]).bid - dist
            if new_sl > pos.sl + (CONFIG["trailing_step_pips"]*10*point):
                request = {"action": mt5.TRADE_ACTION_SLTP, "position": pos.ticket, "sl": new_sl, "tp": pos.tp}
                mt5.order_send(request)
        elif pos.type == mt5.ORDER_TYPE_SELL:
            dist = CONFIG["trailing_dist_pips"] * 10 * point
            new_sl = mt5.symbol_info_tick(CONFIG["trade_symbol"]).ask + dist
            if pos.sl == 0 or new_sl < pos.sl - (CONFIG["trailing_step_pips"]*10*point):
                request = {"action": mt5.TRADE_ACTION_SLTP, "position": pos.ticket, "sl": new_sl, "tp": pos.tp}
                mt5.order_send(request)

# =============================================================================
# GUI DASHBOARD
# =============================================================================
class DashboardGUI:
    def __init__(self, root):
        self.root = root
        self.root.title("XAU Scalper Command")
        self.root.geometry("300x400")
        self.root.attributes("-topmost", True)
        self.root.configure(bg="#1e1e1e")
        
        style = ttk.Style()
        style.theme_use('clam')
        style.configure("TLabel", background="#1e1e1e", foreground="white")
        style.configure("TButton", background="#333", foreground="white")
        
        # Status
        self.status_var = tk.StringVar(value="Connecting...")
        self.status_label = tk.Label(root, textvariable=self.status_var, font=("Segoe UI", 10, "bold"), bg="#1e1e1e", fg="white")
        self.status_label.pack(pady=10)
        
        # Price
        self.price_var = tk.StringVar(value="Waiting for tick...")
        ttk.Label(root, textvariable=self.price_var).pack()
        
        # Trade Counts
        self.counts_var = tk.StringVar(value="Scalp: 0 | Swing: 0")
        tk.Label(root, textvariable=self.counts_var, font=("Segoe UI", 9), bg="#1e1e1e", fg="#cccccc").pack(pady=2)

        ttk.Separator(root, orient='horizontal').pack(fill='x', pady=10)

        # Inputs
        self.create_input("Signal Sym:", "signal_symbol")
        self.create_input("Trade Sym:", "trade_symbol")
        self.create_input("Scalp Lot:", "fixed_lot_size")
        self.create_input("Swing Lot:", "swing_lot_size")
        self.create_input("Max Entries:", "max_entries")
        
        # Toggles
        self.create_toggle("Scalp Mode", "scalp_mode")
        self.create_toggle("Swing Mode", "swing_mode")
        self.create_toggle("Force Market Order", "force_market")
        self.create_toggle("Trailing Stop", "use_trailing")
        
        ttk.Separator(root, orient='horizontal').pack(fill='x', pady=10)
        
        # Close All
        tk.Button(root, text="CLOSE SCALP TRADES", bg="#cc0000", fg="white", font=("Segoe UI", 9, "bold"), command=lambda: self.close_trades(0)).pack(fill='x', padx=20, pady=5)
        tk.Button(root, text="CLOSE SWING TRADES", bg="#8b0000", fg="white", font=("Segoe UI", 9, "bold"), command=lambda: self.close_trades(1)).pack(fill='x', padx=20, pady=5)

        self.update_gui()

    def create_input(self, label, config_key):
        frame = ttk.Frame(self.root)
        frame.pack(fill='x', padx=20, pady=2)
        ttk.Label(frame, text=label).pack(side='left')
        
        val = CONFIG[config_key]
        var = tk.StringVar(value=str(val))
        is_float = isinstance(val, float)
        
        if isinstance(val, (int, float)):
            step = 0.01 if is_float else 1
            entry = ttk.Spinbox(frame, from_=0, to=1000, increment=step, textvariable=var, width=8)
        else:
            entry = ttk.Entry(frame, textvariable=var, width=8)
            
        entry.pack(side='right')
        def on_change(*args):
            try:
                val = var.get()
                if config_key in ["signal_symbol", "trade_symbol"]:
                    CONFIG[config_key] = val
                elif is_float:
                    CONFIG[config_key] = round(float(val), 2)
                else:
                    CONFIG[config_key] = int(float(val))
            except: pass
        var.trace("w", on_change)

    def create_toggle(self, label, config_key):
        frame = ttk.Frame(self.root)
        frame.pack(fill='x', padx=20, pady=2)
        var = tk.BooleanVar(value=CONFIG[config_key])
        def on_toggle(): CONFIG[config_key] = var.get()
        ttk.Checkbutton(frame, text=label, variable=var, command=on_toggle).pack(side='left')

    def close_trades(self, magic_offset=0):
        magic = CONFIG["magic_number"] + magic_offset
        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"], magic=magic)
        if positions:
            for pos in positions:
                type_close = mt5.ORDER_TYPE_SELL if pos.type == mt5.ORDER_TYPE_BUY else mt5.ORDER_TYPE_BUY
                price_close = mt5.symbol_info_tick(CONFIG["trade_symbol"]).bid if pos.type == mt5.ORDER_TYPE_BUY else mt5.symbol_info_tick(CONFIG["trade_symbol"]).ask
                req = {"action": mt5.TRADE_ACTION_DEAL, "symbol": CONFIG["trade_symbol"], "position": pos.ticket, "volume": pos.volume, "type": type_close, "price": price_close, "magic": magic}
                mt5.order_send(req)

    def update_gui(self):
        txt = state["status_text"]
        
        # Check Terminal AutoTrading Status
        term_info = mt5.terminal_info()
        if term_info and not term_info.trade_allowed:
            self.status_var.set("⚠️ AutoTrading Disabled in MT5")
            self.status_label.config(fg="orange")
        else:
            self.status_var.set(txt)
            if "Connected" in txt:
                self.status_label.config(fg="#00ff00") # Green
            elif "Error" in txt or "Disconnected" in txt or "Failed" in txt:
                self.status_label.config(fg="#ff3333") # Red Alert
            else:
                self.status_label.config(fg="yellow") # Connecting
            
        tick = mt5.symbol_info_tick(CONFIG["trade_symbol"])
        if tick: self.price_var.set(f"Bid: {tick.bid:.2f} | Ask: {tick.ask:.2f}")
        
        # Update Trade Counts
        positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
        scalp_count = 0
        swing_count = 0
        if positions:
            for pos in positions:
                if pos.magic == CONFIG["magic_number"]:
                    scalp_count += 1
                elif pos.magic == CONFIG["magic_number"] + 1:
                    swing_count += 1
        self.counts_var.set(f"Scalp: {scalp_count} | Swing: {swing_count}")
        
        self.root.after(500, self.update_gui)

# =============================================================================
# MAIN LOOP
# =============================================================================
def trading_loop():
    if not mt5.initialize():
        state["status_text"] = "MT5 Init Failed!"
        return
    
    # Ensure symbol is selected
    mt5.symbol_select(CONFIG["trade_symbol"], True)

    def on_open(ws):
        state["status_text"] = "Connected"

    def on_message(ws, message):
        try:
            decoder = json.JSONDecoder()
            pos = 0
            while pos < len(message):
                # Skip whitespace
                while pos < len(message) and message[pos].isspace():
                    pos += 1
                if pos >= len(message):
                    break
                
                try:
                    data, index = decoder.raw_decode(message, pos)
                    pos = index
                except json.JSONDecodeError:
                    # Handle trailing garbage or incomplete JSON
                    if pos == 0:
                        print(f"WS Error: Malformed JSON at start: {message[:50]}...")
                    break
                
                # Filter by symbol to ensure we only act on the target pair
                if data.get("symbol") != CONFIG["signal_symbol"]:
                    continue

                # Check if it is a valid signal
                if isinstance(data, dict) and "entryType" in data:
                    # Log only signals/trades to file
                    with open("ws_debug.log", "a", encoding="utf-8") as f:
                        f.write(f"[{datetime.now()}] {json.dumps(data)}\n")

                    state["latest_signal"] = data
                    
                    # Check for New Signal
                    sig_id = data.get("signalId")
                    e_type = data.get("entryType", "none")
                    classification = data.get("classification", "unknown")
                    
                    print(f"[WS] Signal Received: {e_type} | ID: {sig_id} | Type: {classification}")
                    
                    # Mode Filtering
                    if classification == "scalp" and not CONFIG["scalp_mode"]:
                        print(f"[FILTER] Scalp signal ignored (Scalp Mode OFF)")
                        continue
                    if "swing" in classification and not CONFIG["swing_mode"]:
                        print(f"[FILTER] Swing signal ignored (Swing Mode OFF)")
                        continue
                    
                    if sig_id != state["last_processed_id"] and e_type in ["long", "short"]:
                        state["last_processed_id"] = sig_id
                        execute_trade(data)
                
                # Throttled Management (Max 10 times per second) to prevent freezing
                if time.time() - state["last_manage_time"] > 0.1:
                    manage_positions()
                    state["last_manage_time"] = time.time()
            
        except Exception as e:
            print(f"WS Error: {e}")

    def on_error(ws, error):
        state["status_text"] = f"WS Error: {error}"
        print(error)

    def on_close(ws, close_status_code, close_msg):
        state["status_text"] = "Disconnected"
        print("### closed ###")

    # Construct WebSocket URL
    ws_url = CONFIG['server_url'].replace("https://", "wss://").replace("http://", "ws://") + "/ws"

    while state["running"]:
        state["status_text"] = "Connecting..."
        ws = websocket.WebSocketApp(ws_url,
                                  on_open=on_open,
                                  on_message=on_message,
                                  on_error=on_error,
                                  on_close=on_close)
        
        # Run blocking call (this thread is already separate from GUI)
        ws.run_forever(reconnect=5)
        time.sleep(5) # Wait before reconnecting if run_forever exits

if __name__ == "__main__":
    t = threading.Thread(target=trading_loop)
    t.daemon = True
    t.start()
    
    root = tk.Tk()
    app = DashboardGUI(root)
    root.mainloop()
    
    state["running"] = False
    mt5.shutdown()