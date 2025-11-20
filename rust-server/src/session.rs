use crate::{
    engines::{scalp::ScalpEngine, swing::SwingEngine},
    EvalRequest, EvalResponse, OpenPosition,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tracing::info;

const MAX_BUFFER_SIZE: usize = 200; // Store up to 200 recent candles per timeframe.

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
    h1_lows: VecDeque<f64>,

    // --- Engine Outputs & Positions ---
    open_scalp_positions: Vec<OpenPosition>,
    open_swing_positions: Vec<OpenPosition>,
    latest_scalp_signal: Option<EvalResponse>,
    latest_swing_signal: Option<EvalResponse>,
}

impl TradingSession {
    pub fn new(symbol: String, filter_scalp_by_swing: bool) -> Self {
        Self {
            symbol,
            filter_scalp_by_swing,
            last_evaluation_timestamp: 0,
            swing_trend: TrendDirection::Sideways,
            m1_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m5_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m5_highs: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m5_lows: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            m30_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_closes: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_highs: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            h1_lows: VecDeque::with_capacity(MAX_BUFFER_SIZE),
            open_scalp_positions: Vec::new(),
            open_swing_positions: Vec::new(),
            latest_scalp_signal: None,
            latest_swing_signal: None,
        }
    }

    /// The main entry point for processing new data for this session.
    /// It updates internal buffers and then runs both trading engines.
    pub fn on_data(&mut self, req: &EvalRequest) {
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
        self.h1_lows = req.h1_lows.clone().unwrap_or_default().into();
        self.last_evaluation_timestamp = req.last_m1_timestamp;

        // 2. Run the Swing Engine first to establish the higher-timeframe context.
        let swing_req = self.build_engine_request("swing");
        let swing_signal = SwingEngine::evaluate(&swing_req);
        info!(symbol = %self.symbol, signal_id = %swing_signal.signal_id, entry_type = %swing_signal.entry_type, "Swing engine evaluated.");

        // Update the session's swing trend based on the new signal.
        self.swing_trend = TrendDirection::from(swing_signal.entry_type.as_str());
        self.latest_swing_signal = Some(swing_signal);

        // 3. Run the Scalp Engine.
        let scalp_req = self.build_engine_request_with_params("scalp", req);
        let mut scalp_signal = ScalpEngine::evaluate(&scalp_req);
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

        // 5. Store the final scalp signal.
        self.latest_scalp_signal = Some(scalp_signal);

        // Placeholder for position management logic
        self.manage_positions();
    }

    /// Builds an `EvalRequest` for a specific engine using the session's data.
    fn build_engine_request(&self, mode: &str) -> EvalRequest {
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
            h1_lows: Some(self.h1_lows.iter().cloned().collect()),
            open_positions: Some(if mode == "scalp" { self.open_scalp_positions.clone() } else { self.open_swing_positions.clone() }),
            mode: mode.to_string(),
            current_price: *self.m1_closes.back().unwrap_or(&0.0), // Safely get the last price or default to 0.0
            last_m1_timestamp: self.last_evaluation_timestamp,
            ..Default::default() // Fills in optional params
        }
    }

    /// Builds an `EvalRequest` for a specific engine, propagating optional parameters from the original request.
    fn build_engine_request_with_params(&self, mode: &str, original_req: &EvalRequest) -> EvalRequest {
        let mut engine_req = self.build_engine_request(mode);
        // Propagate all optional parameters from the original request
        engine_req.spread_limit_points = original_req.spread_limit_points;
        engine_req.spread_points = original_req.spread_points;
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