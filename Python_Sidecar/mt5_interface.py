import MetaTrader5 as mt5 # type: ignore
import time
import json
import csv
import os
import concurrent.futures
from datetime import datetime, timedelta
from config import CONFIG, state, SYMBOL_FILLING_FOK, SYMBOL_FILLING_IOC

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
    # Optimization: Use Numpy field access directly (ticks is a structured array)
    bids = ticks['bid']
    return (bids.max() - bids.min()) / (get_point() * 10.0)

def calculate_local_atr(symbol, timeframe_str=None, period=14):
    """Calculate ATR(14) using configured timeframe candles from MT5."""
    if timeframe_str is None:
        timeframe_str = CONFIG.get("atr_timeframe", "M5")
        
    tf_str = timeframe_str
    tf_map = {
        "M1": mt5.TIMEFRAME_M1, "M5": mt5.TIMEFRAME_M5, "M15": mt5.TIMEFRAME_M15,
        "M30": mt5.TIMEFRAME_M30, "H1": mt5.TIMEFRAME_H1, "H4": mt5.TIMEFRAME_H4, "D1": mt5.TIMEFRAME_D1
    }
    tf = tf_map.get(tf_str, mt5.TIMEFRAME_M5)
    
    # Fetch period+1 candles to calculate True Range for 'period' candles
    rates = mt5.copy_rates_from_pos(symbol, tf, 0, period + 1)
    if rates is None or len(rates) < period + 1:
        return 0.0
    
    tr_sum = 0.0
    for i in range(1, len(rates)):
        high = rates[i]['high']
        low = rates[i]['low']
        close_prev = rates[i-1]['close']
        tr = max(high - low, abs(high - close_prev), abs(low - close_prev))
        tr_sum += tr
        
    return tr_sum / period

def get_chart_data(symbol, timeframe_str="M1", count=100):
    """Fetch recent candles for charting."""
    tf_map = {
        "M1": mt5.TIMEFRAME_M1,
        "M5": mt5.TIMEFRAME_M5,
        "M15": mt5.TIMEFRAME_M15,
        "M30": mt5.TIMEFRAME_M30,
        "H1": mt5.TIMEFRAME_H1,
        "H4": mt5.TIMEFRAME_H4,
        "D1": mt5.TIMEFRAME_D1,
    }
    tf = tf_map.get(timeframe_str, mt5.TIMEFRAME_M1)
    rates = mt5.copy_rates_from_pos(symbol, tf, 0, count)
    
    if rates is None:
        # Try selecting symbol and retry
        if mt5.symbol_select(symbol, True):
             rates = mt5.copy_rates_from_pos(symbol, tf, 0, count)

    if rates is None or len(rates) == 0:
        return []
    
    data = []
    for r in rates:
        data.append({
            "time": int(r['time']),
            "open": float(r['open']),
            "high": float(r['high']),
            "low": float(r['low']),
            "close": float(r['close']),
            "tick_volume": int(r['tick_volume']),
        })
    return data

# =============================================================================
# PERFORMANCE LOGGING
# =============================================================================
TRADES_JSON = "active_trades.json"
CSV_FILENAME = "strategy_performance.csv"

def init_trade_tracking():
    """Load active trades from JSON on startup."""
    if os.path.exists(TRADES_JSON):
        try:
            with open(TRADES_JSON, "r") as f:
                state["active_trades"] = json.load(f)
        except Exception as e:
            print(f"[LOG] Failed to load active trades: {e}")

def save_active_trades():
    """Save active trades to JSON."""
    try:
        with open(TRADES_JSON, "w") as f:
            json.dump(state["active_trades"], f, indent=4)
    except Exception as e:
        print(f"[LOG] Failed to save active trades: {e}")

def log_trade_performance(ticket, signal_data, deals):
    """Calculate P/L from deals and write to CSV."""
    total_profit = 0.0
    total_swap = 0.0
    total_comm = 0.0
    close_time = ""
    close_price = 0.0
    
    # Aggregate deals (Entry and Exits)
    for d in deals:
        total_profit += d.profit
        total_swap += d.swap
        total_comm += d.commission
        if d.entry == mt5.DEAL_ENTRY_OUT or d.entry == mt5.DEAL_ENTRY_INOUT:
            close_time = datetime.fromtimestamp(d.time).strftime("%Y-%m-%d %H:%M:%S")
            close_price = d.price

    net_pl = total_profit + total_swap + total_comm
    
    # Prepare CSV Row
    file_exists = os.path.exists(CSV_FILENAME)
    headers = ["SignalID", "Time", "Ticket", "Symbol", "Type", "Volume", "EntryPrice", "SL", "TP", "Conviction", "CloseTime", "ClosePrice", "GrossProfit", "Swap", "Comm", "NetPL", "Classification"]
    
    raw_type = signal_data.get("entryType", "").lower()
    display_type = "BUY" if "long" in raw_type else "SELL" if "short" in raw_type else raw_type.upper()

    row = [
        signal_data.get("signalId", "N/A"),
        signal_data.get("timestamp", datetime.now().strftime("%Y-%m-%d %H:%M:%S")),
        ticket,
        signal_data.get("symbol", ""),
        display_type,
        signal_data.get("volume", 0.0),
        signal_data.get("price", 0.0),
        signal_data.get("sl", 0.0),
        signal_data.get("tp", 0.0),
        signal_data.get("conviction", 0.0),
        close_time,
        close_price,
        round(total_profit, 2),
        round(total_swap, 2),
        round(total_comm, 2),
        round(net_pl, 2),
        signal_data.get("classification", "scalp")
    ]
    
    with open(CSV_FILENAME, "a", newline="") as f:
        writer = csv.writer(f)
        if not file_exists:
            writer.writerow(headers)
        writer.writerow(row)

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
    
    # --- Pre-Execution Checks ---
    term_info = mt5.terminal_info()
    if not term_info or not term_info.trade_allowed:
        msg = "AutoTrading disabled in Terminal (Click 'Algo Trading' button)"
        print(f"[EXEC] Error: {msg}")
        with state["lock"]:
            state["gui_logs"].append({
                "time": datetime.now().strftime("%H:%M:%S"),
                "ticket": "-",
                "type": "Exec Error",
                "details": msg
            })
        return

    acct_info = mt5.account_info()
    if acct_info:
        if not acct_info.trade_allowed:
            msg = "Trading disabled for this Account (Check Broker/Password)"
            print(f"[EXEC] Error: {msg}")
            with state["lock"]:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": "-",
                    "type": "Exec Error",
                    "details": msg
                })
            return
        if not acct_info.trade_expert:
            msg = f"AutoTrading disabled by Server for Acct {acct_info.login} (Broker restricted)"
            print(f"[EXEC] Error: {msg}")
            with state["lock"]:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": "-",
                    "type": "Exec Error",
                    "details": msg
                })
            return
            
        # --- Netting Account Protection ---
        # If account is Netting (0) or Exchange (1), opposite trades will close existing ones.
        # We must protect Manual trades (magic=0) from being closed by bot trades.
        if acct_info.margin_mode != mt5.ACCOUNT_MARGIN_MODE_RETAIL_HEDGING and not CONFIG.get("allow_manual_closure_on_netting", False):
            positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
            if positions:
                is_long = "long" in entry_type
                for pos in positions:
                    is_opposite = (is_long and pos.type == mt5.ORDER_TYPE_SELL) or \
                                  (not is_long and pos.type == mt5.ORDER_TYPE_BUY)
                    
                    if is_opposite and pos.magic == 0:
                        msg = f"Aborted {entry_type} to protect Manual Trade #{pos.ticket} (Netting Account)"
                        print(f"[EXEC] {msg}")
                        with state["lock"]:
                            state["gui_logs"].append({
                                "time": datetime.now().strftime("%H:%M:%S"),
                                "ticket": "-",
                                "type": "Exec Abort",
                                "details": "Protecting Manual Trade (Netting)"
                            })
                        return
    
    # Simple Filters
    ask, bid = get_ask_bid()
    if ask == 0.0 or bid == 0.0:
        msg = f"Invalid price (0.0) for {CONFIG['trade_symbol']}"
        print(f"[EXEC] Error: {msg}")
        with state["lock"]:
            state["gui_logs"].append({
                "time": datetime.now().strftime("%H:%M:%S"),
                "ticket": "-",
                "type": "Exec Error",
                "details": msg
            })
        return

    spread = (ask - bid) / get_point()
    if spread > CONFIG["max_spread_points"]:
        msg = f"Spread too high: {spread:.1f} > {CONFIG['max_spread_points']}"
        with state["lock"]:
            state["gui_logs"].append({
                "time": datetime.now().strftime("%H:%M:%S"),
                "ticket": "-",
                "type": "Filter",
                "details": msg
            })
        return

    # Check Symbol Info for Filling Mode
    symbol_info = mt5.symbol_info(CONFIG["trade_symbol"])
    if symbol_info is None:
        msg = f"Symbol {CONFIG['trade_symbol']} not found"
        print(f"[EXEC] Error: {msg}")
        with state["lock"]:
            state["gui_logs"].append({
                "time": datetime.now().strftime("%H:%M:%S"),
                "ticket": "-",
                "type": "Exec Error",
                "details": msg
            })
        return

    # Prepare Order
    raw_sl = float(signal.get("slPrice", 0.0))
    raw_tp1 = float(signal.get("tp1Price", 0.0))
    raw_tp2 = float(signal.get("tp2Price", 0.0))
    
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
    
    # Calculate Entries based on Conviction
    conviction = float(signal.get("convictionScore", signal.get("conviction", 0.0)))
    
    # Filter: Minimum Conviction
    min_conv = CONFIG.get("min_conviction", 20.0)
    if conviction < min_conv:
        msg = f"Conviction {conviction:.1f}% < {min_conv}%"
        with state["lock"]:
            state["gui_logs"].append({
                "time": datetime.now().strftime("%H:%M:%S"),
                "ticket": "-",
                "type": "Filter",
                "details": msg
            })
        return

    target_entries = int((conviction / 100.0) * CONFIG["max_entries"])
    entries_to_execute = max(1, target_entries) # Ensure at least 1 trade

    with state["lock"]:
        state["gui_logs"].append({
            "time": datetime.now().strftime("%H:%M:%S"),
            "ticket": "-",
            "type": "Signal",
            "details": f"Conviction: {conviction:.1f}% | Scaling: {entries_to_execute}"
        })
    
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
        point = symbol_info.point
        
        def normalize(val):
            return round(round(val / tick_size) * tick_size, digits)
            
        price = normalize(price)
        
        if raw_sl > 0: raw_sl = normalize(raw_sl)
        if raw_tp1 > 0: raw_tp1 = normalize(raw_tp1)
        if raw_tp2 > 0: raw_tp2 = normalize(raw_tp2)
        
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
    
    # Calculate TP Split (Majority to TP1)
    count_tp1 = (entries_to_execute + 1) // 2
        
    # Execute Multiple Entries
    for i in range(entries_to_execute):
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

        # Assign TP1 or TP2
        current_tp = raw_tp1
        tp_label = "TP1"
        if raw_tp2 > 0 and i >= count_tp1:
            current_tp = raw_tp2
            tp_label = "TP2"

        request = {
            "action": action,
            "symbol": CONFIG["trade_symbol"],
            "volume": volume,
            "type": mt5_type,
            "price": price,
            "sl": raw_sl,
            "tp": current_tp,
            "deviation": 20,
            "magic": magic,
            "comment": f"{comment} C:{int(conviction)} ({i+1}/{entries_to_execute})",
            "type_time": type_time,
            "expiration": order_expiration,
            "type_filling": filling_type,
        }
        
        res = mt5.order_send(request)
        if res and res.retcode == mt5.TRADE_RETCODE_DONE:
            msg = f"[EXEC] {comment} Open ({i+1}/{entries_to_execute}): {entry_type} | Vol: {volume} | Price: {price} | {tp_label}"
            with state["lock"]:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": str(res.order),
                    "type": "Trade Open",
                    "details": f"{entry_type.upper()} {volume} @ {price} ({tp_label})"
                })
            
            # Register for tracking
            with state["lock"]:
                state["active_trades"][str(res.order)] = {
                    "signalId": signal.get("signalId", "Manual"),
                    "timestamp": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
                    "symbol": CONFIG["trade_symbol"],
                    "entryType": entry_type,
                    "volume": volume,
                    "price": price,
                    "sl": raw_sl,
                    "tp": current_tp,
                    "conviction": conviction,
                    "classification": classification
                }
            save_active_trades()
            
        else:
            err_msg = res.comment if res else 'Unknown'
            with state["lock"]:
                state["gui_logs"].append({
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": "-",
                    "type": "Exec Error",
                    "details": f"({i+1}/{entries_to_execute}): {err_msg}"
                })
        
        time.sleep(0.05) # Slight delay to ensure order processing

# =============================================================================
# TRADE MANAGEMENT
# =============================================================================
def monitor_closed_trades(open_positions):
    """Check if any tracked trades have closed."""
    if not state["active_trades"]: return

    open_tickets = {str(p.ticket) for p in open_positions}
    tracked_tickets = list(state["active_trades"].keys())
    
    for ticket in tracked_tickets:
        if ticket not in open_tickets:
            # Trade is closed, fetch history
            deals = mt5.history_deals_get(position=int(ticket))
            
            if deals:
                # Log to CSV
                signal_data = state["active_trades"][ticket]
                log_trade_performance(ticket, signal_data, deals)
                
                # Remove from tracking
                with state["lock"]:
                    del state["active_trades"][ticket]
                save_active_trades()

def close_all_positions(mode="scalp"):
    """
    Closes all open positions for the specified mode (scalp or swing).
    """
    positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
    if not positions: return
    
    magic_target = CONFIG["magic_number"] if mode == "scalp" else CONFIG["magic_number"] + 1
    
    for pos in positions:
        if pos.magic == magic_target:
            tick = mt5.symbol_info_tick(pos.symbol)
            if not tick: continue
            
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
                "comment": f"Session End ({mode})"
            }
            
            res = mt5.order_send(req)
            if res and res.retcode == mt5.TRADE_RETCODE_DONE:
                with state["lock"]:
                    state["gui_logs"].append({
                        "time": datetime.now().strftime("%H:%M:%S"),
                        "ticket": str(pos.ticket),
                        "type": "Auto Close",
                        "details": f"Session End ({mode})"
                    })

def manage_positions():
    # 0. Update ATRs
    # Dashboard Gauge ATR (Global setting)
    gauge_atr = calculate_local_atr(CONFIG["trade_symbol"], CONFIG.get("atr_timeframe", "M5"))
    if gauge_atr > 0:
        state["server_atr"] = gauge_atr
        
    # Strategy-specific ATRs for Trailing
    atr_scalp = calculate_local_atr(CONFIG["trade_symbol"], CONFIG.get("atr_timeframe_scalp", "M5"))
    atr_swing = calculate_local_atr(CONFIG["trade_symbol"], CONFIG.get("atr_timeframe_swing", "H1"))

    # Fetch all positions for the symbol (including Manual with magic=0)
    all_positions = mt5.positions_get(symbol=CONFIG["trade_symbol"])
    
    if all_positions is None:
        return

    # Check for closed trades
    monitor_closed_trades(all_positions)
    
    if not all_positions: return
    
    symbol_info = mt5.symbol_info(CONFIG["trade_symbol"])
    if not symbol_info: return
    
    point = symbol_info.point
    tick_size = symbol_info.trade_tick_size
    digits = symbol_info.digits
    
    def normalize(val):
        return round(val, digits)
    
    # Sync time with server to ensure timeouts work correctly regardless of local clock
    tick_info = mt5.symbol_info_tick(CONFIG["trade_symbol"])
    server_time = tick_info.time if tick_info else time.time()
    
    update_requests = []

    for pos in all_positions:
        # Filter: Only manage Scalp, Swing, or Manual (if enabled)
        is_scalp = pos.magic == CONFIG["magic_number"]
        is_swing = pos.magic == CONFIG["magic_number"] + 1
        is_manual = pos.magic == 0
        
        if not (is_scalp or is_swing or (is_manual and CONFIG["manage_manual"])):
            continue

        # 1. Calculate Profit in Pips
        current_price = symbol_info.bid if pos.type == mt5.ORDER_TYPE_BUY else symbol_info.ask
        if current_price == 0.0: continue # Skip if price is invalid
        
        if pos.type == mt5.ORDER_TYPE_BUY:
            diff = current_price - pos.price_open
        else:
            diff = pos.price_open - current_price
        
        # Assuming 1 pip = 10 points for XAUUSD (2 digits)
        profit_pips = diff / (point * 10)
        
        is_buy = pos.type == mt5.ORDER_TYPE_BUY

        # 2. Stagnation Logic (Partial Close) - Skip for Manual trades
        is_stagnating = False
        if CONFIG["use_stagnation"] and not is_manual and pos.ticket not in state["stagnated_orders"]:
            elapsed = server_time - pos.time
            limit = CONFIG["stagnation_sec"]
            
            # Dynamic Stagnation: Increase time limit for Swing trades
            if CONFIG["use_dynamic_stag"] and pos.magic == CONFIG["magic_number"] + 1:
                limit *= CONFIG["stag_time_mult"]
            
            if elapsed > limit and pos.profit <= 0:
                vol_to_close = pos.volume
                
                if vol_to_close >= symbol_info.volume_min:
                    type_close = mt5.ORDER_TYPE_SELL if pos.type == mt5.ORDER_TYPE_BUY else mt5.ORDER_TYPE_BUY
                    req = {
                        "action": mt5.TRADE_ACTION_DEAL,
                        "symbol": pos.symbol,
                        "position": pos.ticket,
                        "volume": vol_to_close,
                        "type": type_close,
                        "price": symbol_info.bid if type_close == mt5.ORDER_TYPE_SELL else symbol_info.ask,
                        "magic": pos.magic,
                        "comment": "Stagnation"
                    }
                    log_entry = {
                        "time": datetime.now().strftime("%H:%M:%S"),
                        "ticket": pos.ticket,
                        "type": "Stagnation",
                        "details": f"Closed {vol_to_close} lots"
                    }
                    
                    # Define callback to update state only on success
                    def on_stag_success(t=pos.ticket):
                        state["stagnated_orders"].add(t)
                        
                    update_requests.append((req, log_entry, on_stag_success))
                    is_stagnating = True
        
        if is_stagnating:
            continue # Skip SL updates if we are partially closing

        # 3. Trailing Stop Logic
        current_sl = pos.sl
        best_sl = current_sl
        sl_reason = "Manual"
        using_atr = False
        
        # Params
        # Use Scalp settings for Scalp trades, Swing settings for Swing AND Manual trades
        if is_scalp:
            use_trailing = CONFIG["use_trailing_scalp"]
            tr_start = CONFIG["trailing_start_pips_scalp"]
            tr_dist_cfg = CONFIG["trailing_dist_pips_scalp"]
            tr_step = CONFIG["trailing_step_pips_scalp"]
        else:
            use_trailing = CONFIG["use_trailing_swing"]
            tr_start = CONFIG["trailing_start_pips_swing"]
            tr_dist_cfg = CONFIG["trailing_dist_pips_swing"]
            tr_step = CONFIG["trailing_step_pips_swing"]
        
        # Debug info container
        debug_msg = None

        # Check Trailing
        if use_trailing and profit_pips >= tr_start:
            dist_pips = tr_dist_cfg
            
            # Dynamic ATR Logic
            if CONFIG.get("use_atr_trailing", False):
                # Select appropriate ATR based on trade type
                active_atr = atr_scalp if is_scalp else atr_swing
                
                if active_atr > 0:
                # Convert ATR (Price) to Pips (1 pip = 10 points)
                    atr_pips = active_atr / (point * 10)
                    dist_pips = atr_pips
                    using_atr = True

            dist_pts = dist_pips * 10 * point
            candidate_sl = (current_price - dist_pts) if is_buy else (current_price + dist_pts)
            
            if is_buy:
                if candidate_sl > best_sl: 
                    best_sl = candidate_sl
                    sl_reason = "Trailing"
                    if using_atr: sl_reason += f" (ATR: {dist_pips:.1f})"
            else:
                if best_sl == 0 or candidate_sl < best_sl: 
                    best_sl = candidate_sl
                    sl_reason = "Trailing"
                    if using_atr: sl_reason += f" (ATR: {dist_pips:.1f})"
            
            # Diagnostic Log (Throttle: Only log if profit is high and no update happening, or periodically)
            # For diagnosis, we log if we are DEEP in profit but SL isn't moving
            if profit_pips > (tr_start + 10) and best_sl == current_sl:
                debug_msg = f"Trail Stuck? P:{profit_pips:.1f} Dist:{dist_pips:.1f} NewSL:{candidate_sl:.2f} <= CurrSL:{current_sl:.2f}"
        
        # Normalize best_sl to ensure valid comparison
        best_sl = normalize(best_sl)
        
        # Apply Update if needed
        if best_sl != current_sl:
            step_pts = tr_step * 10 * point
            should_update = False
            
            if is_buy:
                if best_sl > current_sl + step_pts: should_update = True
            else:
                if current_sl == 0 or best_sl < current_sl - step_pts: should_update = True
            
            if should_update:
                req = {"action": mt5.TRADE_ACTION_SLTP, "position": pos.ticket, "sl": best_sl, "tp": pos.tp}
                log_entry = {
                    "time": datetime.now().strftime("%H:%M:%S"),
                    "ticket": pos.ticket,
                    "type": "SL Update",
                    "details": f"{current_sl:.2f} -> {best_sl:.2f} ({sl_reason})"
                }
                update_requests.append((req, log_entry, None))
        elif debug_msg:
            # Log the diagnostic message if we didn't update but maybe should have
            # Use a simple throttle to avoid spamming every 100ms
            if int(time.time()) % 5 == 0: # Log max once per 5 seconds per trade
                 with state["lock"]:
                    # Check if we haven't logged this recently to avoid flood
                    state["gui_logs"].append({"time": datetime.now().strftime("%H:%M:%S"), "ticket": pos.ticket, "type": "Trail Diag", "details": debug_msg})


    # 4. Execute All Updates Concurrently
    if update_requests:
        def execute_and_log(item):
            req, log_entry, on_success = item
            res = mt5.order_send(req)
            if res:
                if res.retcode == mt5.TRADE_RETCODE_DONE:
                    if log_entry:
                        with state["lock"]:
                            state["gui_logs"].append(log_entry)
                    # Execute callback if provided
                    if on_success:
                        on_success()
                elif res.retcode == 10025: # TRADE_RETCODE_NO_CHANGES
                    # Suppress "No changes" error as it's harmless
                    pass
            else:
                err_msg = res.comment if res else "Unknown Error"
                with state["lock"]:
                    state["gui_logs"].append({
                        "time": datetime.now().strftime("%H:%M:%S"),
                        "ticket": req.get("position", "?"),
                        "type": "Update Fail",
                        "details": f"{err_msg}"
                    })

        with concurrent.futures.ThreadPoolExecutor(max_workers=10) as executor:
            futures = [executor.submit(execute_and_log, item) for item in update_requests]
            for f in concurrent.futures.as_completed(futures):
                pass # Wait for all to complete