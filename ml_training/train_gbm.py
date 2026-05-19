"""
train_gbm.py  —  Feature MLP (replaces Geometric Brownian Motion)
=================================================================
Trains a lightweight Multi-Layer Perceptron that learns non-linear
relationships between 5 technical indicators and the next-3-bar
log return. The trained model is exported to ONNX so the Rust
`generic_onnx` predictor can load and run it at inference time.

Why this replaces GBM
---------------------
GBM was a Monte Carlo simulator: it injected a fresh random Gaussian
draw (z ~ N(0,1)) on every single inference call, making the ensemble
non-deterministic and providing zero directional alpha. This MLP is
fully deterministic and learns genuine indicator-to-return relationships.
"""

import json
import os

import numpy as np
import torch
import torch.nn as nn
from torch.utils.data import DataLoader, TensorDataset

from dataset import load_all_timeframes
from preprocessing import compute_features

# ---------- Hyper-parameters ----------
HIDDEN_SIZES  = [32, 16]
BATCH_SIZE    = 32
EPOCHS        = 40
LR            = 5e-4
DROPOUT       = 0.2
WEIGHT_DECAY  = 1e-4
VAL_SPLIT     = 0.2
SEED          = 42
TIMEFRAMES    = ["m5", "m30", "h1", "h4", "d1"]

torch.manual_seed(SEED)
os.makedirs("models", exist_ok=True)


# ---------- Architecture ----------
class FeatureMLP(nn.Module):
    """Maps 5 normalised technical features → next 3-bar log return."""
    def __init__(self, n_features, hidden, dropout):
        super().__init__()
        layers = []
        prev = n_features
        for h in hidden:
            layers += [nn.Linear(prev, h), nn.GELU(), nn.Dropout(dropout)]
            prev = h
        layers.append(nn.Linear(prev, 1))
        layers.append(nn.Tanh())   # Bound output to (-1, 1)
        self.net = nn.Sequential(*layers)

    def forward(self, x):
        return self.net(x)


# ---------- Training loop ----------
import pandas as pd

data_all = load_all_timeframes()

for tf in TIMEFRAMES:
    if tf not in data_all:
        print(f"[{tf}] No data found, skipping.")
        continue

    ohlcv = data_all[tf]["ohlcv"]
    df = pd.DataFrame(ohlcv, columns=["open", "high", "low", "close", "volume"])

    X, y, scaler = compute_features(df)

    if len(X) < 100:
        print(f"[{tf}] Too few samples ({len(X)}), skipping.")
        continue

    # Chronological train/val split
    split = int(len(X) * (1 - VAL_SPLIT))
    X_tr, y_tr = X[:split], y[:split]
    X_vl, y_vl = X[split:], y[split:]

    # Scale target to improve training dynamics
    y_scale = float(np.std(y_tr)) if np.std(y_tr) > 0 else 1.0
    scaler['y_scale'] = y_scale

    scaler_path = f"models/feature_mlp_{tf}_scaler.json"
    with open(scaler_path, "w") as f:
        json.dump(scaler, f, indent=4)
    print(f"[{tf}] Saved scaler → {scaler_path}")

    X_tr_t = torch.tensor(X_tr)
    y_tr_t = torch.tensor(y_tr / y_scale).unsqueeze(1)
    X_vl_t = torch.tensor(X_vl)
    y_vl_t = torch.tensor(y_vl / y_scale).unsqueeze(1)

    loader  = DataLoader(TensorDataset(X_tr_t, y_tr_t), BATCH_SIZE, shuffle=True)
    model   = FeatureMLP(n_features=X.shape[1], hidden=HIDDEN_SIZES, dropout=DROPOUT)
    opt     = torch.optim.AdamW(model.parameters(), lr=LR, weight_decay=WEIGHT_DECAY)
    loss_fn = nn.MSELoss()

    best_val, patience_cnt, patience = float("inf"), 0, 8

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
            torch.save(model.state_dict(), f"models/feature_mlp_{tf}_best.pt")
        else:
            patience_cnt += 1

        if (epoch + 1) % 10 == 0:
            print(f"[{tf}] Epoch {epoch+1}/{EPOCHS}  val_loss={val_loss:.6f}  best={best_val:.6f}")

        if patience_cnt >= patience:
            print(f"[{tf}] Early stopping at epoch {epoch+1}.")
            break

    # Restore best weights
    model.load_state_dict(torch.load(f"models/feature_mlp_{tf}_best.pt", weights_only=True))
    model.eval()

    onnx_path = f"models/feature_mlp_{tf}.onnx"
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
    print(f"[{tf}] Exported Feature MLP → {onnx_path}")
