import pandas as pd
import numpy as np


def compute_rsi(close: pd.Series, period: int = 14) -> pd.Series:
    """Wilder's RSI, normalized to [-1, 1] for use as a model feature."""
    delta = close.diff()
    gain = delta.clip(lower=0).ewm(alpha=1 / period, min_periods=period, adjust=False).mean()
    loss = (-delta.clip(upper=0)).ewm(alpha=1 / period, min_periods=period, adjust=False).mean()
    rs = gain / loss.replace(0, np.nan)
    rsi_raw = 100 - (100 / (1 + rs))
    # Normalize: 0..100 → -1..1 so the MLP doesn't need to learn the offset
    return (rsi_raw / 50.0) - 1.0


def compute_ema(series: pd.Series, span: int) -> pd.Series:
    return series.ewm(span=span, adjust=False).mean()


def compute_atr(high: pd.Series, low: pd.Series, close: pd.Series, period: int = 14) -> pd.Series:
    prev_close = close.shift(1)
    tr = pd.concat([
        high - low,
        (high - prev_close).abs(),
        (low - prev_close).abs(),
    ], axis=1).max(axis=1)
    return tr.ewm(alpha=1 / period, min_periods=period, adjust=False).mean()


def compute_features(df: pd.DataFrame) -> tuple:
    """
    Computes the 5 deterministic technical features used by both the
    FeatureMLP (GBM replacement) and RegimeMLP (Heston replacement).

    Features are exactly mirrored by the Rust `generic_onnx/predict.rs`
    runtime so training and inference are bit-for-bit identical.

    Args:
        df: DataFrame with columns ['open', 'high', 'low', 'close', 'volume'].

    Returns:
        (X, y, scaler_params) where:
        - X: float32 array of shape [N, 5]
        - y: float32 array of shape [N]  — next-3-bar log return (target)
        - scaler_params: dict with 'mean' and 'std' arrays (length 5) for z-scoring
    """
    close = df['close']
    high  = df['high']
    low   = df['low']

    atr14 = compute_atr(high, low, close, 14)
    ema20 = compute_ema(close, 20)

    features = pd.DataFrame({
        # 1. Short momentum
        'roc_3':      close.pct_change(3),
        # 2. Medium momentum
        'roc_10':     close.pct_change(10),
        # 3. Normalised volatility
        'atr_ratio':  atr14 / close.replace(0, np.nan),
        # 4. Momentum oscillator (normalised to -1..1)
        'rsi_14':     compute_rsi(close, 14),
        # 5. Mean-reversion distance (in ATR units)
        'dist_ema20': (close - ema20) / atr14.replace(0, np.nan),
    })

    # Next-3-bar log return as regression target
    log_return_3 = np.log(close.shift(-3) / close)

    # Align and drop rows with NaN
    combined = pd.concat([features, log_return_3.rename('target')], axis=1).dropna()

    X = combined[features.columns].values.astype(np.float32)
    y = combined['target'].values.astype(np.float32)

    # Per-feature z-score scaler (computed on full dataset)
    scaler_params = {
        'mean':       X.mean(axis=0).tolist(),
        'std':        X.std(axis=0).tolist(),
        'n_features': X.shape[1],
    }

    # Normalise X
    std_safe = np.where(np.array(scaler_params['std']) == 0, 1.0, scaler_params['std'])
    X_scaled = (X - np.array(scaler_params['mean'])) / std_safe

    return X_scaled.astype(np.float32), y, scaler_params
