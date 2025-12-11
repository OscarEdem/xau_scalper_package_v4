use crate::{
    engines::{scalp::ScalpEngine, swing::SwingEngine},
    engines::predictor_cache::PredictorCache,
    EvalRequest, EvalResponse,
};
use dashmap::DashMap;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tracing::info;

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

/// Handles trade execution logic: Cooldowns, Anti-Spam, Pyramiding, and Reversals.
/// Decouples the "decision to notify" from the "session state".
#[derive(Debug, Clone)]
pub struct ExecutionManager {
    pub last_notified_signal_id: Option<String>,
    pub last_notified_direction: String, // "long", "short", "none"
    pub last_notified_time: i64,         // Unix timestamp
    pub last_notified_price: f64,
    pub pyramiding_enabled: bool,
}

impl ExecutionManager {
    pub fn new(pyramiding_enabled: bool) -> Self {
        Self {
            last_notified_signal_id: None,
            last_notified_direction: "none".to_string(),
            last_notified_time: 0,
            last_notified_price: 0.0,
            pyramiding_enabled,
        }
    }

    /// Evaluates if a signal should be broadcasted based on execution rules.
    /// Returns true if the signal is valid for notification.
    pub fn evaluate_execution(&mut self, signal: &mut EvalResponse, current_time: i64) -> bool {
        if signal.entry_type == "none" {
            return false;
        }

        // Check if this is the same signal ID we are already tracking
        if self.last_notified_signal_id.as_ref() == Some(&signal.signal_id) {
            // Keep it active, but don't re-notify
            return false;
        }

        let direction_changed = signal.entry_type != self.last_notified_direction;
        let time_diff = current_time - self.last_notified_time;
        let conviction = signal.conviction_score.unwrap_or(0.0);
        
        // Cooldown: 5 minutes (300 seconds)
        let cooldown_passed = time_diff > 300;

        // --- Pyramiding Logic ---
        let price_delta_pct = if self.last_notified_price > 0.0 {
            (signal.entry_price - self.last_notified_price) / self.last_notified_price
        } else { 0.0 };

        let pyramiding_threshold = 0.0015; // 0.15%
        let is_pyramiding_breakout = self.pyramiding_enabled && !direction_changed 
            && match signal.entry_type.as_str() {
                "long" => price_delta_pct > pyramiding_threshold,
                "short" => price_delta_pct < -pyramiding_threshold,
                _ => false,
            };

        // --- Reversal Logic ---
        let is_rapid_reversal = direction_changed && time_diff < 180;
        let is_valid_reversal = direction_changed && (!is_rapid_reversal || conviction > 60.0);

        // --- Decision ---
        is_valid_reversal || (!direction_changed && cooldown_passed) || is_pyramiding_breakout
    }

    pub fn update_state(&mut self, signal: &EvalResponse, current_time: i64) {
        self.last_notified_signal_id = Some(signal.signal_id.clone());
        self.last_notified_direction = signal.entry_type.clone();
        self.last_notified_time = current_time;
        self.last_notified_price = signal.entry_price;
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
    pub max_buffer_size: usize,

    // --- State Data ---
    last_evaluation_timestamp: i64,
    last_m1_timestamp: i64,
    last_m5_timestamp: i64,
    last_m30_timestamp: i64,
    last_h1_timestamp: i64,
    last_h4_timestamp: i64,
    last_d1_timestamp: i64,
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
    latest_scalp_signal: Option<EvalResponse>,
    latest_swing_signal: Option<EvalResponse>,

    pub execution: ExecutionManager,
    pub swing_execution: ExecutionManager,
}

impl TradingSession {
    pub fn new(symbol: String, filter_scalp_by_swing: bool, max_buffer_size: usize) -> Self {
        Self {
            symbol,
            filter_scalp_by_swing,
            predictor_model: "gbm".to_string(), // Default to GBM
            max_buffer_size,
            last_evaluation_timestamp: 0,
            last_m1_timestamp: 0,
            last_m5_timestamp: 0,
            last_m30_timestamp: 0,
            last_h1_timestamp: 0,
            last_h4_timestamp: 0,
            last_d1_timestamp: 0,
            swing_trend: TrendDirection::Sideways,
            m1_closes: VecDeque::with_capacity(max_buffer_size),
            m5_closes: VecDeque::with_capacity(max_buffer_size),
            m5_highs: VecDeque::with_capacity(max_buffer_size),
            m5_lows: VecDeque::with_capacity(max_buffer_size),
            m30_closes: VecDeque::with_capacity(max_buffer_size),
            h1_closes: VecDeque::with_capacity(max_buffer_size),
            h1_highs: VecDeque::with_capacity(max_buffer_size),
            h1_opens: VecDeque::with_capacity(max_buffer_size),
            h1_lows: VecDeque::with_capacity(max_buffer_size),
            h4_closes: VecDeque::with_capacity(max_buffer_size),
            h4_highs: VecDeque::with_capacity(max_buffer_size),
            h4_lows: VecDeque::with_capacity(max_buffer_size),
            d1_opens: VecDeque::with_capacity(max_buffer_size),
            d1_closes: VecDeque::with_capacity(max_buffer_size),
            latest_scalp_signal: None,
            latest_swing_signal: None,
            execution: ExecutionManager::new(true), // Scalp: Allow pyramiding
            swing_execution: ExecutionManager::new(false), // Swing: No pyramiding (First signal only)
        }
    }

    /// The main entry point for processing new data for this session.
    /// It updates internal buffers and then runs both trading engines.
    /// Note: req is now EvalRequest<'static> (owned) coming from the API.
    /// Returns a list of signals that should be notified.
    pub fn on_data(&mut self, req: EvalRequest<'static>, predictor_cache: &PredictorCache) -> Vec<EvalResponse> {
        info!(symbol = %self.symbol, "Processing new data for session.");

        // 1. Update data buffers with the latest candle data from the request.
        // Smart update: If input is small (incremental), push back. If large (sync), replace.
        // Guard against duplicate ticks: Only update if timestamp advanced OR it's a full sync (>10 items).
        
        // M1 Update
        if req.last_m1_timestamp > self.last_m1_timestamp || req.closes.len() > 10 {
            Self::update_buffer(&mut self.m1_closes, &req.closes, self.max_buffer_size);
            self.last_m1_timestamp = req.last_m1_timestamp;
        }

        // M5 Update
        if let Some(ts) = req.last_m5_timestamp {
            if ts > self.last_m5_timestamp || req.m5_closes.len() > 10 {
                Self::update_buffer(&mut self.m5_closes, &req.m5_closes, self.max_buffer_size);
                Self::update_buffer(&mut self.m5_highs, &req.m5_highs, self.max_buffer_size);
                Self::update_buffer(&mut self.m5_lows, &req.m5_lows, self.max_buffer_size);
                self.last_m5_timestamp = ts;
            }
        } else if !req.m5_closes.is_empty() {
            tracing::warn!(symbol = %self.symbol, "Received M5 data but no last_m5_timestamp. Ignoring update.");
        }

        // M30 Update
        if let Some(ts) = req.last_m30_timestamp {
            if ts > self.last_m30_timestamp || req.m30_closes.len() > 10 {
                Self::update_buffer(&mut self.m30_closes, &req.m30_closes, self.max_buffer_size);
                self.last_m30_timestamp = ts;
            }
        } else if !req.m30_closes.is_empty() {
            tracing::warn!(symbol = %self.symbol, "Received M30 data but no last_m30_timestamp. Ignoring update.");
        }
        
        // H1 Update
        if let Some(ts) = req.last_h1_timestamp {
            if ts > self.last_h1_timestamp || req.h1_closes.as_ref().map_or(false, |v| v.len() > 10) {
                if let Some(v) = &req.h1_closes { Self::update_buffer(&mut self.h1_closes, v, self.max_buffer_size); }
                if let Some(v) = &req.h1_highs { Self::update_buffer(&mut self.h1_highs, v, self.max_buffer_size); }
                if let Some(v) = &req.h1_opens { Self::update_buffer(&mut self.h1_opens, v, self.max_buffer_size); }
                if let Some(v) = &req.h1_lows { Self::update_buffer(&mut self.h1_lows, v, self.max_buffer_size); }
                self.last_h1_timestamp = ts;
            }
        } else if req.h1_closes.as_ref().map_or(false, |v| !v.is_empty()) {
            tracing::warn!(symbol = %self.symbol, "Received H1 data but no last_h1_timestamp. Ignoring update.");
        }
        
        // H4 Update
        if let Some(ts) = req.last_h4_timestamp {
            if ts > self.last_h4_timestamp || req.h4_closes.as_ref().map_or(false, |v| v.len() > 10) {
                if let Some(v) = &req.h4_closes { Self::update_buffer(&mut self.h4_closes, v, self.max_buffer_size); }
                if let Some(v) = &req.h4_highs { Self::update_buffer(&mut self.h4_highs, v, self.max_buffer_size); }
                if let Some(v) = &req.h4_lows { Self::update_buffer(&mut self.h4_lows, v, self.max_buffer_size); }
                self.last_h4_timestamp = ts;
            }
        } else if req.h4_closes.as_ref().map_or(false, |v| !v.is_empty()) {
            tracing::warn!(symbol = %self.symbol, "Received H4 data but no last_h4_timestamp. Ignoring update.");
        }
        
        // D1 Update
        if let Some(ts) = req.last_d1_timestamp {
            if ts > self.last_d1_timestamp || req.d1_closes.as_ref().map_or(false, |v| v.len() > 10) {
                if let Some(v) = &req.d1_opens { Self::update_buffer(&mut self.d1_opens, v, self.max_buffer_size); }
                if let Some(v) = &req.d1_closes { Self::update_buffer(&mut self.d1_closes, v, self.max_buffer_size); }
                self.last_d1_timestamp = ts;
            }
        } else if req.d1_closes.as_ref().map_or(false, |v| !v.is_empty()) {
            tracing::warn!(symbol = %self.symbol, "Received D1 data but no last_d1_timestamp. Ignoring update.");
        }
        
        self.last_evaluation_timestamp = req.last_m1_timestamp;

        // Log buffer status to help debug "Insufficient Data"
        info!(
            symbol = %self.symbol,
            m1_len = self.m1_closes.len(),
            m5_len = self.m5_closes.len(),
            h1_len = self.h1_closes.len(),
            "Session buffers updated."
        );

        let mut notifications = Vec::new();

        // 2. Run the Swing Engine
        // Scope the request to drop the borrow of self immediately after use
        let mut swing_signal = {
            let mut swing_req = self.build_engine_request("swing", req.current_price);
            swing_req.upcoming_events = req.upcoming_events.clone();
            SwingEngine::evaluate(&swing_req, predictor_cache)
        };
        
        info!(symbol = %self.symbol, signal_id = %swing_signal.signal_id, entry_type = %swing_signal.entry_type, "Swing engine evaluated.");

        // Latching Logic & Trade Management
        if let Some(existing) = &self.latest_swing_signal {
            if existing.signal_id == swing_signal.signal_id {
                // 1. Latch Entry
                swing_signal.entry_price = existing.entry_price;
                swing_signal.limit_order_price = existing.limit_order_price;
                swing_signal.recommended_order_type = existing.recommended_order_type.clone();

                // 2. SL Management (Lock & Trail)
                let current_price = req.current_price;
                let mut managed_sl = existing.sl_price; // Start with locked SL

                // Preserve Initial SL in debug_info for the frontend
                let initial_sl = existing.debug_info.as_ref()
                    .and_then(|d| d.get("initial_sl"))
                    .cloned()
                    .unwrap_or_else(|| format!("{:.2}", existing.sl_price));

                if swing_signal.debug_info.is_none() {
                    swing_signal.debug_info = Some(HashMap::new());
                }
                if let Some(d) = &mut swing_signal.debug_info {
                    d.insert("initial_sl".to_string(), initial_sl);
                }

                if swing_signal.entry_type == "long" {
                    // Invalidation: If price dropped below existing SL, kill signal
                    if current_price < managed_sl {
                        swing_signal.entry_type = "none".to_string();
                        swing_signal.reason = format!("Invalidated (Hit SL): {:.2} < {:.2}", current_price, managed_sl);
                    } else {
                        // Trailing: Move SL up if targets reached
                        if current_price > swing_signal.tp1_price { managed_sl = managed_sl.max(swing_signal.entry_price); }
                        if current_price > swing_signal.tp2_price { managed_sl = managed_sl.max(swing_signal.tp1_price); }
                        // Lock: Ensure SL never drops
                        swing_signal.sl_price = managed_sl.max(swing_signal.sl_price);
                    }
                } else if swing_signal.entry_type == "short" {
                    // Invalidation
                    if current_price > managed_sl {
                        swing_signal.entry_type = "none".to_string();
                        swing_signal.reason = format!("Invalidated (Hit SL): {:.2} > {:.2}", current_price, managed_sl);
                    } else {
                        // Trailing
                        if current_price < swing_signal.tp1_price { managed_sl = managed_sl.min(swing_signal.entry_price); }
                        if current_price < swing_signal.tp2_price { managed_sl = managed_sl.min(swing_signal.tp1_price); }
                        // Lock: Ensure SL never rises
                        swing_signal.sl_price = managed_sl.min(swing_signal.sl_price);
                    }
                }
            }
        }

        // Swing Execution Logic (Anti-Flicker & First-Signal Only)
        let current_time = req.last_m1_timestamp;
        let should_notify_swing = self.swing_execution.evaluate_execution(&mut swing_signal, current_time);
        if should_notify_swing {
            self.swing_execution.update_state(&swing_signal, current_time);
            notifications.push(swing_signal.clone());
        }

        // Update the session's swing trend based on the new signal.
        self.swing_trend = TrendDirection::from(swing_signal.entry_type.as_str());
        self.latest_swing_signal = Some(swing_signal);

        // 3. Run the Scalp Engine (reuse base_req data)
        let predictor_model = self.predictor_model.clone();
        let mut scalp_signal = {
            let mut scalp_req = self.build_engine_request("scalp", req.current_price);
            // Apply scalp-specific params from the original request
            scalp_req.spread_limit_points = req.spread_limit_points;
            scalp_req.spread_points = req.spread_points;
            scalp_req.kf_process_noise = req.kf_process_noise;
            scalp_req.kf_measurement_noise = req.kf_measurement_noise;
            scalp_req.upcoming_events = req.upcoming_events.clone();
            scalp_req.predictor_model = Some(predictor_model);

            ScalpEngine::evaluate(&scalp_req, predictor_cache)
        };
        info!(symbol = %self.symbol, signal_id = %scalp_signal.signal_id, entry_type = %scalp_signal.entry_type, "Scalp engine evaluated.");

        // 4. Apply cross-engine filtering if enabled.
        // This is where the scalp signal can be suppressed if it conflicts with the swing trend.
        if self.filter_scalp_by_swing {
            let scalp_trend = TrendDirection::from(scalp_signal.entry_type.as_str());
            if self.swing_trend != TrendDirection::Sideways && scalp_trend != self.swing_trend {
                info!(
                    symbol = %self.symbol,
                    "Scalp signal ({:?}) conflicts with swing trend ({:?}). Adding caution warning.",
                    scalp_trend, self.swing_trend
                );
                // Warn user but allow signal
                let trend_desc = match self.swing_trend {
                    TrendDirection::Up => "Upward",
                    TrendDirection::Down => "Downward",
                    _ => "Sideways",
                };
                scalp_signal.reason = format!("(Counter-Trend: {}): {}", trend_desc, scalp_signal.reason);
            }
        }

        // 5. FILTERING LOGIC & FINAL STORAGE
        // Delegate execution logic to the ExecutionManager
        let should_notify = self.execution.evaluate_execution(&mut scalp_signal, current_time);
        let is_same_id = self.execution.last_notified_signal_id.as_ref() == Some(&scalp_signal.signal_id);

        if should_notify {
            // Update execution state
            self.execution.update_state(&scalp_signal, current_time);
            notifications.push(scalp_signal.clone());
            self.latest_scalp_signal = Some(scalp_signal);
        } else if is_same_id {
            // Keep active for API visibility, but don't re-notify
            self.latest_scalp_signal = Some(scalp_signal);
        } else if scalp_signal.entry_type != "none" {
            // Valid signal but suppressed by spam filter
            info!(
                symbol = %self.symbol,
                reason = "Suppressed by anti-spam filter",
                "Scalp signal suppressed."
            );
            self.latest_scalp_signal = None;
        } else {
            // If the engine returns "none", there's no signal to store or suppress.
            self.latest_scalp_signal = Some(scalp_signal);
        }

        notifications
    }

    /// Helper to update a buffer.
    /// If `new_data` is large (> 10 items), it replaces the buffer (Sync).
    /// If `new_data` is small, it appends to the buffer and maintains MAX_BUFFER_SIZE (Incremental).
    fn update_buffer(buffer: &mut VecDeque<f64>, new_data: &[f64], max_len: usize) {
        if new_data.is_empty() { return; }

        if new_data.len() > 10 {
            // Assume full sync
            buffer.clear();
            // Cap at MAX_BUFFER_SIZE to prevent unbounded growth
            let start = new_data.len().saturating_sub(max_len);
            buffer.extend(new_data[start..].iter().cloned());
        } else {
            // Assume incremental update
            for &val in new_data {
                buffer.push_back(val);
                if buffer.len() > max_len {
                    buffer.pop_front();
                }
            }
        }
    }

    /// Builds an `EvalRequest` for a specific engine using the session's data.
    /// Uses Zero-Copy (Cow::Borrowed) to avoid allocations.
    fn build_engine_request<'a>(&'a mut self, mode: &str, current_price: f64) -> EvalRequest<'a> {
        // make_contiguous ensures the VecDeque is a single slice in memory
        let m1_closes = self.m1_closes.make_contiguous();
        let m5_closes = self.m5_closes.make_contiguous();
        let m5_highs = self.m5_highs.make_contiguous();
        let m5_lows = self.m5_lows.make_contiguous();
        let m30_closes = self.m30_closes.make_contiguous();
        
        // For Option fields, we map them
        let h1_closes = if !self.h1_closes.is_empty() { Some(Cow::Borrowed(self.h1_closes.make_contiguous() as &[f64])) } else { None };
        let h1_highs = if !self.h1_highs.is_empty() { Some(Cow::Borrowed(self.h1_highs.make_contiguous() as &[f64])) } else { None };
        let h1_opens = if !self.h1_opens.is_empty() { Some(Cow::Borrowed(self.h1_opens.make_contiguous() as &[f64])) } else { None };
        let h1_lows = if !self.h1_lows.is_empty() { Some(Cow::Borrowed(self.h1_lows.make_contiguous() as &[f64])) } else { None };
        
        let h4_closes = if !self.h4_closes.is_empty() { Some(Cow::Borrowed(self.h4_closes.make_contiguous() as &[f64])) } else { None };
        let h4_highs = if !self.h4_highs.is_empty() { Some(Cow::Borrowed(self.h4_highs.make_contiguous() as &[f64])) } else { None };
        let h4_lows = if !self.h4_lows.is_empty() { Some(Cow::Borrowed(self.h4_lows.make_contiguous() as &[f64])) } else { None };
        
        let d1_opens = if !self.d1_opens.is_empty() { Some(Cow::Borrowed(self.d1_opens.make_contiguous() as &[f64])) } else { None };
        let d1_closes = if !self.d1_closes.is_empty() { Some(Cow::Borrowed(self.d1_closes.make_contiguous() as &[f64])) } else { None };

        EvalRequest {
            symbol: Cow::Borrowed(&self.symbol),
            timeframe: Cow::Borrowed("M1"),
            closes: Cow::Borrowed(m1_closes),
            highs: Cow::Borrowed(&[]), 
            opens: Cow::Borrowed(&[]), 
            lows: Cow::Borrowed(&[]),  
            volumes: Cow::Borrowed(&[]), 
            m5_closes: Cow::Borrowed(m5_closes),
            m5_highs: Cow::Borrowed(m5_highs),
            m5_lows: Cow::Borrowed(m5_lows),
            m30_closes: Cow::Borrowed(m30_closes),
            h1_closes,
            h1_highs,
            h1_opens,
            h1_lows,
            h4_closes,
            h4_highs,
            h4_lows,
            d1_opens,
            d1_closes,
            mode: Cow::Owned(mode.to_string()), // Mode is usually small string
            current_price, 
            last_m1_timestamp: self.last_evaluation_timestamp,
            ..Default::default() // Fills in optional params
        }
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
#[derive(Clone)]
pub struct SessionManager {
    pub sessions: Arc<DashMap<String, Arc<Mutex<TradingSession>>>>,
    pub max_buffer_size: usize,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self { sessions: Arc::new(DashMap::new()), max_buffer_size: 500 }
    }
}

impl SessionManager {
    pub fn new(max_buffer_size: usize) -> Self {
        Self { sessions: Arc::new(DashMap::new()), max_buffer_size }
    }

    pub fn get_or_create_session(&self, symbol: &str, filter_scalp_by_swing: bool) -> Arc<Mutex<TradingSession>> {
        self.sessions.entry(symbol.to_string()).or_insert_with(|| {
            info!("Creating new trading session for symbol: {}", symbol);
            Arc::new(Mutex::new(TradingSession::new(symbol.to_string(), filter_scalp_by_swing, self.max_buffer_size)))
        }).clone()
    }
}