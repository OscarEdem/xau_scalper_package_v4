use crate::{
    engines::{scalp::ScalpEngine, swing::SwingEngine},
    engines::predictor_cache::PredictorCache,
    EvalRequest, EvalResponse, OpenPosition, TradeLog,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tracing::info;

const MAX_BUFFER_SIZE: usize = 500; // Store up to 200 recent candles per timeframe.

/// Represents the dominant trend direction determined by the SwingEngine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrendDirection {
    Up,
    Down,
    Sideways,
}

impl From<&str> for TrendDirection {
    fn from(s: &str) -> Self {
        match s {
            "long" => TrendDirection::Up,
            "short" => TrendDirection::Down,
            _ => TrendDirection::Sideways,
        }
    }
}

/// Manages the state for a single trading symbol (e.g., "XAUUSD").
/// A TradingSession is created for each symbol the system trades. It holds all
/// necessary data buffers, open positions, and latest signals for that symbol.
#[derive(Debug)]
pub struct TradingSession {
    pub symbol: String,
    /// Configuration flag for cross-engine filtering.
    /// If true, scalp signals will only be considered if they align with the swing trend.
    pub filter_scalp_by_swing: bool,
    /// The predictor model to use for this session (e.g., "gbm", "lstm").
    pub predictor_model: String,

    // --- State Data ---
    last_evaluation_timestamp: i64,
    swing_trend: TrendDirection,

    // --- Data Buffers ---
    // Using VecDeque for efficient push_front/pop_back operations.
    m1_closes: VecDeque<f64>,
    m5_closes: VecDeque<f64>,
    m5_highs: VecDeque<f64>,
    m5_lows: VecDeque<f64>,
    m30_closes: VecDeque<f64>,
    h1_closes: VecDeque<f64>,
    h1_highs: VecDeque<f64>,
    h1_opens: VecDeque<f64>,
    h1_lows: VecDeque<f64>,
    h4_closes: VecDeque<f64>,
    h4_highs: VecDeque<f64>,
    h4_lows: VecDeque<f64>,
    d1_opens: VecDeque<f64>,
    d1_closes: VecDeque<f64>,

    // --- Engine Outputs & Positions ---
    open_scalp_positions: Vec<OpenPosition>,
    open_swing_positions: Vec<OpenPosition>,
    latest_scalp_signal: Option<EvalResponse>,
    latest_swing_signal: Option<EvalResponse>,
    trade_logs: VecDeque<TradeLog>,

    // NEW: Anti-Spam State
    last_notified_signal_id: Option<String>,
    last_notified_direction: String, // "long", "short", "none"
    last_notified_time: i64,         // Unix timestamp
    last_notified_price: f64,
}

impl TradingSession {
    pub fn new(symbol: String, filter_scalp_by_swing: bool) -> Self {
        Self {
            symbol,
            filter_scalp_by_swing,
            predictor_model: "gbm".to_string(), // Default to GBM
            last_evaluation_timestamp: 0,
            swing_trend: TrendDirection::Sideways,
            m1_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m5_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m5_highs: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m5_lows: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m30_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_highs: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_opens: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_lows: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h4_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h4_highs: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h4_lows: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            d1_opens: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            d1_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            open_scalp_positions: Vec::new(),
            open_swing_positions: Vec::new(),
            latest_scalp_signal: None,
            latest_swing_signal: None,
            trade_logs: VecDeque::with_capacity(500), // Store last 500 trades per symbol

            // Initialize spam filters
            last_notified_signal_id: None,
            last_notified_direction: "none".to_string(),
            last_notified_time: 0,
            last_notified_price: 0.0,
        }
    }

    /// The main entry point for processing new data for this session.
    /// It updates internal buffers and then runs both trading engines.
    pub fn on_data(&mut self, req: EvalRequest, predictor_cache: &PredictorCache) {
        info!(symbol = %self.symbol, "Processing new data for session.");

        // 1. Update data buffers with the latest candle data from the request.
        // In a real system, you'd likely receive individual new candles and push them.
        // Here, we're just replacing the buffers for simplicity.
        self.m1_closes = req.closes.iter().cloned().collect();
        self.m5_closes = req.m5_closes.iter().cloned().collect();
        self.m5_highs = req.m5_highs.iter().cloned().collect();
        self.m5_lows = req.m5_lows.iter().cloned().collect();
        self.m30_closes = req.m30_closes.iter().cloned().collect();
        self.h1_closes = req.h1_closes.clone().unwrap_or_default().into();
        self.h1_highs = req.h1_highs.clone().unwrap_or_default().into();
        self.h1_opens = req.h1_opens.clone().unwrap_or_default().into();
        self.h1_lows = req.h1_lows.clone().unwrap_or_default().into();
        self.h4_closes = req.h4_closes.clone().unwrap_or_default().into();
        self.h4_highs = req.h4_highs.clone().unwrap_or_default().into();
        self.h4_lows = req.h4_lows.clone().unwrap_or_default().into();
        self.d1_opens = req.d1_opens.clone().unwrap_or_default().into();
        self.d1_closes = req.d1_closes.clone().unwrap_or_default().into();
        self.last_evaluation_timestamp = req.last_m1_timestamp;

        // 2. Run the Swing Engine first to establish the higher-timeframe context.
        let swing_req = self.build_engine_request("swing", &req);
        let swing_signal = SwingEngine::evaluate(&swing_req, predictor_cache);
        info!(symbol = %self.symbol, signal_id = %swing_signal.signal_id, entry_type = %swing_signal.entry_type, "Swing engine evaluated.");

        // Update the session's swing trend based on the new signal.
        self.swing_trend = TrendDirection::from(swing_signal.entry_type.as_str());
        self.latest_swing_signal = Some(swing_signal);

        // 3. Run the Scalp Engine.
        let scalp_req = self.build_engine_request_with_params("scalp", &req);
        let mut scalp_signal = ScalpEngine::evaluate(&scalp_req, predictor_cache);
        info!(symbol = %self.symbol, signal_id = %scalp_signal.signal_id, entry_type = %scalp_signal.entry_type, "Scalp engine evaluated.");

        // 4. Apply cross-engine filtering if enabled.
        // This is where the scalp signal can be suppressed if it conflicts with the swing trend.
        if self.filter_scalp_by_swing {
            let scalp_trend = TrendDirection::from(scalp_signal.entry_type.as_str());
            if self.swing_trend != TrendDirection::Sideways && scalp_trend != self.swing_trend {
                info!(
                    symbol = %self.symbol,
                    "Filtering scalp signal. Direction ({:?}) conflicts with swing trend ({:?}).",
                    scalp_trend, self.swing_trend
                );
                // Invalidate the scalp signal
                scalp_signal = EvalResponse {
                    reason: format!("Filtered: Scalp signal conflicts with swing trend ({:?})", self.swing_trend),
                    ..Default::default()
                };
            }
        }

        // 5. FILTERING LOGIC & FINAL STORAGE
        let current_time = req.last_m1_timestamp;
        let is_valid_signal = scalp_signal.entry_type != "none";

        if is_valid_signal {
            let direction_changed = scalp_signal.entry_type != self.last_notified_direction;
            let time_diff = current_time - self.last_notified_time;
            
            // Cooldown: 5 minutes (300 seconds)
            let cooldown_passed = time_diff > 300;

            // Check if we are already in a trade for this direction
            let already_in_trade = self.open_scalp_positions.iter().any(|p| 
                (p.direction == "buy" && scalp_signal.entry_type == "long") ||
                (p.direction == "sell" && scalp_signal.entry_type == "short")
            );
            
            // --- Pyramiding Logic ---
            // Calculate percentage distance from the LAST signal price
            let price_delta_pct = if self.last_notified_price > 0.0 {
                (scalp_signal.entry_price - self.last_notified_price) / self.last_notified_price
            } else {
                0.0
            };

            // Allow a new signal if price has moved significantly in our favor.
            let pyramiding_threshold = 0.0015; // 0.15% (e.g., ~$3 on Gold)
            let is_pyramiding_breakout = !direction_changed 
                && match scalp_signal.entry_type.as_str() {
                    "long" => price_delta_pct > pyramiding_threshold,  // Price went UP significantly
                    "short" => price_delta_pct < -pyramiding_threshold, // Price went DOWN significantly
                    _ => false,
                };

            // --- Updated Decision Matrix ---
            let should_notify =
                // Priority 1: Trend Reversal (Always notify)
                direction_changed 
                // Priority 2: Standard New Entry (Not in trade, cooldown passed)
                || (!direction_changed && cooldown_passed && !already_in_trade)
                // Priority 3: Pyramiding Exception (In trade, but strong breakout)
                || (already_in_trade && is_pyramiding_breakout);

            if should_notify {
                // Tag the signal if it's a pyramid entry
                if is_pyramiding_breakout {
                    scalp_signal.classification = "scalp_pyramid".to_string();
                    scalp_signal.reason = format!("Breakout_Add: {}", scalp_signal.reason);
                }

                // UPDATE STATE
                self.last_notified_signal_id = Some(scalp_signal.signal_id.clone());
                self.last_notified_direction = scalp_signal.entry_type.clone();
                self.last_notified_time = current_time;
                self.last_notified_price = scalp_signal.entry_price;
                
                // Allow the signal to pass through
                self.latest_scalp_signal = Some(scalp_signal);
            } else {
                // SUPPRESS THE SIGNAL
                // We set it to None so the API doesn't send a push notification.
                // The original signal is logged above, but the final state is suppressed.
                info!(
                    symbol = %self.symbol,
                    reason = "Suppressed by anti-spam filter",
                    time_since_last = time_diff,
                    already_in_trade = already_in_trade,
                    "Scalp signal suppressed."
                );
                self.latest_scalp_signal = None; 
            }
        } else {
            // If the engine returns "none", there's no signal to store or suppress.
            self.latest_scalp_signal = Some(scalp_signal);
        }

        // Placeholder for position management logic
        self.manage_positions();
    }

    /// Builds an `EvalRequest` for a specific engine using the session's data.
    fn build_engine_request(&self, mode: &str, original_req: &EvalRequest) -> EvalRequest {
        // This clones the data from the session buffers.
        // For very high performance, you might use `Arc`s to avoid deep copies.
        EvalRequest {
            symbol: self.symbol.clone(),
            timeframe: "M1".to_string(), // Base timeframe
            closes: self.m1_closes.iter().cloned().collect(),
            highs: vec![], // Populate with real data if needed by engines
            opens: vec![], // Populate with real data if needed by engines
            lows: vec![],  // Populate with real data if needed by engines
            volumes: vec![], // Populate with real data if needed by engines
            m5_closes: self.m5_closes.iter().cloned().collect(),
            m5_highs: self.m5_highs.iter().cloned().collect(),
            m5_lows: self.m5_lows.iter().cloned().collect(),
            m30_closes: self.m30_closes.iter().cloned().collect(),
            h1_closes: Some(self.h1_closes.iter().cloned().collect()),
            h1_highs: Some(self.h1_highs.iter().cloned().collect()),
            h1_opens: Some(self.h1_opens.iter().cloned().collect()),
            h1_lows: Some(self.h1_lows.iter().cloned().collect()),
            h4_closes: Some(self.h4_closes.iter().cloned().collect()),
            h4_highs: Some(self.h4_highs.iter().cloned().collect()),
            h4_lows: Some(self.h4_lows.iter().cloned().collect()),
            d1_opens: Some(self.d1_opens.iter().cloned().collect()),
            d1_closes: Some(self.d1_closes.iter().cloned().collect()),
            open_positions: Some(if mode == "scalp" { self.open_scalp_positions.clone() } else { self.open_swing_positions.clone() }),
            mode: mode.to_string(),
            current_price: original_req.current_price, // Use the live price from the original request
            last_m1_timestamp: self.last_evaluation_timestamp,
            ..Default::default() // Fills in optional params
        }
    }

    /// Builds an `EvalRequest` for a specific engine, propagating optional parameters from the original request.
    fn build_engine_request_with_params(&self, mode: &str, original_req: &EvalRequest) -> EvalRequest {
        let mut engine_req = self.build_engine_request(mode, original_req);
        // Propagate all optional parameters from the original request
        engine_req.spread_limit_points = original_req.spread_limit_points;
        engine_req.spread_points = original_req.spread_points;
        engine_req.kf_process_noise = original_req.kf_process_noise;
        engine_req.kf_measurement_noise = original_req.kf_measurement_noise;
        engine_req.upcoming_events = original_req.upcoming_events.clone();
        engine_req.predictor_model = Some(self.predictor_model.clone());
        // ... propagate other optional params as needed ...
        engine_req
    }

    /// Placeholder for logic to manage open trades (e.g., trailing stops).
    fn manage_positions(&mut self) {
        // Here you would iterate through `self.open_scalp_positions` and `self.open_swing_positions`
        // and decide if any actions (like closing or updating SL/TP) are needed based on new data.
        info!(symbol = %self.symbol, "Checking open positions (scalp: {}, swing: {}).", self.open_scalp_positions.len(), self.open_swing_positions.len());
    }

    pub fn get_latest_signals(&self) -> (Option<EvalResponse>, Option<EvalResponse>) {
        (self.latest_scalp_signal.clone(), self.latest_swing_signal.clone())
    }

    /// Gets the signal ID of the latest scalp signal, if any.
    pub fn get_latest_scalp_signal_id(&self) -> Option<String> {
        self.latest_scalp_signal.as_ref().map(|s| s.signal_id.clone())
    }

    /// Gets the signal ID of the latest swing signal, if any.
    pub fn get_latest_swing_signal_id(&self) -> Option<String> {
        self.latest_swing_signal.as_ref().map(|s| s.signal_id.clone())
    }

    /// Returns the timestamp of the last evaluation.
    pub fn get_last_eval_timestamp(&self) -> i64 {
        self.last_evaluation_timestamp
    }

    /// Sets the signals to None, used by the cleanup task.
    pub fn invalidate_signals(&mut self) {
        self.latest_scalp_signal = None;
        self.latest_swing_signal = None;
    }

    /// Adds a new trade log to this session.
    pub fn add_trade_log(&mut self, log: TradeLog) {
        self.trade_logs.push_front(log);
    }

    /// Returns a clone of the trade logs for this session.
    pub fn get_trade_logs(&self) -> VecDeque<TradeLog> {
        self.trade_logs.clone()
    }

    /// Clears all trade logs for this session.
    pub fn clear_trade_logs(&mut self) {
        self.trade_logs.clear();
    }
}

/// Manages all active TradingSessions, keyed by symbol.
/// This is the main stateful component of the application.
#[derive(Clone, Default)]
pub struct SessionManager {
    pub sessions: Arc<Mutex<HashMap<String, TradingSession>>>,
}

impl SessionManager {
    pub fn get_or_create_session(&self, symbol: &str, filter_scalp_by_swing: bool) -> std::sync::MutexGuard<'_, HashMap<String, TradingSession>> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.entry(symbol.to_string()).or_insert_with(|| {
            info!("Creating new trading session for symbol: {}", symbol);
            TradingSession::new(symbol.to_string(), filter_scalp_by_swing)
        });
        sessions
    }
}