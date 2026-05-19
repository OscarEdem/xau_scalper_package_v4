import MetaTrader5 as mt5
import pandas as pd
from datetime import datetime
import pytz

# --- Configuration ---
SYMBOL = "XAUUSD"  # Check your Market Watch (e.g., "Gold", "XAUUSD.m", etc.)
START_DATE = datetime(2023, 1, 1, tzinfo=pytz.utc) # Jan 1, 2023
END_DATE = datetime.now(pytz.utc)                  # Today

TIMEFRAMES = {
    "m5": mt5.TIMEFRAME_M5,
    "m30": mt5.TIMEFRAME_M30,
    "h1": mt5.TIMEFRAME_H1,
    "h4": mt5.TIMEFRAME_H4,
    "d1": mt5.TIMEFRAME_D1
}

def fetch_and_save():
    # 1. Connect to MT5
    if not mt5.initialize():
        print("initialize() failed, error code =", mt5.last_error())
        quit()

    print(f"Connected. Fetching {SYMBOL} data from {START_DATE.date()} to {END_DATE.date()}...")

    # 2. Loop through timeframes
    for name, tf_constant in TIMEFRAMES.items():
        # --- KEY CHANGE: Use copy_rates_range ---
        rates = mt5.copy_rates_range(SYMBOL, tf_constant, START_DATE, END_DATE)
        
        if rates is None or len(rates) == 0:
            print(f"Failed to get data for {name} (Check symbol name or history depth)")
            continue

        # 3. Convert to DataFrame
        df = pd.DataFrame(rates)
        
        # 4. Format columns
        df['timestamp'] = pd.to_datetime(df['time'], unit='s')
        
        # Rename tick_volume to volume
        if 'tick_volume' in df.columns:
            df.rename(columns={'tick_volume': 'volume'}, inplace=True)
        
        # Select and Reorder
        # Note: If you want real volume (futures), change 'volume' to 'real_volume' below
        df = df[['timestamp', 'open', 'high', 'low', 'close', 'volume']]
        
        # 5. Save
        filename = f"{name}_data.csv"
        df.to_csv(filename, index=False)
        print(f"[{name}] Saved {len(df)} rows to {filename}")

    # 6. Shutdown
    mt5.shutdown()

if __name__ == "__main__":
    fetch_and_save()