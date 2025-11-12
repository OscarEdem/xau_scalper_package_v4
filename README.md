<div align="center">

# 🌉 XAU Scalper Bridge v7

### A High-Performance Algorithmic Trading System for XAU/USD

![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)
![MQL5](https://img.shields.io/badge/MQL5-0053A3?style=for-the-badge&logo=mql5&logoColor=white)
![Docker](https://img.shields.io/badge/docker-%230db7ed.svg?style=for-the-badge&logo=docker&logoColor=white)

</div>

---

**XAU Scalper Bridge** is a sophisticated, multi-component algorithmic trading system designed for scalping the XAU/USD (Gold) market. It leverages a high-performance Rust backend for complex signal analysis and a robust MQL5 Expert Advisor (EA) for seamless integration with the MetaTrader 5 terminal.

The core philosophy is to trade **quality over quantity** by waiting for a confluence of technical factors across multiple timeframes before executing a trade.

## 🏛️ System Architecture

The system is composed of two primary components that work in tandem:

1.  **Rust Server (The Brain):** A powerful `axum` web server that receives market data from the EA. It performs complex, multi-timeframe analysis using a pre-defined strategy to calculate a trade signal and a "conviction score". It also acts as a central repository for all trade history.

2.  **MQL5 Expert Advisor (The Bridge):** An EA that runs on the MetaTrader 5 chart. Its job is to collect M1 and M5 market data, send it to the Rust server, execute trades based on the server's response, manage the position with an advanced trailing stop, and log all trade events back to the server.

```
+---------------------------+      (1) Market Data (M1/M5)      +---------------------+
|                           |----------------------------------->|                     |
|   MetaTrader 5 Terminal   |              (JSON)                |     Rust Server     |
|      (MQL5 EA)            |<-----------------------------------|      (The Brain)      |
|                           |  (2) Trade Signal & Conviction     |                     |
+---------------------------+              (JSON)                +---------------------+
           |
           | (3) Open/Close/Log Trades
           v
+---------------------------+
|                           |
|      Trading Broker       |
|                           |
+---------------------------+
```

## ✨ Key Features

-   **Multi-Timeframe Analysis:** Uses M5 data for trend direction and M1 data for precise entries, filtering out market noise.
-   **Confluence-Based Strategy:** Executes trades only when **five** distinct technical conditions align, generating a "Conviction Score" for each potential signal.
-   **Dynamic Risk Management:** Automatically adjusts lot size based on the conviction score, risking more on "A+" setups and less (or nothing) on weaker signals.
-   **Advanced Trailing Stop:** Implements an ATR-based "Chandelier Exit" to let winning trades run and protect profits adaptively based on market volatility.
-   **Centralized Trade Analytics API:** All trades are logged to the server, which exposes an interactive API (`/history`) with performance statistics (P/L, Win Rate, Profit Factor) and date filtering.
-   **Interactive API Documentation:** Automatically generated Swagger UI provides a beautiful and easy-to-use interface for exploring the server's API.
-   **High-Performance Backend:** Built in Rust for speed, safety, and reliability, ensuring signals are processed with minimal latency.
-   **Comprehensive MQL5 Dashboard:** A clean, modern on-chart interface displays the current signal, server status, market info, and key indicator values in real-time.
-   **Dockerized Deployment:** Fully containerized with a multi-stage `Dockerfile` for easy, consistent deployment on any cloud server or local machine.
-   **Parallelized Backtester:** Includes a powerful, multi-threaded backtester written in Rust to rapidly optimize strategy parameters over historical data.

---

## 🚀 Installation and Usage

### Prerequisites
-   Rust Language Toolchain
-   Docker Desktop
-   MetaTrader 5 Terminal

### 1. The Rust Server

The server is the core analytical engine.

#### Build and Run Locally:
```bash
# Navigate to the project root directory
cd /path/to/xau_scalper_package_v4-main

# Build the server in release mode
cargo build --release --package xau-scalper-server

# Run the server
./target/release/xau-scalper-server
```

#### Build and Run with Docker:
```bash
# Build the Docker image
docker build -t xau-scalper-v7 .

# Run the server inside a container
docker run --rm -p 3000:3000 xau-scalper-v7
```
The server will be accessible at `http://127.0.0.1:3000`.

### 2. The MQL5 Expert Advisor

The EA connects MetaTrader 5 to your Rust server.

1.  **Copy the EA File:** Copy `mt5-ea/XAU_ScalperBridge_Fixed.mq5` into your MT5 data folder under `MQL5/Experts/`.
2.  **Compile:** Open MetaEditor in MT5, find the EA in the Navigator, and press `F7` to compile it.
3.  **Configure MT5:**
    -   Go to `Tools -> Options -> Expert Advisors`.
    -   Check `Allow WebRequest for listed URL`.
    -   Add `http://127.0.0.1:3000`.
4.  **Attach to Chart:**
    -   Open an **M1** chart for **XAUUSD**.
    -   Drag the `XAU_ScalperBridge_Fixed` EA onto the chart.
    -   In the **"Inputs"** tab, ensure the parameters are set correctly. **Crucially, set `NumCloses` to `250` or higher** to provide enough data to the server.

### 3. Running the System

With both the server running and the EA attached to the chart, the system is live. The EA will send data on each new M1 bar, and the on-chart dashboard will update with the server's response.

---

## 🔬 API and Analytics

The server provides a powerful interface for monitoring and analysis.

### Interactive API Docs (Swagger UI)

Navigate to `http://127.0.0.1:3000/swagger-ui` in your browser to see a full, interactive documentation of all available API endpoints.

### API Endpoints

| Method | Endpoint      | Description                                                                                             |
| :----- | :------------ | :------------------------------------------------------------------------------------------------------ |
| `POST` | `/eval`       | The core endpoint used by the EA to get a trade signal.                                                 |
| `POST` | `/log_trade`  | Used by the EA to send details of opened and closed trades to the server for logging.                     |
| `GET`  | `/history`    | **Analytics endpoint.** Returns trade history with performance stats. Can be filtered by `start_date` and `end_date`. |
| `GET`  | `/health`     | A simple health check endpoint that returns "OK".                                                       |

### Analyzing Performance

To view a performance report, access the history endpoint in your browser:

`http://127.0.0.1:3000/history`

To filter for a specific period:

`http://127.0.0.1:3000/history?start_date=2023-11-01&end_date=2023-11-30`

---

## ⚙️ EA Parameters

The MQL5 EA has several input parameters for customization:

| Parameter              | Description                                                                    | Default Value |
| ---------------------- | ------------------------------------------------------------------------------ | ------------- |
| `ServerUrl`            | The URL of the Rust server's `/eval` endpoint.                                 | `http://127.0.0.1:3000/eval` |
| `RiskPercent`          | The percentage of account balance to risk on a full-conviction (score=5) trade. | `0.5`         |
| `NumCloses`            | **IMPORTANT:** Number of historical bars to send to the server. Must be > 202. | `80` (Change to `250`) |
| `MaxSpreadPoints`      | The maximum allowed spread in points to place a trade.                         | `160`         |
| `MagicNumber`          | A unique ID to distinguish this EA's trades from others.                       | `1337`        |
| `RsiPeriod`            | The period for the RSI indicator.                                              | `16`          |
| `EmaFastPeriod`        | The period for the fast EMA.                                                   | `5`           |
| `EmaSlowPeriod`        | The period for the slow EMA.                                                   | `50`          |
| `AtrPeriod`            | The period for the ATR indicator.                                              | `14`          |
| `SlAtrMultiplier`      | The multiplier for calculating the initial Stop Loss based on ATR.             | `1.0`         |
| `TpAtrMultiplier`      | The multiplier for calculating the initial Take Profit based on ATR.           | `1.5`         |
| `UseTrailingStop`      | Enables or disables the ATR-based trailing stop.                               | `true`        |
| `TrailingStopATRMlt`   | The ATR multiplier for the trailing stop.                                      | `1.0`         |
| `EnableServerLogging`  | Enables or disables sending trade logs to the server.                          | `true`        |

---

## ⚡ Backtesting and Optimization

The project includes a high-speed, parallelized backtester to find the optimal strategy parameters.

1.  **Prepare Data:** Ensure you have `m1_data_comma.csv` and `m5_data_comma.csv` in the `/app` directory inside the container. The `Dockerfile` handles the conversion from the raw MT5 export format.

2.  **Run the Backtester:** Use Docker to run the backtester binary. This command will test thousands of parameter combinations in parallel.

    ```bash
    docker run --rm xau-scalper-v7 backtest --m1-file /app/m1_data_comma.csv --m5-file /app/m5_data_comma.csv
    ```

3.  **Analyze Results:** The backtester will output the best parameter sets based on different metrics (Sharpe Ratio, Profit Factor / Max Drawdown), which you can then use in the live EA.

    **Example Backtest Result:**
    ```
    --- Best Result (Optimized for Profit Factor / Max Drawdown) ---
    Parameters: EMA(5/50), RSI(16), SL: 1.0*ATR, TP: 1.5*ATR
    Sharpe Ratio: 0.167 | Profit Factor: 3.99
    Final Balance: 123093006844.01
    Net Profit: 123092996844.01
    Max Drawdown: 2.68%
    Total Trades: 3546
    Win Rate: 66.84%
    ```

---

## 📜 License

This project is licensed under the MIT License. See the `LICENSE` file for details.

