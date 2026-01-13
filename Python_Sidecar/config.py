import threading
import json
import os

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
    "fixed_lot_size": 0.01,
    "swing_lot_size": 0.03,
    "max_entries": 10,
    "min_conviction": 20.0,
    "scalp_mode": True,
    "swing_mode": True,
    "force_market": False,
    "manage_manual": False,
    "allow_manual_closure_on_netting": False,
    
    # -- Filters --
    "max_spread_points": 162,
    "min_velocity_pips": 0.5,
    "velocity_lookback_sec": 10,
    
    # -- Stops --
    "trailing_start_pips_scalp": 30.0,
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
    "atr_high_vol_threshold": 2.0,
    "atr_timeframe": "M5",
    "atr_timeframe_scalp": "M5",
    "atr_timeframe_swing": "H1",
    "chart_rr_ratio": 1.5,
    "chart_timeframe": "M1",
    
    # -- Session Filters --
    "session_scalp_syd": True,
    "session_scalp_tok": True,
    "session_scalp_lon": True,
    "session_scalp_ny": True,
    "session_swing_syd": True,
    "session_swing_tok": True,
    "session_swing_lon": True,
    "session_swing_ny": True,
    "session_scalp_overlap_tok_lon": True,
    "session_scalp_overlap_lon_ny": True,
    "session_swing_overlap_tok_lon": True,
    "session_swing_overlap_lon_ny": True,
    
    # -- Auto Close on Session End --
    "close_scalp_syd_end": False,
    "close_scalp_tok_end": False,
    "close_scalp_lon_end": False,
    "close_scalp_ny_end": False,
    "close_swing_syd_end": False,
    "close_swing_tok_end": False,
    "close_swing_lon_end": False,
    "close_swing_ny_end": False,
}

CONFIG = DEFAULT_CONFIG.copy()
CONFIG_FILE = "user_config.json"

def load_config():
    if os.path.exists(CONFIG_FILE):
        try:
            with open(CONFIG_FILE, "r") as f:
                saved = json.load(f)
                for k, v in saved.items():
                    if k in CONFIG:
                        CONFIG[k] = v
        except Exception as e:
            print(f"Failed to load config: {e}")

def save_config():
    try:
        with open(CONFIG_FILE, "w") as f:
            json.dump(CONFIG, f, indent=4)
    except Exception as e:
        print(f"Failed to save config: {e}")

load_config()

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