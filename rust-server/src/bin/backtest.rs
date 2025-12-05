use rayon::prelude::*;
use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;
use xau_scalper_server::{EvalRequest, TradingSession};

#[derive(Debug, Deserialize, Clone)] // Deserialize by position
pub struct Candle {
    time: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: u32,
}

/// Holds a snapshot of all required data for a single M1 candle in the backtest.
#[derive(Debug, Clone)]
struct CombinedCandle {
    m1_open: f64,
    m1_high: f64,
    m1_low: f64,
    m1_close: f64,
    m5_closes: Vec<f64>,
    m5_highs: Vec<f64>,
    m5_lows: Vec<f64>,
    m30_closes: Vec<f64>,
    h1_closes: Option<Vec<f64>>,
    h1_opens: Option<Vec<f64>>,
    h1_highs: Option<Vec<f64>>,
    h1_lows: Option<Vec<f64>>,
    h4_closes: Option<Vec<f64>>,
    h4_highs: Option<Vec<f64>>,
    h4_lows: Option<Vec<f64>>,
    d1_opens: Option<Vec<f64>>,
    d1_closes: Option<Vec<f64>>,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "backtest", about = "A backtester for the XAU/USD scalper strategy.")]
struct Opt {
    /// Path to the M1 CSV file
    #[structopt(long, parse(from_os_str))]
    m1_file: PathBuf,

    /// Number of historical bars to send in each evaluation request
    #[structopt(short, long, default_value = "80")]
    history_size: usize,

    /// Initial account balance
    #[structopt(short, long, default_value = "10000")]
    balance: f64,

    /// Risk per trade as a percentage of balance
    #[structopt(short, long, default_value = "1.0")]
    risk_percent: f64,

    /// Path to the M5 CSV file
    #[structopt(long, parse(from_os_str))]
    m5_file: PathBuf,

    /// Path to the M30 CSV file (optional)
    #[structopt(long, parse(from_os_str))]
    m30_file: Option<PathBuf>,

    /// Path to the H1 CSV file (optional)
    #[structopt(long, parse(from_os_str))]
    h1_file: Option<PathBuf>,

    /// Path to the H4 CSV file (optional)
    #[structopt(long, parse(from_os_str))]
    h4_file: Option<PathBuf>,

    /// Path to the D1 CSV file (optional)
    #[structopt(long, parse(from_os_str))]
    d1_file: Option<PathBuf>,

    /// Path to the news events CSV file (optional)
    #[structopt(long, parse(from_os_str))]
    news_file: Option<PathBuf>,

    /// Predictor model to use (e.g., "gbm", "heston", "lstm")
    #[structopt(long, default_value = "gbm")]
    predictor_model: String,
}

#[derive(Debug)]
enum TradeDirection {
    Long,
    Short,
}

#[derive(Debug)]
struct Trade {
    direction: TradeDirection,
    entry_price: f64,
    stop_loss: f64,
    take_profit: f64,
    lot_size: f64,
    // --- NEW: Add fields for trailing stop ---
    entry_candle_index: usize,
}

#[derive(Debug, Clone)]
struct BacktestParams {
    ema_fast: usize,
    ema_slow: usize,
    rsi_period: usize,
    sl_atr_multiplier: f64,
    tp_atr_multiplier: f64,
    min_sl_pips: f64,
    max_sl_pips: f64,
    kf_process_noise: f64,
    kf_measurement_noise: f64,
}

#[derive(Debug, Clone)]
struct BacktestResult {
    params: BacktestParams,
    final_balance: f64,
    net_profit: f64,
    total_trades: usize,
    win_rate: f64,
    max_drawdown: f64,
    sharpe_ratio: f64,
    profit_factor: f64,
}

/// Runs a single backtest with a given set of parameters.
fn run_single_backtest(params: BacktestParams, combined_candles: &[CombinedCandle], opt: &Opt) -> Option<BacktestResult> {
    let mut balance = opt.balance;
    let mut active_trade: Option<Trade> = None;
    let mut wins = 0;
    let mut losses = 0;
    let mut pnl_history = Vec::new();
    let mut peak_balance = balance;
    let mut gross_profit = 0.0;
    let mut gross_loss = 0.0;
    let mut max_drawdown: f64 = 0.0;

    // Create a single trading session for this backtest run.
    let mut session = TradingSession::new("XAUUSD".to_string(), true);

    // --- NEW: Safety check to prevent panics ---
    if combined_candles.len() <= opt.history_size {
        println!("Not enough candle data ({}) to meet history requirement ({}). Skipping this parameter set.", combined_candles.len(), opt.history_size);
        return None;
    }

    for i in opt.history_size..combined_candles.len() {
        let combined_candle = &combined_candles[i];

        // --- OPTIMIZATION: Slice only the necessary window of data ---
        let history_start_idx = i.saturating_sub(opt.history_size);

        // Build EvalRequest for the strategy
        let req = EvalRequest {
            symbol: "XAUUSD".to_string(),
            timeframe: "M1".to_string(),
            closes: combined_candles[history_start_idx..=i].iter().map(|c| c.m1_close).collect(),
            highs: combined_candles[history_start_idx..=i].iter().map(|c| c.m1_high).collect(),
            opens: combined_candles[history_start_idx..=i].iter().map(|c| c.m1_open).collect(),
            lows: combined_candles[history_start_idx..=i].iter().map(|c| c.m1_low).collect(),
            volumes: vec![0; i + 1], // Not used in this backtest, provide dummy data
            m5_closes: combined_candle.m5_closes.clone(),
            m5_highs: combined_candle.m5_highs.clone(),
            m5_lows: combined_candle.m5_lows.clone(),
            m30_closes: combined_candle.m30_closes.clone(),
            h1_closes: combined_candle.h1_closes.clone(),
            h1_opens: combined_candle.h1_opens.clone(),
            h1_highs: combined_candle.h1_highs.clone(),
            h1_lows: combined_candle.h1_lows.clone(),
            h4_closes: combined_candle.h4_closes.clone(),
            h4_highs: combined_candle.h4_highs.clone(),
            h4_lows: combined_candle.h4_lows.clone(),
            d1_opens: combined_candle.d1_opens.clone(),
            d1_closes: combined_candle.d1_closes.clone(),
            open_positions: None, // No open positions for backtest
            rsi_period: Some(params.rsi_period),
            ema_fast: Some(params.ema_fast),
            ema_mid: Some(8), // Default
            ema_slow: Some(params.ema_slow),
            atr_period: Some(14), // Default
            sma_period: Some(200), // Default
            stoch_k_period: Some(14), // Default
            stoch_d_period: Some(3), // Default
            stoch_slowing: Some(3), // Default
            tp1_pips: Some(params.tp_atr_multiplier * 10.0), // Convert multiplier to pips for simplicity
            tp2_pips: None,
            sl_pips: Some(0.0), // Not used
            tp_atr_multiplier: Some(params.tp_atr_multiplier),
            kf_process_noise: Some(params.kf_process_noise),
            kf_measurement_noise: Some(params.kf_measurement_noise),
            sl_atr_multiplier: Some(params.sl_atr_multiplier),
            confirmation_bars: Some(2), // Default
            spread_limit_points: Some(30.0), // Default
            atr_regime_threshold: Some(2.0), // Default
            spread_points: Some(0.0), // Default
            price_decimals: Some(2), // Default
            min_sl_pips: Some(params.min_sl_pips),
            max_sl_pips: Some(params.max_sl_pips),
            roc_period: Some(3), // Default
            roc_threshold: Some(0.18), // Default
            adx_period: Some(14), // Default
            adx_threshold: Some(25.0), // Default
            persistence_bars: Some(5), // Default
            chandelier_period: Some(22), // Default
            chandelier_atr_mult: Some(3.0), // Default
            max_hold_bars: Some(100), // Default
            max_risk_pct: Some(0.01), // Default 1% risk
            imbalance_mitigation: Some(true),
            last_m1_timestamp: 0,
            last_m5_timestamp: None,
            last_m30_timestamp: None,
            last_h1_timestamp: None,
            mode: "scalp".to_string(), // Hardcode to scalp for this backtest
            current_price: combined_candle.m1_close,
            upcoming_events: Some(vec![]), // Assume no news for backtest
            predictor_model: Some(opt.predictor_model.clone()),
        };

        // Process the data through the session to generate signals.
        session.on_data(&req);
        // We are only interested in the scalp signal for this backtest.
        let (scalp_signal, _swing_signal) = session.get_latest_signals();
        let response = scalp_signal.unwrap_or_default();

        // --- Check if active trade should be closed ---
        if let Some(trade) = active_trade.take() {
            let (pnl, _reason) = match trade.direction {
                TradeDirection::Long => {
                    if combined_candle.m1_low <= trade.stop_loss {
                        (-(trade.entry_price - trade.stop_loss) * trade.lot_size, "Stop Loss")
                    } else if combined_candle.m1_high >= trade.take_profit {
                        ((trade.take_profit - trade.entry_price) * trade.lot_size, "Take Profit")
                    } else {
                        // Trade still active
                        active_trade = Some(trade);
                        (0.0, "")
                    }
                }
                TradeDirection::Short => {
                    if combined_candle.m1_high >= trade.stop_loss {
                        (-(trade.stop_loss - trade.entry_price) * trade.lot_size, "Stop Loss")
                    } else if combined_candle.m1_low <= trade.take_profit {
                        ((trade.entry_price - trade.take_profit) * trade.lot_size, "Take Profit")
                    } else {
                        // Trade still active
                        active_trade = Some(trade);
                        (0.0, "")
                    }
                }
            };

            if pnl != 0.0 {
                balance += pnl;
                pnl_history.push(pnl);

                peak_balance = peak_balance.max(balance);
                let drawdown = (peak_balance - balance) / peak_balance;
                max_drawdown = max_drawdown.max(drawdown);

                if pnl > 0.0 {
                    wins += 1;
                    gross_profit += pnl;
                } else {
                    losses += 1;
                    gross_loss += pnl.abs();
                }
                continue;
            }
        }

        // --- Check for new trade signals ---
        if active_trade.is_none() {
            if response.entry_type == "long" {
                let entry_price = response.entry_price;
                let stop_loss = response.sl_price;
                let take_profit = response.tp1_price;
                let risk_per_point = (entry_price - stop_loss).abs();
                if risk_per_point <= 0.0 { continue; }
                
                let risk_amount = balance * (opt.risk_percent / 100.0);
                let lot_size = risk_amount / risk_per_point;

                active_trade = Some(Trade { direction: TradeDirection::Long, entry_price, stop_loss, take_profit, lot_size, entry_candle_index: i });
            } else if response.entry_type == "short" {
                let entry_price = response.entry_price;
                let stop_loss = response.sl_price;
                let take_profit = response.tp1_price;
                let risk_per_point = (entry_price - stop_loss).abs();
                if risk_per_point <= 0.0 { continue; }
                
                let risk_amount = balance * (opt.risk_percent / 100.0);
                let lot_size = risk_amount / risk_per_point;

                active_trade = Some(Trade { direction: TradeDirection::Short, entry_price, stop_loss, take_profit, lot_size, entry_candle_index: i });
            }
        }
    }

    let total_trades = wins + losses;
    if total_trades < 10 { return None; } // Ignore results with too few trades

    let win_rate = (wins as f64 / total_trades as f64) * 100.0;
    let net_profit = balance - opt.balance;

    let profit_factor = if gross_loss > 0.0 {
        gross_profit / gross_loss
    } else {
        f64::INFINITY // Infinite profit factor if no losses
    };

    let sharpe_ratio = if pnl_history.len() > 1 {
        let mean_pnl = pnl_history.iter().sum::<f64>() / pnl_history.len() as f64;
        let std_dev = {
            let variance = pnl_history.iter().map(|v| (v - mean_pnl).powi(2)).sum::<f64>() / (pnl_history.len() - 1) as f64;
            variance.sqrt()
        };
        if std_dev > 0.0 { mean_pnl / std_dev } else { 0.0 }
    } else {
        0.0
    };

    Some(BacktestResult {
        params,
        final_balance: balance,
        net_profit,
        total_trades,
        win_rate,
        max_drawdown: max_drawdown * 100.0,
        sharpe_ratio,
        profit_factor,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let opt = Opt::from_args();
    println!("Starting backtest with options: {:?}", opt);

    // --- Load M1 and M5 data ---
    let m1_file = File::open(&opt.m1_file)?;
    let mut m1_rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(m1_file);
    let m1_candles: Vec<Candle> = m1_rdr.deserialize().collect::<Result<_, _>>()?;
    println!("Loaded {} M1 candles.", m1_candles.len());

    let m5_file = File::open(&opt.m5_file)?;
    let mut m5_rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(m5_file);
    let m5_candles: Vec<Candle> = m5_rdr.deserialize().collect::<Result<_, _>>()?;
    println!("Loaded {} M5 candles.", m5_candles.len());

    // Load M30 data if provided
    let m30_candles = if let Some(ref path) = opt.m30_file {
        let file = File::open(path)?;
        let mut rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(file);
        let candles: Vec<Candle> = rdr.deserialize().collect::<Result<_, _>>()?;
        println!("Loaded {} M30 candles.", candles.len());
        Some(candles)
    } else {
        None
    };

    // Load H1 data if provided
    let h1_candles = if let Some(ref path) = opt.h1_file {
        let file = File::open(path)?;
        let mut rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(file);
        let candles: Vec<Candle> = rdr.deserialize().collect::<Result<_, _>>()?;
        println!("Loaded {} H1 candles.", candles.len());
        Some(candles)
    } else {
        None
    };

    // Load H4 data if provided
    let h4_candles = if let Some(ref path) = opt.h4_file {
        let file = File::open(path)?;
        let mut rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(file);
        let candles: Vec<Candle> = rdr.deserialize().collect::<Result<_, _>>()?;
        println!("Loaded {} H4 candles.", candles.len());
        Some(candles)
    } else {
        None
    };

    // Load D1 data if provided
    let d1_candles = if let Some(ref path) = opt.d1_file {
        let file = File::open(path)?;
        let mut rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(file);
        let candles: Vec<Candle> = rdr.deserialize().collect::<Result<_, _>>()?;
        println!("Loaded {} D1 candles.", candles.len());
        Some(candles)
    } else {
        None
    };


    // --- Pre-calculate indicators for both timeframes ---
    // M1 Indicators
    let _m1_opens: Vec<f64> = m1_candles.iter().map(|c| c.open).collect();

    // M5 Indicators
    let m5_closes: Vec<f64> = m5_candles.iter().map(|c| c.close).collect();
    let m5_highs: Vec<f64> = m5_candles.iter().map(|c| c.high).collect();
    let m5_lows: Vec<f64> = m5_candles.iter().map(|c| c.low).collect();

    // M30 Indicators
    let m30_closes = m30_candles.as_ref().map(|c| c.iter().map(|candle| candle.close).collect::<Vec<f64>>()).unwrap_or_default();

    // H1 Indicators
    let h1_closes = h1_candles.as_ref().map(|c| c.iter().map(|candle| candle.close).collect::<Vec<f64>>());
    let h1_opens = h1_candles.as_ref().map(|c| c.iter().map(|candle| candle.open).collect::<Vec<f64>>());
    let h1_highs = h1_candles.as_ref().map(|c| c.iter().map(|candle| candle.high).collect::<Vec<f64>>());
    let h1_lows = h1_candles.as_ref().map(|c| c.iter().map(|candle| candle.low).collect::<Vec<f64>>());

    // H4 Indicators
    let h4_closes = h4_candles.as_ref().map(|c| c.iter().map(|candle| candle.close).collect::<Vec<f64>>());
    let h4_highs = h4_candles.as_ref().map(|c| c.iter().map(|candle| candle.high).collect::<Vec<f64>>());
    let h4_lows = h4_candles.as_ref().map(|c| c.iter().map(|candle| candle.low).collect::<Vec<f64>>());

    // D1 Indicators
    let d1_opens = d1_candles.as_ref().map(|c| c.iter().map(|candle| candle.open).collect::<Vec<f64>>());
    let d1_closes = d1_candles.as_ref().map(|c| c.iter().map(|candle| candle.close).collect::<Vec<f64>>());

    // --- Combine M1 and M5 data ---
    // Simplified alignment: assume M1 and M5 data are aligned by index for backtest purposes
    let mut combined_candles: Vec<CombinedCandle> = Vec::new();

    // Combine data
    for i in 0..m1_candles.len() {
        // Simplified time alignment for backtesting
        let m5_idx = i / 5;
        let m30_idx = i / 30;
        let h1_idx = i / 60;
        let h4_idx = i / 240;
        let d1_idx = i / 1440;

        let combined = CombinedCandle {
            m1_open: m1_candles[i].open,
            m1_high: m1_candles[i].high,
            m1_low: m1_candles[i].low,
            m1_close: m1_candles[i].close,
            m5_closes: m5_closes.iter().take(m5_idx + 1).cloned().collect(),
            m5_highs: m5_highs.iter().take(m5_idx + 1).cloned().collect(),
            m5_lows: m5_lows.iter().take(m5_idx + 1).cloned().collect(),
            m30_closes: m30_closes.iter().take(m30_idx + 1).cloned().collect(),
            h1_closes: h1_closes.as_ref().map(|d| d.iter().take(h1_idx + 1).cloned().collect()),
            h1_opens: h1_opens.as_ref().map(|d| d.iter().take(h1_idx + 1).cloned().collect()),
            h1_highs: h1_highs.as_ref().map(|d| d.iter().take(h1_idx + 1).cloned().collect()),
            h1_lows: h1_lows.as_ref().map(|d| d.iter().take(h1_idx + 1).cloned().collect()),
            h4_closes: h4_closes.as_ref().map(|d| d.iter().take(h4_idx + 1).cloned().collect()),
            h4_highs: h4_highs.as_ref().map(|d| d.iter().take(h4_idx + 1).cloned().collect()),
            h4_lows: h4_lows.as_ref().map(|d| d.iter().take(h4_idx + 1).cloned().collect()),
            d1_opens: d1_opens.as_ref().map(|d| d.iter().take(d1_idx + 1).cloned().collect()),
            d1_closes: d1_closes.as_ref().map(|d| d.iter().take(d1_idx + 1).cloned().collect()),
        };

        combined_candles.push(combined);
    }

    println!("Combined {} M1 candles with higher timeframes.", combined_candles.len());

    // --- Define Parameter Ranges for Optimization ---
    let ema_fast_periods = vec![3]; // Fixed for now
    let ema_slow_periods = vec![50]; // Fixed for now
    let rsi_periods = vec![16]; // Fixed for now
    let sl_multipliers = vec![1.0, 1.5];
    let tp_multipliers = vec![1.5, 2.0];
    let min_sl_pips_range = vec![15.0, 25.0];
    let max_sl_pips_range = vec![150.0, 200.0, 250.0];
    let kf_q_range = vec![0.01, 0.005];
    let kf_r_range = vec![0.1, 0.2];

    let mut param_combinations = Vec::new();
    for fast_p in ema_fast_periods {
        for slow_p in ema_slow_periods.clone() {
            for rsi_p in rsi_periods.clone() {
                for min_sl in &min_sl_pips_range {
                    for max_sl in &max_sl_pips_range {
                        for sl_m in sl_multipliers.iter() {
                            for tp_m in tp_multipliers.iter() {
                                for kf_q in &kf_q_range {
                                    for kf_r in &kf_r_range {
                                        param_combinations.push(BacktestParams {
                                            ema_fast: fast_p, ema_slow: slow_p, rsi_period: rsi_p,
                                            sl_atr_multiplier: *sl_m, tp_atr_multiplier: *tp_m,
                                            min_sl_pips: *min_sl, max_sl_pips: *max_sl,
                                            kf_process_noise: *kf_q, kf_measurement_noise: *kf_r,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("Generated {} parameter combinations to test.", param_combinations.len());

    // --- Run Backtests in Parallel ---
    let results: Vec<BacktestResult> = param_combinations
        .par_iter()
        .filter_map(|params| {
            if combined_candles.len() < opt.history_size { return None; }
            run_single_backtest(params.clone(), &combined_candles, &opt)
        })
        .collect();

    println!("Finished running {} successful backtests.", results.len());

    // --- Find and display the best results for each metric ---
    if results.is_empty() {
        println!("\nNo suitable results found. Try adjusting parameter ranges or increasing the number of trades threshold.");
        return Ok(());
    }

    // 1. Best by Sharpe Ratio
    let best_sharpe = results.iter().max_by(|a, b| a.sharpe_ratio.partial_cmp(&b.sharpe_ratio).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(result) = best_sharpe {
        println!("\n--- Best Result (Optimized for Sharpe Ratio) ---");
        println!(
            "Parameters: EMA({}/{}), RSI({}), SL: {:.1}*ATR, TP: {:.1}*ATR, MinSL: {}, MaxSL: {}, KF(q:{}, r:{})",
            result.params.ema_fast, result.params.ema_slow, result.params.rsi_period,
            result.params.sl_atr_multiplier, result.params.tp_atr_multiplier,
            result.params.min_sl_pips, result.params.max_sl_pips,
            result.params.kf_process_noise, result.params.kf_measurement_noise
        );
        println!("Sharpe Ratio: {:.3} | Profit Factor: {:.2}", result.sharpe_ratio, result.profit_factor);
        println!("Final Balance: {:.2}", result.final_balance);
        println!("Net Profit: {:.2}", result.net_profit);
        println!("Max Drawdown: {:.2}%", result.max_drawdown);
        println!("Total Trades: {}", result.total_trades);
        println!("Win Rate: {:.2}%", result.win_rate);
    }

    // 2. Best by Profit Factor / Max Drawdown
    let best_pf_dd = results.iter().max_by(|a, b| {
        let val_a = if a.max_drawdown > 0.0 { a.profit_factor / a.max_drawdown } else { f64::INFINITY };
        let val_b = if b.max_drawdown > 0.0 { b.profit_factor / b.max_drawdown } else { f64::INFINITY };
        val_a.partial_cmp(&val_b).unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(result) = best_pf_dd {
        println!("\n--- Best Result (Optimized for Profit Factor / Max Drawdown) ---");
        println!(
            "Parameters: EMA({}/{}), RSI({}), SL: {:.1}*ATR, TP: {:.1}*ATR, MinSL: {}, MaxSL: {}, KF(q:{}, r:{})",
            result.params.ema_fast, result.params.ema_slow, result.params.rsi_period,
            result.params.sl_atr_multiplier, result.params.tp_atr_multiplier,
            result.params.min_sl_pips, result.params.max_sl_pips,
            result.params.kf_process_noise, result.params.kf_measurement_noise
        );
        println!("Sharpe Ratio: {:.3} | Profit Factor: {:.2}", result.sharpe_ratio, result.profit_factor);
        println!("Final Balance: {:.2}", result.final_balance);
        println!("Net Profit: {:.2}", result.net_profit);
        println!("Max Drawdown: {:.2}%", result.max_drawdown);
        println!("Total Trades: {}", result.total_trades);
        println!("Win Rate: {:.2}%", result.win_rate);
    }

    Ok(())
}