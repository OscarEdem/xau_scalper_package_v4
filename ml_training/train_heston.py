"""
train_heston.py  —  Regime Classifier MLP (replaces Heston Stochastic Vol)
==========================================================================
Trains a small MLP that classifies the current market regime into one of
three states: Trending-Up (+1), Trending-Down (-1), Sideways (0). The
model outputs a continuous regime score in (-1, 1) which the ensemble uses
as a directional bias multiplier.

Why this replaces Heston
------------------------
The Heston model (as implemented) drew fresh random noise (z ~ N(0,1))
on every inference call — non-deterministic and with zero drift assumption,
meaning zero directional alpha. This regime classifier is fully deterministic
and actively filters out low-conviction choppy market states.

Regime Label Construction (Self-Supervised)
------------------------------------------
Labels are derived entirely from price action — no manual tagging needed:
  - If next-10-bar return > +0.5 × normalised ATR → TRENDING UP   ( +1 )
  - If next-10-bar return < -0.5 × normalised ATR → TRENDING DOWN  ( -1 )
  - Otherwise                                     → SIDEWAYS       (  0 )
"""

import json
import os

import numpy as np
import pandas as pd
import torch
import torch.nn as nn
from torch.utils.data import DataLoader, TensorDataset

from dataset import load_all_timeframes
from preprocessing import compute_features, compute_atr

# ---------- Hyper-parameters ----------
HIDDEN_SIZES  = [24, 12]
BATCH_SIZE    = 32
EPOCHS        = 50
LR            = 5e-4
DROPOUT       = 0.25
WEIGHT_DECAY  = 1e-4
VAL_SPLIT     = 0.2
TREND_ATR_THR = 0.5    # ATR multiples needed to call a move "trending"
HORIZON       = 10     # Bars ahead for regime assignment
SEED          = 42
TIMEFRAMES    = ["m5", "m30", "h1", "h4", "d1"]

torch.manual_seed(SEED)
os.makedirs("models", exist_ok=True)


# ---------- Architecture ----------
class RegimeMLP(nn.Module):
    """Maps 5 normalised technical features → regime score ∈ (-1, +1)."""
    def __init__(self, n_features, hidden, dropout):
        super().__init__()
        layers = []
        prev = n_features
        for h in hidden:
            layers += [nn.Linear(prev, h), nn.GELU(), nn.Dropout(dropout)]
            prev = h
        layers.append(nn.Linear(prev, 1))
        layers.append(nn.Tanh())
        self.net = nn.Sequential(*layers)

    def forward(self, x):
        return self.net(x)


def build_regime_labels(df, horizon, atr_thr):
    """Self-supervised regime labels from price action."""
    close = df['close']
    high  = df['high']
    low   = df['low']

    atr14     = compute_atr(high, low, close, 14)
    fwd_ret   = np.log(close.shift(-horizon) / close)
    threshold = (atr14 / close.replace(0, np.nan)) * atr_thr

    labels = pd.Series(0.0, index=df.index)
    labels[fwd_ret >  threshold] =  1.0
    labels[fwd_ret < -threshold] = -1.0
    return labels


# ---------- Training loop ----------
data_all = load_all_timeframes()

for tf in TIMEFRAMES:
    if tf not in data_all:
        print(f"[{tf}] No data found, skipping.")
        continue

    ohlcv = data_all[tf]["ohlcv"]
    df    = pd.DataFrame(ohlcv, columns=["open", "high", "low", "close", "volume"])

    X, _, scaler = compute_features(df)

    # Align labels to the rows that survived dropna in compute_features
    df_aligned = df.iloc[-len(X):].reset_index(drop=True)
    labels = build_regime_labels(df_aligned, HORIZON, TREND_ATR_THR)
    valid  = labels.notna() & (labels.abs() <= 1.0)
    X = X[valid.values]
    y = labels[valid].values.astype(np.float32)

    if len(X) < 100:
        print(f"[{tf}] Too few samples ({len(X)}), skipping.")
        continue

    scaler_path = f"models/regime_mlp_{tf}_scaler.json"
    with open(scaler_path, "w") as f:
        json.dump(scaler, f, indent=4)
    print(f"[{tf}] Saved scaler → {scaler_path}")

    split  = int(len(X) * (1 - VAL_SPLIT))
    X_tr_t = torch.tensor(X[:split])
    y_tr_t = torch.tensor(y[:split]).unsqueeze(1)
    X_vl_t = torch.tensor(X[split:])
    y_vl_t = torch.tensor(y[split:]).unsqueeze(1)

    loader  = DataLoader(TensorDataset(X_tr_t, y_tr_t), BATCH_SIZE, shuffle=True)
    model   = RegimeMLP(n_features=X.shape[1], hidden=HIDDEN_SIZES, dropout=DROPOUT)
    opt     = torch.optim.AdamW(model.parameters(), lr=LR, weight_decay=WEIGHT_DECAY)
    loss_fn = nn.MSELoss()

    best_val, patience_cnt, patience = float("inf"), 0, 10

    for epoch in range(EPOCHS):
        model.train()
        for xb, yb in loader:
            opt.zero_grad()
            loss_fn(model(xb), yb).backward()
            opt.step()

        model.eval()
        with torch.no_grad():
            val_loss = loss_fn(model(X_vl_t), y_vl_t).item()

        if val_loss < best_val:
            best_val = val_loss
            patience_cnt = 0
            torch.save(model.state_dict(), f"models/regime_mlp_{tf}_best.pt")
        else:
            patience_cnt += 1

        if (epoch + 1) % 10 == 0:
            print(f"[{tf}] Epoch {epoch+1}/{EPOCHS}  val_loss={val_loss:.6f}  best={best_val:.6f}")

        if patience_cnt >= patience:
            print(f"[{tf}] Early stopping at epoch {epoch+1}.")
            break

    model.load_state_dict(torch.load(f"models/regime_mlp_{tf}_best.pt", weights_only=True))
    model.eval()

    onnx_path = f"models/regime_mlp_{tf}.onnx"
    dummy = torch.randn(1, X.shape[1])
    torch.onnx.export(
        model, dummy, onnx_path,
        dynamo=False,
        export_params=True,
        opset_version=17,
        do_constant_folding=True,
        input_names=["input"],
        output_names=["output"],
        dynamic_axes={"input": {0: "batch_size"}, "output": {0: "batch_size"}},
    )
    print(f"[{tf}] Exported Regime MLP → {onnx_path}")
