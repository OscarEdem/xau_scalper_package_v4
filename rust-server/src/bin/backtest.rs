use rayon::prelude::*;
use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, StructOpt)]
#[structopt(name = "backtest", about = "A backtester for the XAU/USD scalper strategy.")]
struct Opt {
    /// Path to the CSV file with historical data
    #[structopt(parse(from_os_str))]
    file: PathBuf,

    /// Number of historical bars to send in each evaluation request
    #[structopt(short, long, default_value = "80")]
    history_size: usize,

    /// Initial account balance
    #[structopt(short, long, default_value = "10000")]
    balance: f64,

    /// Risk per trade as a percentage of balance
    #[structopt(short, long, default_value = "1.0")]
    risk_percent: f64,
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
fn run_single_backtest(params: BacktestParams, candles: &[Candle], opt: &Opt) -> Option<BacktestResult> {
    // --- OPTIMIZATION: Pre-calculate all indicators ---
    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
    let highs: Vec<f64> = candles.iter().map(|c| c.high).collect();
    let lows: Vec<f64> = candles.iter().map(|c| c.low).collect();

    let ema_fast_values = xau_scalper_server::ema(&closes, params.ema_fast);
    let ema_slow_values = xau_scalper_server::ema(&closes, params.ema_slow);
    let rsi_values = xau_scalper_server::rsi(&closes, params.rsi_period);
    let atr_values = xau_scalper_server::atr(&highs, &lows, &closes, 14); // ATR period is often fixed

    let enriched_candles: Vec<EnrichedCandle> = candles.iter().enumerate().map(|(i, c)| EnrichedCandle {
        time: c.time.clone(),
        open: c.open,
        high: c.high,
        low: c.low,
        close: c.close,
        ema_fast: ema_fast_values[i],
        ema_slow: ema_slow_values[i],
        rsi: rsi_values[i],
        atr: atr_values[i],
    }).collect();
    // --- End of Optimization ---


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

    for i in opt.history_size..enriched_candles.len() {
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
            let prev_candle = &enriched_candles[i-1];
            let last_close = current_candle.close;
            let last_rsi = current_candle.rsi;

            let price_confirms_buy = last_close > current_candle.ema_slow;
            let price_confirms_sell = last_close < current_candle.ema_slow;

            let is_buy_signal = prev_candle.ema_fast <= prev_candle.ema_slow && current_candle.ema_fast > current_candle.ema_slow && price_confirms_buy && last_rsi < 80.0;
            let is_sell_signal = prev_candle.ema_fast >= prev_candle.ema_slow && current_candle.ema_fast < current_candle.ema_slow && price_confirms_sell && last_rsi > 20.0;

            if is_buy_signal || is_sell_signal {
                let entry_price = current_candle.open;
                let last_atr = current_candle.atr;

                let sl_pips = (last_atr * params.sl_atr_multiplier) / pip_size;
                let tp_pips = (last_atr * params.tp_atr_multiplier) / pip_size;

                if sl_pips <= 0.0 { continue; } // Avoid division by zero

                let risk_amount = balance * (opt.risk_percent / 100.0);
                let sl_points = sl_pips * pip_size;
                let lot_size = risk_amount / sl_points;

                let (direction, stop_loss, take_profit) = if is_buy_signal {
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

    let file = File::open(&opt.file)?;
    let mut rdr = csv::Reader::from_reader(file);
    let candles: Vec<Candle> = rdr.deserialize().collect::<Result<_, _>>()?;
    println!("Loaded {} candles from data file.", candles.len());

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

    // --- Run Backtests in Parallel ---
    let results: Vec<BacktestResult> = param_combinations
        .par_iter() // The magic of Rayon for parallel execution!
        .filter_map(|params| run_single_backtest(params.clone(), &candles, &opt))
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