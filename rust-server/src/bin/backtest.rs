use rayon::prelude::*;
use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::path::PathBuf;
use structopt::StructOpt;
use xau_scalper_server::{EvalRequest, EvalResponse};
use xau_scalper_server::strategy::evaluate_strategy;

#[derive(Debug, Deserialize, Clone)]
pub struct Candle {
    time: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: u32,
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
}

/// Runs a single backtest with a given set of parameters.
fn run_single_backtest(params: BacktestParams, candles: &[Candle], opt: &Opt) -> Option<BacktestResult> {
    let mut balance = opt.balance;
    let mut active_trade: Option<Trade> = None;
    let mut wins = 0;
    let mut losses = 0;
    let mut pnl_history = Vec::new();
    let mut peak_balance = balance;
    let mut max_drawdown: f64 = 0.0;
    let pip_size = 0.01;

    for i in opt.history_size..candles.len() {
        let current_candle = &candles[i];

        // --- Check if active trade should be closed ---
        if let Some(trade) = active_trade.take() {
            let (pnl, _reason) = match trade.direction {
                TradeDirection::Long => {
                    if current_candle.low <= trade.stop_loss {
                        (-(trade.entry_price - trade.stop_loss) * trade.lot_size, "Stop Loss")
                    } else if current_candle.high >= trade.take_profit {
                        ((trade.take_profit - trade.entry_price) * trade.lot_size, "Take Profit")
                    } else {
                        (0.0, "") // Trade still active
                    }
                }
                TradeDirection::Short => {
                    if current_candle.high >= trade.stop_loss {
                        (-(trade.stop_loss - trade.entry_price) * trade.lot_size, "Stop Loss")
                    } else if current_candle.low <= trade.take_profit {
                        ((trade.entry_price - trade.take_profit) * trade.lot_size, "Take Profit")
                    } else {
                        (0.0, "") // Trade still active
                    }
                }
            };

            if pnl != 0.0 {
                balance += pnl;
                pnl_history.push(pnl);

                peak_balance = peak_balance.max(balance);
                let drawdown = (peak_balance - balance) / peak_balance;
                max_drawdown = max_drawdown.max(drawdown);

                if pnl > 0.0 { wins += 1; } else { losses += 1; }
                continue;
            } else {
                active_trade = Some(trade);
            }
        }

        // --- Check for new trade signals ---
        if active_trade.is_none() {
            let history_slice = &candles[i - opt.history_size..=i];
            let req = EvalRequest {
                symbol: "XAUUSD".into(),
                timeframe: "M1".into(),
                closes: history_slice.iter().map(|c| c.close).collect(),
                highs: history_slice.iter().map(|c| c.high).collect(),
                lows: history_slice.iter().map(|c| c.low).collect(),
                rsi_period: Some(params.rsi_period),
                ema_fast: Some(params.ema_fast),
                ema_slow: Some(params.ema_slow),
                atr_period: None,
                tp_pips: None,
                sl_pips: None,
                sl_atr_multiplier: Some(params.sl_atr_multiplier),
                tp_atr_multiplier: Some(params.tp_atr_multiplier),
            };

            let res: EvalResponse = evaluate_strategy(&req);

            if res.action_advice == "buy" || res.action_advice == "sell" {
                let entry_price = current_candle.open;
                let sl_pips = res.sl_pips;
                let tp_pips = res.tp_pips;

                if sl_pips <= 0.0 { continue; } // Avoid division by zero

                let risk_amount = balance * (opt.risk_percent / 100.0);
                let sl_points = sl_pips * pip_size;
                let lot_size = risk_amount / sl_points;

                let (direction, stop_loss, take_profit) = if res.action_advice == "buy" {
                    (TradeDirection::Long, entry_price - sl_points, entry_price + tp_pips * pip_size)
                } else {
                    (TradeDirection::Short, entry_price + sl_points, entry_price - tp_pips * pip_size)
                };

                active_trade = Some(Trade { direction, entry_price, stop_loss, take_profit, lot_size });
            }
        }
    }

    let total_trades = wins + losses;
    if total_trades < 10 { return None; } // Ignore results with too few trades

    let win_rate = (wins as f64 / total_trades as f64) * 100.0;
    let net_profit = balance - opt.balance;

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

    // --- Find the Best Result (e.g., by Sharpe Ratio) ---
    let best_result = results.into_iter().max_by(|a, b| a.sharpe_ratio.partial_cmp(&b.sharpe_ratio).unwrap_or(std::cmp::Ordering::Equal));

    if let Some(result) = best_result {
        println!("\n--- Best Result (Optimized for Sharpe Ratio) ---");
        println!(
            "Parameters: EMA({}/{}), RSI({}), SL: {:.1}*ATR, TP: {:.1}*ATR",
            result.params.ema_fast, result.params.ema_slow, result.params.rsi_period,
            result.params.sl_atr_multiplier, result.params.tp_atr_multiplier
        );
        println!("Sharpe Ratio: {:.3}", result.sharpe_ratio);
        println!("Final Balance: {:.2}", result.final_balance);
        println!("Net Profit: {:.2}", result.net_profit);
        println!("Max Drawdown: {:.2}%", result.max_drawdown);
        println!("Total Trades: {}", result.total_trades);
        println!("Win Rate: {:.2}%", result.win_rate);
    } else {
        println!("\nNo suitable results found. Try adjusting parameter ranges or increasing the number of trades threshold.");
    }
    
    Ok(())
}