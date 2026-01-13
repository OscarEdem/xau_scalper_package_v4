import sys
import time
import websocket # type: ignore
import json
from datetime import datetime
import ctypes

from PySide6.QtCore import QThread, QObject, Signal, Slot # type: ignore
from PySide6.QtWidgets import QApplication # type: ignore

try:
    import MetaTrader5 as mt5 # type: ignore
except ImportError as e:
    error_msg = f"CRITICAL ERROR: Failed to import MetaTrader5 module.\nDetails: {e}"
    if "numpy" in str(e).lower():
        error_msg += "\n\nPOSSIBLE CAUSE: Incompatible Numpy version (2.0+).\nPlease downgrade to Numpy 1.x (pip install \"numpy<2\")."
    ctypes.windll.user32.MessageBoxW(0, error_msg, "Startup Error", 0x10)
    sys.exit(1)

from config import CONFIG, state
from mt5_interface import execute_trade, manage_positions, init_trade_tracking, close_all_positions
from gui import DashboardGUI

# =============================================================================
# WORKER CLASS
# =============================================================================

def check_session_closures():
    """Checks if current time matches session end and triggers auto-close."""
    now = datetime.utcnow()
    h = now.hour
    m = now.minute
    
    # Session End Hours (UTC)
    ends = {
        "syd": 6,
        "tok": 9,
        "lon": 17,
        "ny": 22
    }
    
    # Run logic only in the first minute of the hour
    if m == 0:
        for sess, end_h in ends.items():
            if h == end_h:
                # Check Scalp
                key_scalp = f"close_scalp_{sess}_end"
                last_run_key = f"last_close_{sess}_scalp"
                if CONFIG.get(key_scalp, False) and time.time() - state.get(last_run_key, 0) > 120:
                    print(f"[AUTO-CLOSE] Closing Scalp trades for {sess.upper()} session end.")
                    close_all_positions("scalp")
                    state[last_run_key] = time.time()
                
                # Check Swing
                key_swing = f"close_swing_{sess}_end"
                last_run_key = f"last_close_{sess}_swing"
                if CONFIG.get(key_swing, False) and time.time() - state.get(last_run_key, 0) > 120:
                    print(f"[AUTO-CLOSE] Closing Swing trades for {sess.upper()} session end.")
                    close_all_positions("swing")
                    state[last_run_key] = time.time()

class TradingWorker(QObject):
    finished = Signal()

    def __init__(self):
        super().__init__()
        self.ws = None

    @Slot()
    def run(self):
        # Retry initialization loop
        while state["running"]:
            if mt5.initialize():
                break
            state["status_text"] = "MT5 Init Failed! Retrying..."
            time.sleep(1)
        
        if not state["running"]:
            self.finished.emit()
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
                        
                        # Check for Signal
                        sig_id = data.get("signalId")
                        e_type = data.get("entryType", "none")
                        classification = data.get("classification", "unknown")

                        display_type = "BUY" if "long" in e_type else "SELL" if "short" in e_type else e_type.upper()

                        with state["lock"]:
                            state["gui_logs"].append({
                                "time": datetime.now().strftime("%H:%M:%S"),
                                "ticket": "-",
                                "type": "Signal Recv",
                                "details":  f"{display_type} | ID: {sig_id} | Type: {classification}"
                            })

                        # Mode Filtering

                        if classification == "scalp" and not CONFIG["scalp_mode"]:
                            with state["lock"]:
                                state["gui_logs"].append({ "time": datetime.now().strftime("%H:%M:%S"), "ticket": "-", "type": "Filter", "details": "Scalp signal ignored (Scalp Mode OFF)"})
                            print(f"[FILTER] Scalp signal ignored (Scalp Mode OFF)")
                            continue
                        if "swing" in classification and not CONFIG["swing_mode"]:
                            print(f"[FILTER] Swing signal ignored (Swing Mode OFF)")
                            continue
                        
                        # Session Filtering
                        h = datetime.utcnow().hour
                        
                        # Define Overlaps
                        is_overlap_lon_ny = 13 <= h < 17
                        is_overlap_tok_lon = 8 <= h < 9
                        
                        # Define Sessions
                        is_syd = 21 <= h or h < 6
                        is_tok = 0 <= h < 9
                        is_lon = 8 <= h < 17
                        is_ny = 13 <= h < 22
                        
                        session_allowed = False
                        
                        def check_session(mode):
                            # Check Overlaps First (Priority)
                            if is_overlap_lon_ny:
                                return CONFIG.get(f"session_{mode}_overlap_lon_ny", True)
                            if is_overlap_tok_lon:
                                return CONFIG.get(f"session_{mode}_overlap_tok_lon", True)
                            
                            # Check Individual Sessions
                            if is_syd and CONFIG.get(f"session_{mode}_syd", True): return True
                            if is_tok and CONFIG.get(f"session_{mode}_tok", True): return True
                            if is_lon and CONFIG.get(f"session_{mode}_lon", True): return True
                            if is_ny and CONFIG.get(f"session_{mode}_ny", True): return True
                            return False

                        if classification == "scalp":
                            session_allowed = check_session("scalp")
                        elif "swing" in classification:
                            session_allowed = check_session("swing")
                        else:
                            session_allowed = True # Allow unknown types or manual
                            
                        if not session_allowed:
                            print(f"[FILTER] {classification} signal ignored (Session Disabled)")
                            continue
                        
                        if sig_id not in state["processed_ids"] and e_type in ["long", "short"]:
                            state["processed_ids"].add(sig_id)
                            state["last_processed_id"] = sig_id
                            execute_trade(data)
                
                    # Throttled Management (Max 10 times per second) to prevent freezing
                    if time.time() - state["last_manage_time"] > 0.1:
                        check_session_closures()
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
            self.ws = websocket.WebSocketApp(ws_url,
                                    on_open=on_open,
                                    on_message=on_message,
                                    on_error=on_error,
                                    on_close=on_close)
            
            # Run blocking call (this thread is already separate from GUI)
            try:
                self.ws.run_forever(reconnect=5)
            except Exception as e:
                print(f"[NET] Connection Error: {e}")
                state["status_text"] = "Connection Failed"
            
            if state["running"]:
                # Wait before reconnecting, but allow quick exit
                for _ in range(50):
                    if not state["running"]: break
                    time.sleep(0.1)
        
        self.finished.emit()

    @Slot()
    def stop(self):
        state["running"] = False
        if self.ws:
            self.ws.close()

if __name__ == "__main__":
    app = QApplication(sys.argv)
    

    # Setup QThread for Trading Logic
    thread = QThread()
    worker = TradingWorker()
    worker.moveToThread(thread)
    
    thread.started.connect(worker.run)
    worker.finished.connect(thread.quit)
    worker.finished.connect(worker.deleteLater)
    thread.finished.connect(thread.deleteLater)
    
    # Ensure clean exit
    app.aboutToQuit.connect(worker.stop)
    
    thread.start()
    
    window = DashboardGUI()
    window.show()
    
    exit_code = app.exec()
    
    # Wait for thread to finish
    if thread.isRunning():
        thread.quit()
        thread.wait(2000)
    
    mt5.shutdown()
    sys.exit(exit_code)