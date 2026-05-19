import pandas as pd
import os

# Define the timeframes you are using, this should match your CSV filenames
TIMEFRAMES = ["m5", "m30", "h1", "h4", "d1"]
DATA_DIR = "data" # Assumes your CSVs are in a 'data' subdirectory

def load_all_timeframes():
    """
    Loads OHLCV data for all specified timeframes from CSV files.
    """
    all_data = {}
    ohlcv_cols = ['open', 'high', 'low', 'close', 'volume']

    for tf in TIMEFRAMES:
        # Match the filenames created by fetch_data_range.py (e.g., "m5_data.csv")
        file_path = os.path.join(DATA_DIR, f"{tf}_data.csv")
        if os.path.exists(file_path):
            # Load the data, assuming it might be tab-separated and have unusual headers
            try:
                # First, try to read as a standard comma-separated CSV
                df = pd.read_csv(file_path, usecols=ohlcv_cols)
            except (ValueError, KeyError):
                # If that fails, try reading as a tab-separated file from MT5 export
                print(f"Reading {file_path} as a tab-separated file.")
                df = pd.read_csv(file_path, sep='\t')
                # Clean up the column names (e.g., '<OPEN>' -> 'open')
                df.columns = [col.strip('<>').lower() for col in df.columns]
                df.rename(columns={'tickvol': 'volume'}, inplace=True)

            all_data[tf] = {"ohlcv": df[ohlcv_cols].to_numpy()}
            print(f"Loaded {len(df)} rows for {tf} from {file_path}")
        else:
            print(f"Warning: Data file not found for timeframe {tf} at {file_path}")

    return all_data