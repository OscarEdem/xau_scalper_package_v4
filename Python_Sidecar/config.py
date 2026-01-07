import threading

# --- DEFAULT CONFIGURATION ---
DEFAULT_CONFIG = {
    "server_url": "https://gold-ml-base-server.onrender.com",
    "signal_symbol": "XAUUSD",
    "trade_symbol": "XAUUSD",
    "symbol_map": {
        "Gold": "XAUUSD",
        "XAUUSDm": "XAUUSD"
    },
    "magic_number": 1337,
    "update_interval": 1.0,
    
    # -- Mutable Settings (Controlled by GUI) --
    "fixed_lot_size": 0.05,
    "swing_lot_size": 0.03,
    "max_entries": 10,
    "min_conviction": 20.0,
    "scalp_mode": True,
    "swing_mode": True,
    "force_market": False,
    "manage_manual": False,
    
    # -- Filters --
    "max_spread_points": 162,
    "min_velocity_pips": 0.5,
    "velocity_lookback_sec": 10,
    
    # -- Stops --
    "trailing_start_pips_scalp": 50.0,
    "trailing_start_pips_swing": 450.0,
    "trailing_dist_pips_scalp": 80.0,
    "trailing_dist_pips_swing": 200.0,
    "trailing_step_pips_scalp": 2.0,
    "trailing_step_pips_swing": 2.0,
    "stagnation_sec": 300, 
    "use_stagnation": True,
    "use_dynamic_stag": True,
    "stag_time_mult": 6.0,
    "use_trailing_scalp": True,
    "use_trailing_swing": True,
    "use_atr_trailing": True,
    "atr_dist_mult_scalp": 1.5,
    "atr_dist_mult_swing": 2.5,
    "atr_high_vol_threshold": 1.0,
    "max_scalp_sl_pips": 100.0,
}

CONFIG = DEFAULT_CONFIG.copy()

# --- GLOBAL STATE ---
state = {
    "latest_signal": None,
    "last_processed_id": None,
    "server_atr": 0.0,
    "running": True,
    "status_text": "Initializing...",
    "lock": threading.Lock(),
    "last_manage_time": 0,
    "last_signal_ts": 0,
    "stagnated_orders": set(),
    "connection_time": None,
    "gui_logs": [],
    "active_trades": {} # Maps ticket -> signal_data
}

# --- MT5 CONSTANTS (Fallback) ---
SYMBOL_FILLING_FOK = 1
SYMBOL_FILLING_IOC = 2