<div align="center">

# 🌉 XAU Scalper Bridge v4

### A High-Performance Algorithmic Trading System for XAU/USD

![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)
![Python](https://img.shields.io/badge/python-3670A0?style=for-the-badge&logo=python&logoColor=ffdd54)
![MQL5](https://img.shields.io/badge/MQL5-0053A3?style=for-the-badge&logo=mql5&logoColor=white)
![Docker](https://img.shields.io/badge/docker-%230db7ed.svg?style=for-the-badge&logo=docker&logoColor=white)

</div>

---

**XAU Scalper Bridge** is a three-component algorithmic trading system for scalping and swing trading XAU/USD (Gold). A Rust backend generates AI-powered trade signals. A Python sidecar consumes those signals via WebSocket and executes trades through the MetaTrader 5 API. An MQL5 EA feeds live market data to the server.

> **Philosophy:** Trade quality over quantity — wait for a confluence of technical factors across multiple timeframes before executing.

---

## 🏛️ System Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  MT5 Terminal                                                        │
│                                                                      │
│  XAU_DATA_BRIDGE.mq5   ─── POST /data (every M1 bar) ─────────────►│
│  (Market Data Sender)  ─── POST /ticks (every 500ms) ─────────────►│
└──────────────────────────────────────────────────────────────────────┘
                                                    │
                                                    ▼
                              ┌──────────────────────────────────────┐
                              │  Rust Server  (The Brain)            │
                              │  hosted on Render / Docker           │
                              │                                      │
                              │  ├─ ScalpEngine (M1 / M5)           │
                              │  │   ├─ LondonHunt                  │
                              │  │   ├─ Momentum (Kalman)           │
                              │  │   ├─ Pullback (2-bar confirm)    │
                              │  │   └─ Fade (Bollinger + ADX)      │
                              │  └─ SwingEngine (H1 / H4 / D1)     │
                              │      ├─ HTF Bias (Daily + H4)       │
                              │      ├─ BOS / CHoCH retest latch    │
                              │      ├─ OB + FVG Detection          │
                              │      └─ Ensemble (GBM/LSTM/Heston)  │
                              │                                      │
                              │  Signal ──► WebSocket /ws ──────────┼──►
                              └──────────────────────────────────────┘  │
                                                                         │
                              ┌──────────────────────────────────────┐  │
                              │  Python Sidecar  (The Executor)      │◄─┘
                              │  xau_controller.py + mt5_interface   │
                              │                                      │
                              │  ├─ WebSocket signal consumer        │
                              │  ├─ Mode / Session / Conviction gate │
                              │  ├─ mt5.order_send() execution       │
                              │  ├─ ATR-capped trailing SL          │
                              │  └─ Stagnation time-stop (5 min)    │
                              └──────────────────────────────────────┘
                                              │
                                              ▼
                                     MetaTrader 5 Broker
```

---

## ✨ Key Features

- **Three-component pipeline:** Data bridge (MQL5) → Signal engine (Rust) → Executor (Python)
- **Multi-timeframe analysis:** M1/M5 for scalp entries, H1/H4/D1 for swing bias
- **SMC signal logic:** BOS retest latch, FVG/OB bounces, SFP detection
- **Dynamic conviction scoring:** LondonHunt and Fade scores scale with setup quality
- **AI ensemble:** LSTM + GBM + Heston models provide directional bias confirmation
- **Server-side position sizing:** Lot size = equity% / (SL distance × point value)
- **Session-aware gating:** Momentum blocked in Asia and late-NY; Sydney scalps disabled
- **Stagnation time-stop:** Scalp trades auto-close after 5 min if flat
- **ATR-capped trailing:** Trailing stop can only tighten, never widen
- **Push notifications:** Expo push alerts for high-conviction signals
- **WebSocket live feed:** Real-time tick broadcast to sidecar and dashboards
- **Dockerized deployment:** Multi-stage `Dockerfile` for Render or any cloud
- **Interactive API docs:** Swagger UI at `/docs`

---

## 🚀 Quick Start

### Prerequisites

- Rust toolchain (`rustup`)
- Python 3.10+ with `pip`
- MetaTrader 5 Terminal (Windows)
- Docker Desktop (optional)

---

### 1. Rust Server

```bash
# Build and run locally
cd xau_scalper_package_v4-main
cargo build --release --package xau-scalper-server
./target/release/xau-scalper-server

# OR via Docker
docker build -t xau-scalper-v4 .
docker run --rm -p 3000:3000 xau-scalper-v4
```

The server starts at `http://0.0.0.0:3000`.

**Key environment variable overrides:**

| Variable | Description | Default |
|----------|-------------|---------|
| `APP__SERVER__PORT` | Listening port | `3000` |
| `APP__TRADING__SCALP__M1_ROC_PERIOD` | M1 surge lookback bars | `3` |
| `APP__TRADING__SWING__CONVICTION_THRESHOLD` | Min swing score to emit signal | `62.0` |
| `APP__TRADING__RISK__RISK_PER_TRADE_PCT` | Equity % risked per trade | `0.01` |
| `DATABASE_URL` | PostgreSQL connection string | _(optional)_ |

> All fields in `config.rs` can be overridden at runtime with `APP__TRADING__SCALP__<FIELD>` or `APP__TRADING__SWING__<FIELD>` env vars.

---

### 2. MQL5 Data Bridge

`XAU_DATA_BRIDGE.mq5` sends OHLCV market data to the Rust server. **It does NOT execute trades.**

1. Copy `mt5-ea/XAU_DATA_BRIDGE.mq5` → MT5 `MQL5/Experts/`
2. Compile in MetaEditor (`F7`)
3. `Tools → Options → Expert Advisors → Allow WebRequest` — add your server URL
4. Attach to an **M1 XAUUSD** chart

**EA Parameters:**

| Parameter | Description | Recommended |
|-----------|-------------|-------------|
| `ServerUrl` | Rust server base URL | `https://xau-scalper-pro.duckdns.org` |
| `NumCloses` | Historical bars sent per timeframe | `300` (min 250) |
| `MaxSpreadPoints` | Skip send if spread exceeds this | `162` |
| `EnableTickBridge` | Send live ticks to `/ticks` | `true` |
| `TickBridgeInterval` | Min ms between tick posts | `500` |

---

### 3. Python Sidecar (Trade Executor)

The sidecar receives signals over WebSocket and places/manages trades via the MT5 Python API.

> 📄 Full documentation: **[Python_Sidecar/README.md](../Python_Sidecar/README.md)**

```bash
cd Python_Sidecar
pip install -r requirements.txt
python xau_controller.py
# OR double-click run_bot.bat
```

**Critical config values (set in GUI Settings or `config.py`):**

| Setting | Value | Reason |
|---------|-------|--------|
| `max_entries` | `1` | No stacking — one position per signal |
| `min_conviction` | `60.0` | Filter low-confluence signals |
| `trailing_start_pips_scalp` | `12.0` | Activate early to protect median wins |
| `trailing_dist_pips_scalp` | `15.0` | ATR trail capped here (tighten only) |
| `stagnation_sec` | `300` | Hard-close scalps after 5 min |
| `use_stagnation` | `true` | Must be enabled |

---

## ⚙️ Strategy Configuration

All server-side parameters are in `rust-server/src/config.rs` and override-able via `config.toml` or `APP__` env vars.

### Scalp Engine (v4.2 defaults)

| Parameter | Value | Change from v4.1 |
|-----------|-------|-----------------|
| `m1_roc_period` | `3` | Was `1` — now requires 3-bar sustained surge |
| `min_conviction` | `60.0` | Was `50.0` |
| `pullback_entry_displacement_atr` | `0.30` | Was `0.20` — deeper pullbacks only |
| `filter_scalp_by_swing` | `true` | Was `false` — scalps must align with H1 |
| `allow_asia_trading` | `false` | Asia is a loss zone |
| `allow_london_open_momentum` | `false` | London open produces fake-outs |
| `momentum_require_ml_confluence` | `true` | ML must confirm momentum direction |

### Swing Engine (v4.2 defaults)

| Parameter | Value | Change from v4.1 |
|-----------|-------|-----------------|
| `conviction_threshold` | `62.0` | Was `50.0` — requires ≥2 confluence bonuses |
| `ensemble_h1_weight` | `0.7` | H1 ML bias weight |
| `ensemble_d1_weight` | `0.3` | D1 ML bias weight |
| `sl_atr_buffer` | `0.25` | SL buffer below structural level |

---

## 🔬 API Reference

Interactive docs: `http://127.0.0.1:3000/docs`

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/data` | Ingest OHLCV from MT5 bridge |
| `POST` | `/ticks` | Ingest live tick data |
| `GET` | `/ws` | WebSocket — live signal stream |
| `GET` | `/signals` | Last 12h signal log |
| `GET` | `/signals/latest` | Latest signal per symbol |
| `GET` | `/signals/{symbol}` | Latest signal for one symbol |
| `GET` | `/analysis/fundamental` | Gemini AI macro outlook |
| `GET` | `/models/loaded` | Loaded ONNX predictor models |
| `GET` | `/metrics` | Server performance metrics |
| `GET` | `/metrics/prometheus` | Prometheus-format metrics |
| `GET` | `/health` | Health check → `"OK"` |

---

## ⚡ Backtesting

```bash
docker run --rm xau-scalper-v4 backtest \
  --m1-file /app/m1_data_comma.csv \
  --m5-file /app/m5_data_comma.csv
```

> ⚠️ **Always run backtests with a fixed lot size** (e.g. `0.01`) and enable spread + slippage simulation. The example below used unbounded compounding — the dollar figure is meaningless. Judge results by **Win Rate**, **Profit Factor**, and **Max Drawdown** only.

```
--- Example Backtest Result (fixed-lot reference) ---
Parameters: EMA(5/50), RSI(16), SL: 1.0×ATR, TP: 1.5×ATR
Sharpe Ratio:  0.167   |  Profit Factor: 3.99
Win Rate:      66.84%  |  Total Trades:  3,546
Max Drawdown:  2.68%
```

---

## 🛠️ v4.2 Changelog — Loss Analysis Fixes

| Component | Change |
|-----------|--------|
| `scalp.rs` | `m1_roc_period` 1→3: require 3-bar sustained M1 surge |
| `scalp.rs` | Pullback: 2 consecutive closes required (no falling-knife) |
| `scalp.rs` | `pullback_entry_displacement_atr` 0.20→0.30 |
| `scalp.rs` | LondonHunt conviction dynamic (sweep depth/ATR), was hardcoded 90 |
| `scalp.rs` | Fade conviction dynamic (inversely with ADX), was hardcoded 85 |
| `swing.rs` | BOS retest latch: signal fires on retest, not breakout candle |
| `config.rs` | `conviction_threshold` 50→62, `min_conviction` 50→60 |
| `config.rs` | `filter_scalp_by_swing` enabled; `pullback_entry_displacement_atr` raised |
| `XAU_DATA_BRIDGE.mq5` | M5 timestamps now sent — fixes London Hunt Asia range accuracy |
| `config.py` | `max_entries` 5→1, `min_conviction` 45→60 |
| `mt5_interface.py` | ATR trailing capped at `trailing_dist_pips_*` — tightens only |
| `mt5_interface.py` | SL updates sequential (MT5 Python API is not thread-safe) |
| `xau_controller.py` | `processed_ids` → bounded `deque(maxlen=500)` |

---

## 📜 License

This project is licensed under the MIT License. See `LICENSE` for details.
