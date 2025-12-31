import sys
import time
import threading
import websocket # type: ignore
import json
from datetime import datetime
import ctypes

try:
    import MetaTrader5 as mt5 # type: ignore
except ImportError as e:
    error_msg = f"CRITICAL ERROR: Failed to import MetaTrader5 module.\nDetails: {e}"
    if "numpy" in str(e).lower():
        error_msg += "\n\nPOSSIBLE CAUSE: Incompatible Numpy version (2.0+).\nPlease downgrade to Numpy 1.x (pip install \"numpy<2\")."
    ctypes.windll.user32.MessageBoxW(0, error_msg, "Startup Error", 0x10)
    sys.exit(1)

from config import CONFIG, state
from mt5_interface import execute_trade, manage_positions, init_trade_tracking
from gui import DashboardGUI

# =============================================================================
# MAIN LOOP
# =============================================================================
def trading_loop():
    if not mt5.initialize():
        state["status_text"] = "MT5 Init Failed!"
        return
    
    # Ensure symbol is selected
    mt5.symbol_select(CONFIG["trade_symbol"], True)
    
    # Initialize Trade Tracking (Load active trades)
    init_trade_tracking()

    if "processed_ids" not in state:
        state["processed_ids"] = set()

    def on_open(ws):
        state["status_text"] = "Connected"
        state["connection_time"] = datetime.now()

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
                
                # Check if it is a valid signal
                if isinstance(data, dict) and "entryType" in data:
                    # Map Server Symbol to Local Symbol
                    srv_sym = data.get("symbol")
                    local_sym = CONFIG.get("symbol_map", {}).get(srv_sym, srv_sym)
                    
                    if local_sym != CONFIG["trade_symbol"]:
                        print(f"[FILTER] Ignored {srv_sym} (Mapped: {local_sym}) != {CONFIG['trade_symbol']}")
                        continue
                    
                    # Update signal symbol to match server (Visual feedback)
                    CONFIG["signal_symbol"] = srv_sym

                    state["latest_signal"] = data
                    state["last_signal_ts"] = time.time()
                    
                    # Check for New Signal
                    sig_id = data.get("signalId")
                    e_type = data.get("entryType", "none")
                    classification = data.get("classification", "unknown")

                    with state["lock"]:
                        state["gui_logs"].append({
                            "time": datetime.now().strftime("%H:%M:%S"),
                            "ticket": "-",
                            "type": "Signal Recv",
                            "details":  f"{e_type} | ID: {sig_id} | Type: {classification}"
                        })

                    # Mode Filtering

                    if classification == "scalp" and not CONFIG["scalp_mode"]:
                        with state["lock"]:
                            state["gui_logs"].append({ "time": datetime.now().strftime("%H:%M:%S"), "ticket": "-", "type": "Filter", "details": "Scalp signal ignored (Scalp Mode OFF)"})
                        continue

                    if classification == "scalp" and not CONFIG["scalp_mode"]:
                        print(f"[FILTER] Scalp signal ignored (Scalp Mode OFF)")
                        continue
                    if "swing" in classification and not CONFIG["swing_mode"]:
                        print(f"[FILTER] Swing signal ignored (Swing Mode OFF)")
                        continue
                    
                    if sig_id not in state["processed_ids"] and e_type in ["long", "short"]:
                        state["processed_ids"].add(sig_id)
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
    
    from PySide6.QtWidgets import QApplication # type: ignore
    app = QApplication(sys.argv)
    window = DashboardGUI()
    window.show()
    exit_code = app.exec()
    
    state["running"] = False
    mt5.shutdown()
    sys.exit(exit_code)