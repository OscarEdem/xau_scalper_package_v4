use rayon::prelude::*;
use serde::Deserialize;
use std::error::Error;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, Deserialize, Clone)] // Deserialize by position
pub struct Candle {
    time: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: u32,
}

/// Holds a candle and its pre-calculated indicator values.
#[derive(Debug, Clone)]
struct EnrichedCandle {
    time: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    ema_fast: f64,
    ema_slow: f64,
    rsi: f64,
    atr: f64,
}

/// A new struct to hold the M1 candle and its corresponding M5 indicators.
#[derive(Debug, Clone)]
struct CombinedCandle {
    m1: EnrichedCandle,
    m5_ema_slow: f64,
    m5_ema_fast: f64,
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
    // The combined_candles are already enriched, so we can use them directly.
    // The `EnrichedCandle` inside `CombinedCandle` is for the M1 timeframe.
    let enriched_candles: Vec<EnrichedCandle> = combined_candles.iter().map(|c| c.m1.clone()).collect();

    let mut balance = opt.balance;
    let mut active_trade: Option<Trade> = None;
    let mut wins = 0;
    let mut losses = 0;
    let mut pnl_history = Vec::new();
    let mut peak_balance = balance;
    let mut gross_profit = 0.0;
    let mut gross_loss = 0.0;
    let mut max_drawdown: f64 = 0.0;
    let pip_size = 0.01;
    let trailing_stop_atr_multiplier = params.sl_atr_multiplier; // Use the same multiplier for trailing

    for i in opt.history_size..combined_candles.len() {
        let current_candle = &enriched_candles[i];

        // --- Check if active trade should be closed ---
        if let Some(trade) = active_trade.take() {
            let (pnl, _reason) = match trade.direction {
                TradeDirection::Long => { // --- MODIFIED: Logic for Long trade exit ---
                    // Calculate the new trailing stop
                    let high_since_entry = enriched_candles[trade.entry_candle_index..=i].iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max);
                    let new_stop_loss = (high_since_entry - current_candle.atr * trailing_stop_atr_multiplier).max(trade.stop_loss);

                    if current_candle.low <= new_stop_loss {
                        (-(trade.entry_price - new_stop_loss) * trade.lot_size, "Trailing Stop")
                    } else if current_candle.high >= trade.take_profit {
                        ((trade.take_profit - trade.entry_price) * trade.lot_size, "Take Profit")
                    } else {
                        // Trade still active, update the trade with the new stop loss
                        active_trade = Some(Trade { stop_loss: new_stop_loss, ..trade });
                        (0.0, "")
                    }
                }
                TradeDirection::Short => { // --- MODIFIED: Logic for Short trade exit ---
                    // Calculate the new trailing stop
                    let low_since_entry = enriched_candles[trade.entry_candle_index..=i].iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
                    let new_stop_loss = (low_since_entry + current_candle.atr * trailing_stop_atr_multiplier).min(trade.stop_loss);

                    if current_candle.high >= new_stop_loss {
                        (-(new_stop_loss - trade.entry_price) * trade.lot_size, "Trailing Stop")
                    } else if current_candle.low <= trade.take_profit {
                        ((trade.entry_price - trade.take_profit) * trade.lot_size, "Take Profit")
                    } else {
                        // Trade still active, update the trade with the new stop loss
                        active_trade = Some(Trade { stop_loss: new_stop_loss, ..trade });
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
            // --- OPTIMIZATION: Use pre-calculated values for entry logic ---
            let prev_m1_candle = &enriched_candles[i-1];
            let current_m1_candle = &enriched_candles[i];
            let combined_candle = &combined_candles[i];

            // --- MULTI-TIMEFRAME LOGIC ---
            // Use M5 for trend direction
            let is_uptrend_m5 = combined_candle.m1.close > combined_candle.m5_ema_slow;
            let is_downtrend_m5 = combined_candle.m1.close < combined_candle.m5_ema_slow;

            // Use M1 for entry signal, confirming with M5 trend
            let is_m1_buy_cross = prev_m1_candle.ema_fast <= prev_m1_candle.ema_slow && current_m1_candle.ema_fast > current_m1_candle.ema_slow;
            let is_m1_sell_cross = prev_m1_candle.ema_fast >= prev_m1_candle.ema_slow && current_m1_candle.ema_fast < current_m1_candle.ema_slow;

            let is_final_buy_signal = is_m1_buy_cross && is_uptrend_m5 && current_m1_candle.rsi < 80.0;
            let is_final_sell_signal = is_m1_sell_cross && is_downtrend_m5 && current_m1_candle.rsi > 20.0;

            if is_final_buy_signal || is_final_sell_signal {
                let entry_price = current_candle.open;
                let last_atr = current_candle.atr;

                let sl_pips = (last_atr * params.sl_atr_multiplier) / pip_size;
                let tp_pips = (last_atr * params.tp_atr_multiplier) / pip_size;

                if sl_pips <= 0.0 { continue; } // Avoid division by zero

                let risk_amount = balance * (opt.risk_percent / 100.0);
                let sl_points = sl_pips * pip_size;
                let lot_size = risk_amount / sl_points;

                let (direction, stop_loss, take_profit) = if is_final_buy_signal {
                    (TradeDirection::Long, entry_price - sl_points, entry_price + tp_pips * pip_size)
                } else {
                    (TradeDirection::Short, entry_price + sl_points, entry_price - tp_pips * pip_size)
                };

                active_trade = Some(Trade { direction, entry_price, stop_loss, take_profit, lot_size, entry_candle_index: i });
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

    // --- Pre-calculate indicators for both timeframes ---
    // M1 Indicators
    let m1_closes: Vec<f64> = m1_candles.iter().map(|c| c.close).collect();
    let m1_highs: Vec<f64> = m1_candles.iter().map(|c| c.high).collect();
    let m1_lows: Vec<f64> = m1_candles.iter().map(|c| c.low).collect();

    // M5 Indicators
    let m5_closes: Vec<f64> = m5_candles.iter().map(|c| c.close).collect();
    let m5_highs: Vec<f64> = m5_candles.iter().map(|c| c.high).collect();
    let m5_lows: Vec<f64> = m5_candles.iter().map(|c| c.low).collect();

    // --- Create a lookup map for M5 candles for efficient access ---
    let m5_candle_map: HashMap<String, Candle> = m5_candles.into_iter().map(|c| (c.time.clone(), c)).collect();

    // --- Combine M1 and M5 data ---
    // This part is tricky. We need to align M1 data with the correct M5 bar.
    // An M5 bar at 10:05 covers the M1 bars from 10:01 to 10:05.
    // We will find the corresponding M5 bar for each M1 bar.
    let mut combined_candles: Vec<CombinedCandle> = Vec::new();
    let mut m5_idx = 0;

    // We will calculate indicators over the whole dataset once, which is much faster.
    // The parameter ranges are defined below, so we'll calculate for the max range needed.
    let max_slow_ema = 50;
    let max_rsi = 20;
    let atr_period = 14;

    let m1_ema_fast_full = xau_scalper_server::ema(&m1_closes, 5); // Example, will be replaced in loop
    let m1_ema_slow_full = xau_scalper_server::ema(&m1_closes, max_slow_ema);
    let m1_rsi_full = xau_scalper_server::rsi(&m1_closes, max_rsi);
    let m1_atr_full = xau_scalper_server::atr(&m1_highs, &m1_lows, &m1_closes, atr_period);

    let m5_ema_fast_full = xau_scalper_server::ema(&m5_closes, 5); // Example
    let m5_ema_slow_full = xau_scalper_server::ema(&m5_closes, max_slow_ema);

    // Create a map for M5 indicators for quick lookup
    let m5_indicator_map: HashMap<_, _> = m5_candle_map.keys().enumerate().map(|(i, time)| {
        (time.clone(), (m5_ema_fast_full[i], m5_ema_slow_full[i]))
    }).collect();

    // This approach is simplified. A proper alignment would require parsing timestamps.
    // For this example, we assume the data is somewhat aligned and we can find the M5 bar.
    // A robust solution would parse times and round the M1 time down to the nearest 5 minutes.
    println!("Combining M1 and M5 data... This is a simplified alignment.");

    // --- Define Parameter Ranges for Optimization ---
    let ema_fast_periods = (5..=15).step_by(2);
    let ema_slow_periods = (20..=50).step_by(5);
    let rsi_periods = (10..=20).step_by(2);
    let sl_multipliers = vec![1.0, 1.5, 2.0];
    let tp_multipliers = vec![1.5, 2.0, 3.0];

    let mut param_combinations = Vec::new();
    for fast_p in ema_fast_periods {
        for slow_p in ema_slow_periods.clone() {
            if fast_p >= slow_p { continue; }
            for rsi_p in rsi_periods.clone() {
                for sl_m in sl_multipliers.iter() {
                    for tp_m in tp_multipliers.iter() {
                        param_combinations.push(BacktestParams {
                            ema_fast: fast_p, ema_slow: slow_p, rsi_period: rsi_p,
                            sl_atr_multiplier: *sl_m, tp_atr_multiplier: *tp_m
                        });
                    }
                }
            }
        }
    }
    println!("Generated {} parameter combinations to test.", param_combinations.len());

    // --- Pre-calculate all indicator variations to avoid re-computation inside the loop ---
    let mut m1_indicator_cache = HashMap::new();
    for params in &param_combinations {
        let key = (params.ema_fast, params.ema_slow, params.rsi_period);
        if !m1_indicator_cache.contains_key(&key) {
            let ema_fast = xau_scalper_server::ema(&m1_closes, params.ema_fast);
            let ema_slow = xau_scalper_server::ema(&m1_closes, params.ema_slow);
            let rsi = xau_scalper_server::rsi(&m1_closes, params.rsi_period);
            m1_indicator_cache.insert(key, (ema_fast, ema_slow, rsi));
        }
    }
    println!("Pre-calculated all indicator variations.");

    // --- Run Backtests in Parallel ---
    let results: Vec<BacktestResult> = param_combinations
        .par_iter() // The magic of Rayon for parallel execution!
        .filter_map(|params| {
            // --- Assemble the specific combined data for this parameter set ---
            let (ema_fast_values, ema_slow_values, rsi_values) = m1_indicator_cache.get(&(params.ema_fast, params.ema_slow, params.rsi_period)).unwrap();

            let combined_candles: Vec<CombinedCandle> = m1_candles.iter().enumerate().filter_map(|(i, m1_candle)| {
                // Simplified time alignment: find M5 bar ending at or just after M1 bar
                let m5_time_key = &m1_candle.time; // This assumes M1 and M5 times can be matched.
                m5_indicator_map.get(m5_time_key).map(|(m5_fast, m5_slow)| CombinedCandle {
                    m1: EnrichedCandle {
                        time: m1_candle.time.clone(), open: m1_candle.open, high: m1_candle.high, low: m1_candle.low, close: m1_candle.close,
                        ema_fast: ema_fast_values[i], ema_slow: ema_slow_values[i], rsi: rsi_values[i], atr: m1_atr_full[i],
                    },
                    m5_ema_fast: *m5_fast,
                    m5_ema_slow: *m5_slow,
                })
            }).collect();

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
            "Parameters: EMA({}/{}), RSI({}), SL: {:.1}*ATR, TP: {:.1}*ATR",
            result.params.ema_fast, result.params.ema_slow, result.params.rsi_period,
            result.params.sl_atr_multiplier, result.params.tp_atr_multiplier
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
            "Parameters: EMA({}/{}), RSI({}), SL: {:.1}*ATR, TP: {:.1}*ATR",
            result.params.ema_fast, result.params.ema_slow, result.params.rsi_period,
            result.params.sl_atr_multiplier, result.params.tp_atr_multiplier
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