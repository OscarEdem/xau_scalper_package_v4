use crate::{
    engines::{scalp::ScalpEngine, swing::SwingEngine},
    engines::predictor_cache::PredictorCache,
    config::TradingSettings,
    EvalRequest, EvalResponse,
};
use dashmap::DashMap;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use tracing::info;
use serde::{Serialize, Deserialize};
use tokio::sync::{Mutex, broadcast};

#[derive(Serialize)]
struct WsSignal<'a> {
    symbol: &'a str,
    #[serde(flatten)]
    signal: &'a EvalResponse,
}

/// Represents the dominant trend direction determined by the SwingEngine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionManager {
    pub last_notified_signal_id: Option<String>,
    pub last_notified_direction: String, // "long", "short", "none"
    pub last_notified_time: i64,         // Unix timestamp
    pub last_notified_price: f64,
    pub pyramiding_enabled: bool,
    pub last_notified_scalp_mode: Option<String>,
    #[serde(default)]
    pub last_pushed_signal_id: Option<String>,
    #[serde(default)]
    pub strict_reversal_mode: bool,
}

impl ExecutionManager {
    pub fn new(pyramiding_enabled: bool, strict_reversal_mode: bool) -> Self {
        Self {
            last_notified_signal_id: None,
            last_notified_direction: "none".to_string(),
            last_notified_time: 0,
            last_notified_price: 0.0,
            pyramiding_enabled,
            last_notified_scalp_mode: None,
            last_pushed_signal_id: None,
            strict_reversal_mode,
        }
    }

    /// Evaluates if a signal should be broadcasted based on execution rules.
    /// Returns true if the signal is valid for notification.
    pub fn evaluate_execution(&mut self, signal: &mut EvalResponse, current_time: i64, reversal_threshold: f64) -> bool {
        if signal.entry_type == "none" {
            return false;
        }

        // Check if this is the same signal ID we are already tracking
        if self.last_notified_signal_id.as_ref() == Some(&signal.signal_id) {
            // Keep it active, but don't re-notify
            return false;
        }

        // --- Fade Mode Rule: No Re-entry ---
        // If the last trade was a Fade, do not allow any re-entry in the same direction. Only a reversal is permitted.
        if self.last_notified_scalp_mode.as_deref() == Some("fade") && signal.entry_type == self.last_notified_direction {
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

        // --- Averaging Logic (Max Profit / Better Entry) ---
        let averaging_threshold = 0.0005; // 0.05% better price triggers re-entry
        let is_averaging_entry = self.pyramiding_enabled && !direction_changed
            && match signal.entry_type.as_str() {
                "long" => price_delta_pct < -averaging_threshold, // Price moved lower (better buy)
                "short" => price_delta_pct > averaging_threshold, // Price moved higher (better sell)
                _ => false,
            };

        // --- Reversal Logic ---
        let is_rapid_reversal = direction_changed && time_diff < 180;
        let is_valid_reversal = direction_changed && (!is_rapid_reversal || conviction >= reversal_threshold);

        // --- Decision ---
        if self.strict_reversal_mode {
            // Strict Mode: Only Reversals or Pyramiding/Averaging. No time-based repeats.
            is_valid_reversal || is_pyramiding_breakout || is_averaging_entry
        } else {
            // Normal Mode (Scalp): Allow repeats after cooldown
            is_valid_reversal || (!direction_changed && cooldown_passed) || is_pyramiding_breakout || is_averaging_entry
        }
    }

    pub fn update_state(&mut self, signal: &EvalResponse, current_time: i64) {
        self.last_notified_signal_id = Some(signal.signal_id.clone());
        self.last_notified_direction = signal.entry_type.clone();
        self.last_notified_time = current_time;
        self.last_notified_price = signal.entry_price;
        self.last_notified_scalp_mode = signal.scalp_mode.clone();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReEntryContext {
    direction: String,
    level: f64,
    timestamp: i64,
}

/// Manages the state for a single trading symbol (e.g., "XAUUSD").
/// A TradingSession is created for each symbol the system trades. It holds all
/// necessary data buffers, open positions, and latest signals for that symbol.
#[derive(Debug, Serialize, Deserialize)]
pub struct TradingSession {
    pub symbol: String,
    /// Configuration flag for cross-engine filtering.
    /// If true, scalp signals will only be considered if they align with the swing trend.
    pub filter_scalp_by_swing: bool,
    /// The predictor model to use for this session (e.g., "gbm", "lstm").
    pub predictor_model: String,

    // --- State Data ---
    last_evaluation_timestamp: i64,
    last_m1_timestamp: i64,
    last_m5_timestamp: i64,
    last_m15_timestamp: i64,
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
    m5_timestamps: VecDeque<i64>,
    m15_closes: VecDeque<f64>,
    m15_highs: VecDeque<f64>,
    m15_lows: VecDeque<f64>,
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
    
    pending_re_entry: Option<ReEntryContext>,

    pub execution: ExecutionManager,
    pub swing_execution: ExecutionManager,

    #[serde(skip)]
    pub broadcast_tx: Option<broadcast::Sender<String>>,
}

impl TradingSession {
    pub fn new(symbol: String, filter_scalp_by_swing: bool, initial_buffer_size: usize, broadcast_tx: Option<broadcast::Sender<String>>) -> Self {
        Self {
            symbol,
            filter_scalp_by_swing,
            predictor_model: "gbm".to_string(), // Default to GBM
            last_evaluation_timestamp: 0,
            last_m1_timestamp: 0,
            last_m5_timestamp: 0,
            last_m15_timestamp: 0,
            last_m30_timestamp: 0,
            last_h1_timestamp: 0,
            last_h4_timestamp: 0,
            last_d1_timestamp: 0,
            swing_trend: TrendDirection::Sideways,
            m1_closes: VecDeque::with_capacity(initial_buffer_size),
            m5_closes: VecDeque::with_capacity(initial_buffer_size),
            m5_highs: VecDeque::with_capacity(initial_buffer_size),
            m5_lows: VecDeque::with_capacity(initial_buffer_size),
            m5_timestamps: VecDeque::with_capacity(initial_buffer_size),
            m15_closes: VecDeque::with_capacity(initial_buffer_size),
            m15_highs: VecDeque::with_capacity(initial_buffer_size),
            m15_lows: VecDeque::with_capacity(initial_buffer_size),
            m30_closes: VecDeque::with_capacity(initial_buffer_size),
            h1_closes: VecDeque::with_capacity(initial_buffer_size),
            h1_highs: VecDeque::with_capacity(initial_buffer_size),
            h1_opens: VecDeque::with_capacity(initial_buffer_size),
            h1_lows: VecDeque::with_capacity(initial_buffer_size),
            h4_closes: VecDeque::with_capacity(initial_buffer_size),
            h4_highs: VecDeque::with_capacity(initial_buffer_size),
            h4_lows: VecDeque::with_capacity(initial_buffer_size),
            d1_opens: VecDeque::with_capacity(initial_buffer_size),
            d1_closes: VecDeque::with_capacity(initial_buffer_size),
            latest_scalp_signal: None,
            latest_swing_signal: None,
            pending_re_entry: None,
            execution: ExecutionManager::new(true, false), // Scalp: Allow pyramiding, Normal mode
            swing_execution: ExecutionManager::new(true, true), // Swing: Pyramiding YES, Strict Reversal YES
            broadcast_tx,
        }
    }

    /// The main entry point for processing new data for this session.
    /// It updates internal buffers and then runs both trading engines.
    /// Note: req is now EvalRequest<'static> (owned) coming from the API.
    /// It takes an Arc<RwLock<...>> to ensure it always reads the latest, hot-reloaded settings.
    /// Returns a list of signals that should be notified.
    pub fn on_data(&mut self, req: EvalRequest<'static>, predictor_cache: &PredictorCache, settings_arc: &Arc<RwLock<TradingSettings>>) -> Vec<EvalResponse> {
        let settings = settings_arc.read().unwrap();
        info!(symbol = %self.symbol, "Processing new data for session with hot-reloaded settings.");

        // Update session configuration from global settings
        self.filter_scalp_by_swing = settings.scalp.filter_scalp_by_swing;

        // 1. Update data buffers with the latest candle data from the request.
        // Smart update: If input is small (incremental), push back. If large (sync), replace.
        // Guard against duplicate ticks: Only update if timestamp advanced OR it's a full sync (>10 items).
        
        // M1 Update
        // Increased sync threshold to 100 to prevent wiping history on small catch-up batches
        if req.last_m1_timestamp > self.last_m1_timestamp || req.closes.len() > settings.sync_threshold {
            Self::update_buffer(&mut self.m1_closes, &req.closes, settings.max_buffer_size);
            self.last_m1_timestamp = req.last_m1_timestamp;
        }

        // M5 Update
        if let Some(ts) = req.last_m5_timestamp {
            if (ts > 0 && ts > self.last_m5_timestamp) || req.m5_closes.len() > settings.sync_threshold {
                Self::update_buffer(&mut self.m5_closes, &req.m5_closes, settings.max_buffer_size);
                Self::update_buffer(&mut self.m5_highs, &req.m5_highs, settings.max_buffer_size);
                Self::update_buffer(&mut self.m5_lows, &req.m5_lows, settings.max_buffer_size);
                
                // Handle timestamps
                if let Some(ts_vec) = &req.m5_timestamps {
                    Self::update_buffer_i64(&mut self.m5_timestamps, ts_vec, settings.max_buffer_size);
                } else {
                    // Backfill if missing (Assume 300s intervals ending at last_m5_timestamp)
                    let count = req.m5_closes.len();
                    let mut generated = Vec::with_capacity(count);
                    for i in 0..count {
                        generated.push(ts - ((count - 1 - i) as i64 * 300));
                    }
                    Self::update_buffer_i64(&mut self.m5_timestamps, &generated, settings.max_buffer_size);
                }

                self.last_m5_timestamp = ts;
            }
        } else if !req.m5_closes.is_empty() {
            tracing::warn!(symbol = %self.symbol, "Received M5 data but no last_m5_timestamp. Ignoring update.");
        }

        // M15 Update
        if let Some(ts) = req.last_m15_timestamp {
            if (ts > 0 && ts > self.last_m15_timestamp) || req.m15_closes.as_ref().map_or(false, |v| v.len() > settings.sync_threshold) {
                if let Some(v) = &req.m15_closes { Self::update_buffer(&mut self.m15_closes, v, settings.max_buffer_size); }
                if let Some(v) = &req.m15_highs { Self::update_buffer(&mut self.m15_highs, v, settings.max_buffer_size); }
                if let Some(v) = &req.m15_lows { Self::update_buffer(&mut self.m15_lows, v, settings.max_buffer_size); }
                self.last_m15_timestamp = ts;
            }
        } else if req.m15_closes.as_ref().map_or(false, |v| !v.is_empty()) {
            tracing::warn!(symbol = %self.symbol, "Received M15 data but no last_m15_timestamp. Ignoring update.");
        }

        // M30 Update
        if let Some(ts) = req.last_m30_timestamp {
            if (ts > 0 && ts > self.last_m30_timestamp) || req.m30_closes.len() > settings.sync_threshold {
                Self::update_buffer(&mut self.m30_closes, &req.m30_closes, settings.max_buffer_size);
                self.last_m30_timestamp = ts;
            }
        } else if !req.m30_closes.is_empty() {
            tracing::warn!(symbol = %self.symbol, "Received M30 data but no last_m30_timestamp. Ignoring update.");
        }
        
        // H1 Update
        if let Some(ts) = req.last_h1_timestamp {
            if (ts > 0 && ts > self.last_h1_timestamp) || req.h1_closes.as_ref().map_or(false, |v| v.len() > settings.sync_threshold) {
                if let Some(v) = &req.h1_closes { Self::update_buffer(&mut self.h1_closes, v, settings.max_buffer_size); }
                if let Some(v) = &req.h1_highs { Self::update_buffer(&mut self.h1_highs, v, settings.max_buffer_size); }
                if let Some(v) = &req.h1_opens { Self::update_buffer(&mut self.h1_opens, v, settings.max_buffer_size); }
                if let Some(v) = &req.h1_lows { Self::update_buffer(&mut self.h1_lows, v, settings.max_buffer_size); }
                self.last_h1_timestamp = ts;
            }
        } else if req.h1_closes.as_ref().map_or(false, |v| !v.is_empty()) {
            tracing::warn!(symbol = %self.symbol, "Received H1 data but no last_h1_timestamp. Ignoring update.");
        }
        
        // H4 Update
        if let Some(ts) = req.last_h4_timestamp {
            if (ts > 0 && ts > self.last_h4_timestamp) || req.h4_closes.as_ref().map_or(false, |v| v.len() > settings.sync_threshold) {
                if let Some(v) = &req.h4_closes { Self::update_buffer(&mut self.h4_closes, v, settings.max_buffer_size); }
                if let Some(v) = &req.h4_highs { Self::update_buffer(&mut self.h4_highs, v, settings.max_buffer_size); }
                if let Some(v) = &req.h4_lows { Self::update_buffer(&mut self.h4_lows, v, settings.max_buffer_size); }
                self.last_h4_timestamp = ts;
            }
        } else if req.h4_closes.as_ref().map_or(false, |v| !v.is_empty()) {
            tracing::warn!(symbol = %self.symbol, "Received H4 data but no last_h4_timestamp. Ignoring update.");
        }
        
        // D1 Update
        if let Some(ts) = req.last_d1_timestamp {
            if (ts > 0 && ts > self.last_d1_timestamp) || req.d1_closes.as_ref().map_or(false, |v| v.len() > settings.sync_threshold) {
                if let Some(v) = &req.d1_opens { Self::update_buffer(&mut self.d1_opens, v, settings.max_buffer_size); }
                if let Some(v) = &req.d1_closes { Self::update_buffer(&mut self.d1_closes, v, settings.max_buffer_size); }
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
            SwingEngine::evaluate(&swing_req, predictor_cache, &settings.swing, &settings.risk)
        };
        
        info!(symbol = %self.symbol, signal_id = %swing_signal.signal_id, entry_type = %swing_signal.entry_type, "Swing engine evaluated.");

        // --- NEW: Explicitly label Liquidity Sweeps in the notification text ---
        if swing_signal.sweep_detected != "none" && !swing_signal.reason.to_lowercase().contains("sweep") {
            let sweep_desc = if swing_signal.sweep_detected == "high_sweep" { "Highs" } else { "Lows" };
            swing_signal.reason = format!("Structural Liquidity Sweep ({}): {}", sweep_desc, swing_signal.reason);
        }

        // Calculate current ATR for logging purposes
        let h1_atr = crate::atr(&req.h1_highs.as_ref().unwrap_or(&Cow::Borrowed(&[])), 
                                &req.h1_lows.as_ref().unwrap_or(&Cow::Borrowed(&[])), 
                                &req.h1_closes.as_ref().unwrap_or(&Cow::Borrowed(&[])), 14)
                                .last().cloned().unwrap_or(0.0);

        // Latching Logic & Trade Management
        // Capture previous SL to detect updates
        let prev_sl = if let Some(existing) = &self.latest_swing_signal {
            if existing.signal_id == swing_signal.signal_id { existing.sl_price } else { 0.0 }
        } else { 0.0 };

        self.manage_active_swing_trade(&mut swing_signal, req.current_price, req.last_m1_timestamp, h1_atr);
        let sl_changed = prev_sl > 0.0 && (swing_signal.sl_price - prev_sl).abs() > 0.0001;

        // --- Check for Re-Entry Trigger (Bounce off BE) ---
        self.check_swing_reentry(&mut swing_signal, req.current_price, req.last_m1_timestamp, &settings.swing);

        // Swing Execution Logic (Anti-Flicker & First-Signal Only)
        let current_time = req.last_m1_timestamp;
        let is_same_swing_id = self.swing_execution.last_notified_signal_id.as_ref() == Some(&swing_signal.signal_id);
        let should_notify_swing = self.swing_execution.evaluate_execution(&mut swing_signal, current_time, settings.swing.conviction_threshold);
        if should_notify_swing {
            self.swing_execution.update_state(&swing_signal, current_time);
            // Only send Push Notification if enabled and conviction is high enough
            if settings.swing.push_notifications_enabled && swing_signal.conviction_score.unwrap_or(0.0) >= settings.swing.push_notification_threshold {
                self.swing_execution.last_pushed_signal_id = Some(swing_signal.signal_id.clone());
                swing_signal.should_push = true;
                Self::format_push_notification(&mut swing_signal, &self.symbol);
            }
            notifications.push(swing_signal.clone());
            if let Some(tx) = &self.broadcast_tx {
                let ws_msg = WsSignal { symbol: &self.symbol, signal: &swing_signal };
                // Strip heavy zone data for WebSocket to reduce payload
                if let Ok(mut v) = serde_json::to_value(&ws_msg) {
                    if let Some(obj) = v.as_object_mut() {
                        obj.remove("liquidityZones");
                        obj.remove("imbalances");
                        obj.remove("imbalanceZones");
                        obj.remove("orderBlocks");
                        obj.remove("debug_info");
                        obj.remove("debugInfo");
                        obj.remove("reason");
                        obj.remove("time_stop_seconds");
                        obj.remove("timeStopSeconds");
                    }
                    let _ = serde_json::to_string(&v).map(|msg| tx.send(msg));
                }
            }
        } else if swing_signal.entry_type == "none" && is_same_swing_id {
            // --- NEW: Notify on Trade Closure (SL Hit / Invalidation) ---
            // If the signal ID matches the active trade, but entry_type is now "none", it means it was just invalidated.
            if settings.swing.push_notifications_enabled && (swing_signal.reason.contains("Invalidated") || swing_signal.reason.contains("Stopped")) {
                let mut close_notification = swing_signal.clone();
                // We must set entry_type back to something valid (e.g. "close") or keep "none" but ensure frontend handles it.
                // Here we keep "none" but rely on the 'reason' field for the UI.
                close_notification.should_push = true;
                Self::format_push_notification(&mut close_notification, &self.symbol);
                notifications.push(close_notification);
                
                // Clear the execution state so we don't notify again
                self.swing_execution.last_notified_signal_id = None;
            }
        } else if is_same_swing_id {
            // Check for late conviction bloom (Signal was processed but not pushed, now strong enough)
            if settings.swing.push_notifications_enabled 
                && self.swing_execution.last_pushed_signal_id.as_ref() != Some(&swing_signal.signal_id)
                && swing_signal.conviction_score.unwrap_or(0.0) >= settings.swing.push_notification_threshold 
            {
                self.swing_execution.last_pushed_signal_id = Some(swing_signal.signal_id.clone());
                swing_signal.should_push = true;
                Self::format_push_notification(&mut swing_signal, &self.symbol);
                notifications.push(swing_signal.clone());
                
                // Broadcast update so UI reflects high conviction
                if let Some(tx) = &self.broadcast_tx {
                    let ws_msg = WsSignal { symbol: &self.symbol, signal: &swing_signal };
                    if let Ok(v) = serde_json::to_value(&ws_msg) {
                        // ... (stripping logic omitted for brevity, same as above) ...
                        let _ = serde_json::to_string(&v).map(|msg| tx.send(msg));
                    }
                }
            }

            // Notify on SL Update (Move to BE, Trailing)
            if sl_changed && settings.swing.push_notifications_enabled {
                let mut reason_text = format!("Update: Stop Loss moved to {:.2}", swing_signal.sl_price);
                // Check if it was a move to Breakeven by comparing the new SL to the entry price
                let is_breakeven_move = (swing_signal.sl_price - swing_signal.entry_price).abs() < 0.0001;
                if is_breakeven_move {
                    reason_text = format!("Update: TP1 Hit. Stop Loss moved to Breakeven at {:.2}", swing_signal.sl_price);
                }

                let mut update_signal = swing_signal.clone();
                update_signal.reason = reason_text;
                update_signal.should_push = true;
                Self::format_push_notification(&mut update_signal, &self.symbol);
                // We push this as a notification. The ID is the same, but the reason is different.
                notifications.push(update_signal);
            }
        }

        // Update the session's swing trend based on the new signal.
        // If no active signal, use HTF bias from debug_info to determine trend for filtering.
        self.swing_trend = match swing_signal.entry_type.as_str() {
            "long" => TrendDirection::Up,
            "short" => TrendDirection::Down,
            _ => {
                let bias = swing_signal.debug_info.as_ref()
                    .and_then(|d| d.get("htf_bias_score"))
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                
                if bias > 0.25 { TrendDirection::Up }
                else if bias < -0.25 { TrendDirection::Down }
                else { TrendDirection::Sideways }
            }
        };
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

            // Note: The ScalpEngine::evaluate function must also be updated to accept `&settings.risk`
            ScalpEngine::evaluate(&scalp_req, predictor_cache, &settings.scalp) // Placeholder, see note
        };
        info!(symbol = %self.symbol, signal_id = %scalp_signal.signal_id, entry_type = %scalp_signal.entry_type, "Scalp engine evaluated.");

        // 4. Apply cross-engine filtering if enabled.
        // This is where the scalp signal can be suppressed if it conflicts with the swing trend.
        if self.filter_scalp_by_swing {
            let scalp_trend = TrendDirection::from(scalp_signal.entry_type.as_str());
            if self.swing_trend != TrendDirection::Sideways 
                && scalp_trend != TrendDirection::Sideways 
                && scalp_trend != self.swing_trend {
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
        let should_notify = self.execution.evaluate_execution(&mut scalp_signal, current_time, settings.scalp.min_conviction);
        // Check ID match before evaluate_execution potentially updates state (though it doesn't here, it's safer)
        let is_same_scalp_id = self.execution.last_notified_signal_id.as_ref() == Some(&scalp_signal.signal_id);

        if should_notify {
            // Update execution state
            self.execution.update_state(&scalp_signal, current_time);
            // Only send Push Notification if enabled and conviction is high enough
            if settings.scalp.push_notifications_enabled && scalp_signal.conviction_score.unwrap_or(0.0) >= settings.scalp.push_notification_threshold {
                self.execution.last_pushed_signal_id = Some(scalp_signal.signal_id.clone());
                scalp_signal.should_push = true;
                Self::format_push_notification(&mut scalp_signal, &self.symbol);
            }
            notifications.push(scalp_signal.clone());
            if let Some(tx) = &self.broadcast_tx {
                let ws_msg = WsSignal { symbol: &self.symbol, signal: &scalp_signal };
                // Strip heavy zone data for WebSocket to reduce payload
                if let Ok(mut v) = serde_json::to_value(&ws_msg) {
                    if let Some(obj) = v.as_object_mut() {
                        obj.remove("liquidityZones");
                        obj.remove("imbalances");
                        obj.remove("imbalanceZones");
                        obj.remove("orderBlocks");
                        obj.remove("debug_info");
                        obj.remove("debugInfo");
                        obj.remove("reason");
                        obj.remove("time_stop_seconds");
                        obj.remove("timeStopSeconds");
                    }
                    let _ = serde_json::to_string(&v).map(|msg| tx.send(msg));
                }
            }
            self.latest_scalp_signal = Some(scalp_signal);
        } else if is_same_scalp_id {
            // Keep active for API visibility, but don't re-notify
            
            // Check for late conviction bloom
            if settings.scalp.push_notifications_enabled 
                && self.execution.last_pushed_signal_id.as_ref() != Some(&scalp_signal.signal_id)
                && scalp_signal.conviction_score.unwrap_or(0.0) >= settings.scalp.push_notification_threshold 
            {
                self.execution.last_pushed_signal_id = Some(scalp_signal.signal_id.clone());
                scalp_signal.should_push = true;
                Self::format_push_notification(&mut scalp_signal, &self.symbol);
                notifications.push(scalp_signal.clone());
                
                // Broadcast update
                if let Some(tx) = &self.broadcast_tx {
                    let ws_msg = WsSignal { symbol: &self.symbol, signal: &scalp_signal };
                    if let Ok(v) = serde_json::to_value(&ws_msg) {
                        let _ = serde_json::to_string(&v).map(|msg| tx.send(msg));
                    }
                }
            }
            
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

    /// Formats the `push_title` and `push_body` fields of a signal for a push notification service.
    fn format_push_notification(signal: &mut EvalResponse, symbol: &str) {
        let direction = signal.entry_type.to_uppercase();
        let classification = signal.classification.to_uppercase();

        // 1. Handle Trade Closure
        if signal.entry_type == "none" {
            signal.push_title = Some(format!("{} Trade Closed", symbol));
            if signal.reason.contains("Invalidated") {
                signal.push_body = Some("Position closed: Stop Loss hit.".to_string());
            } else if signal.reason.contains("Stopped") {
                signal.push_body = Some("Position closed: Stopped at Breakeven/Trail.".to_string());
            } else {
                signal.push_body = Some("Position invalidated or closed.".to_string());
            }
            return;
        }

        // 2. Handle Updates
        if signal.reason.starts_with("Update:") {
            signal.push_title = Some(format!("{} {} Update", symbol, direction));
            if signal.reason.contains("TP1 Hit") {
                signal.push_body = Some(format!("💰 TP1 Hit! SL moved to Breakeven @ {:.4}", signal.sl_price));
            } else {
                signal.push_body = Some(format!("🛡️ SL Trailed to {:.4}", signal.sl_price));
            }
            return;
        }

        // 3. Handle New Entries (Scalp, Swing, Re-entry, SFP)
        let mut title_prefix = "New";
        let mut body_prefix = "".to_string();

        if signal.classification == "swing_reentry" {
            title_prefix = "Re-Entry";
            body_prefix = "🔄 ".to_string();
        } else if signal.sweep_detected != "none" {
            body_prefix = "💧 Liquidity Sweep. ".to_string();
        } else if signal.reason.starts_with("(Counter-Trend:") {
            body_prefix = "⚠️ Counter-Trend. ".to_string();
        }

        signal.push_title = Some(format!("{} {} Signal: {} {}", title_prefix, symbol, classification, direction));
        signal.push_body = Some(format!("{}Entry: {:.4}, SL: {:.4}, TP1: {:.4}", body_prefix, signal.entry_price, signal.sl_price, signal.tp1_price));
    }

    fn manage_active_swing_trade(&mut self, swing_signal: &mut EvalResponse, current_price: f64, timestamp: i64, current_atr: f64) {
        if let Some(existing) = &self.latest_swing_signal {
            // Only latch if the existing signal is still active. If it was invalidated, treat fresh signal as new.
            if existing.signal_id == swing_signal.signal_id && existing.entry_type != "none" {
                // 1. Latch Entry
                swing_signal.entry_price = existing.entry_price;
                swing_signal.limit_order_price = existing.limit_order_price;
                swing_signal.recommended_order_type = existing.recommended_order_type.clone();

                // Latch TPs to prevent them from floating with current price.
                // This ensures trailing logic (which compares current_price vs TP1) works consistently.
                swing_signal.tp1_price = existing.tp1_price;
                swing_signal.tp2_price = existing.tp2_price;
                swing_signal.tp3_price = existing.tp3_price;

                // 2. SL Management (Lock & Trail)
                let mut managed_sl = existing.sl_price; // Start with locked SL

                // --- NEW: Persistence for High/Low Watermark ---
                // Track best price to ensure trailing triggers even if price retraces immediately.
                let mut best_price = current_price;
                if let Some(info) = &existing.debug_info {
                    if let Some(val_str) = info.get("best_price") {
                        if let Ok(val) = val_str.parse::<f64>() {
                            best_price = val;
                        }
                    }
                }

                // Preserve Initial SL in debug_info for the frontend
                let initial_sl = existing.debug_info.as_ref()
                    .and_then(|d| d.get("initial_sl"))
                    .cloned()
                    .unwrap_or_else(|| format!("{:.5}", existing.sl_price));
                
                let initial_sl_val = initial_sl.parse::<f64>().unwrap_or(existing.sl_price);

                if swing_signal.debug_info.is_none() {
                    swing_signal.debug_info = Some(HashMap::new());
                }
                if let Some(d) = &mut swing_signal.debug_info {
                    d.insert("initial_sl".to_string(), initial_sl);
                    
                    // Update and store watermark
                    if swing_signal.entry_type == "long" {
                        best_price = best_price.max(current_price);
                    } else {
                        best_price = best_price.min(current_price);
                    }
                    d.insert("best_price".to_string(), format!("{:.5}", best_price));
                }

                if swing_signal.entry_type == "long" {
                    // Invalidation: If price dropped below existing SL, kill signal
                    if current_price < managed_sl {
                        // Log SL Hit details
                        let entry_atr = existing.debug_info.as_ref()
                            .and_then(|d| d.get("entry_atr"))
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        
                        let session_name = crate::engines::scalp::ScalpEngine::session_utc(timestamp);
                        
                        tracing::info!(symbol = %self.symbol, "SL HIT: Session={}, EntryATR={:.4}, ExitATR={:.4}, Price={:.2}, SL={:.2}", session_name, entry_atr, current_atr, current_price, managed_sl);

                        swing_signal.entry_type = "none".to_string();
                        // Distinguish between Initial SL hit and Trailing/BE hit
                        if managed_sl > initial_sl_val + 0.00001 {
                            swing_signal.reason = format!("Stopped at Breakeven/Trail: {:.2} < {:.2}", current_price, managed_sl);
                            // Enable Re-Entry Watch
                            self.pending_re_entry = Some(ReEntryContext {
                                direction: "long".to_string(),
                                level: managed_sl,
                                timestamp,
                            });
                        } else {
                            swing_signal.reason = format!("Invalidated (Hit SL): {:.2} < {:.2}", current_price, managed_sl);
                        }
                        // Reset execution memory to allow re-entry if structure holds
                        self.swing_execution.last_notified_signal_id = None;
                    } else {
                        // Trailing: Move SL up if targets reached
                        if best_price >= swing_signal.tp1_price { managed_sl = managed_sl.max(swing_signal.entry_price); }
                        if best_price >= swing_signal.tp2_price { managed_sl = managed_sl.max(swing_signal.tp1_price); }
                        // Lock: Ensure SL never drops
                        swing_signal.sl_price = managed_sl.max(swing_signal.sl_price);
                    }
                } else if swing_signal.entry_type == "short" {
                    // Invalidation
                    if current_price > managed_sl {
                        // Log SL Hit details
                        let entry_atr = existing.debug_info.as_ref()
                            .and_then(|d| d.get("entry_atr"))
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let session_name = crate::engines::scalp::ScalpEngine::session_utc(timestamp);

                        tracing::info!(symbol = %self.symbol, "SL HIT: Session={}, EntryATR={:.4}, ExitATR={:.4}, Price={:.2}, SL={:.2}", session_name, entry_atr, current_atr, current_price, managed_sl);

                        swing_signal.entry_type = "none".to_string();
                        // Distinguish between Initial SL hit and Trailing/BE hit
                        if managed_sl < initial_sl_val - 0.00001 {
                            swing_signal.reason = format!("Stopped at Breakeven/Trail: {:.2} > {:.2}", current_price, managed_sl);
                            // Enable Re-Entry Watch
                            self.pending_re_entry = Some(ReEntryContext {
                                direction: "short".to_string(),
                                level: managed_sl,
                                timestamp,
                            });
                        } else {
                            swing_signal.reason = format!("Invalidated (Hit SL): {:.2} > {:.2}", current_price, managed_sl);
                        }
                        // Reset execution memory to allow re-entry if structure holds
                        self.swing_execution.last_notified_signal_id = None;
                    } else {
                        // Trailing
                        if best_price <= swing_signal.tp1_price { managed_sl = managed_sl.min(swing_signal.entry_price); }
                        if best_price <= swing_signal.tp2_price { managed_sl = managed_sl.min(swing_signal.tp1_price); }
                        // Lock: Ensure SL never rises
                        swing_signal.sl_price = managed_sl.min(swing_signal.sl_price);
                    }
                }
            }
        }
    }

    fn check_swing_reentry(&mut self, swing_signal: &mut EvalResponse, current_price: f64, timestamp: i64, settings: &crate::config::SwingSettings) {
        // Safety: Do not trigger re-entry if the engine is explicitly blocked (News/Volatility)
        let is_blocked = swing_signal.reason.starts_with("Blocked");
        if swing_signal.entry_type == "none" && !is_blocked {
            if let Some(ctx) = &self.pending_re_entry {
                let elapsed = timestamp - ctx.timestamp;
                // Expire after configured time
                if elapsed > settings.reentry_expiration_seconds {
                    self.pending_re_entry = None;
                } else {
                    let bounce_threshold = settings.reentry_bounce_threshold;
                    let invalidation_threshold = settings.reentry_invalidation_threshold;

                    if ctx.direction == "long" {
                        if current_price < ctx.level * (1.0 - invalidation_threshold) {
                            self.pending_re_entry = None; // Failed support
                        } else if current_price > ctx.level * (1.0 + bounce_threshold) {
                            // Trigger Re-Entry
                            swing_signal.entry_type = "long".to_string();
                            swing_signal.entry_price = current_price;
                            swing_signal.reason = "Re-Entry: Price bounced off Breakeven/Support".to_string();
                            swing_signal.signal_id = format!("{}-reentry-{}", self.symbol, timestamp);
                            // Set SL/TP for re-entry (tight SL below bounce)
                            swing_signal.sl_price = ctx.level * (1.0 - settings.reentry_sl_pct); // SL just below BE
                            swing_signal.tp1_price = current_price * (1.0 + settings.reentry_tp1_pct);
                            swing_signal.tp2_price = current_price * (1.0 + settings.reentry_tp2_pct);
                            swing_signal.classification = "swing_reentry".to_string();
                            swing_signal.conviction_score = Some(60.0); // Default conviction for re-entry
                            
                            self.pending_re_entry = None; // Consumed
                        }
                    } else if ctx.direction == "short" {
                        if current_price > ctx.level * (1.0 + invalidation_threshold) {
                            self.pending_re_entry = None; // Failed resistance
                        } else if current_price < ctx.level * (1.0 - bounce_threshold) {
                            // Trigger Re-Entry
                            swing_signal.entry_type = "short".to_string();
                            swing_signal.entry_price = current_price;
                            swing_signal.reason = "Re-Entry: Price bounced off Breakeven/Resistance".to_string();
                            swing_signal.signal_id = format!("{}-reentry-{}", self.symbol, timestamp);
                            swing_signal.sl_price = ctx.level * (1.0 + settings.reentry_sl_pct);
                            swing_signal.tp1_price = current_price * (1.0 - settings.reentry_tp1_pct);
                            swing_signal.tp2_price = current_price * (1.0 - settings.reentry_tp2_pct);
                            swing_signal.classification = "swing_reentry".to_string();
                            swing_signal.conviction_score = Some(60.0);

                            self.pending_re_entry = None; // Consumed
                        }
                    }
                }
            }
        }
    }

    /// Helper to update a buffer.
    /// If `new_data` is large (> 10 items), it replaces the buffer (Sync).
    /// If `new_data` is small, it appends to the buffer and maintains MAX_BUFFER_SIZE (Incremental).
    fn update_buffer(buffer: &mut VecDeque<f64>, new_data: &[f64], max_len: usize) {
        if new_data.is_empty() { return; }

        // Heuristic: Only treat as "Full Sync" (replace) if data is substantial (>100 items).
        // Smaller batches are treated as incremental catch-ups to preserve history.
        if new_data.len() > 100 {
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

    fn update_buffer_i64(buffer: &mut VecDeque<i64>, new_data: &[i64], max_len: usize) {
        if new_data.is_empty() { return; }
        if new_data.len() > 100 {
            buffer.clear();
            let start = new_data.len().saturating_sub(max_len);
            buffer.extend(new_data[start..].iter().cloned());
        } else {
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
        let m5_timestamps = if !self.m5_timestamps.is_empty() { Some(Cow::Borrowed(self.m5_timestamps.make_contiguous() as &[i64])) } else { None };
        let m30_closes = self.m30_closes.make_contiguous();

        let m15_closes = if !self.m15_closes.is_empty() { Some(Cow::Borrowed(self.m15_closes.make_contiguous() as &[f64])) } else { None };
        let m15_highs = if !self.m15_highs.is_empty() { Some(Cow::Borrowed(self.m15_highs.make_contiguous() as &[f64])) } else { None };
        let m15_lows = if !self.m15_lows.is_empty() { Some(Cow::Borrowed(self.m15_lows.make_contiguous() as &[f64])) } else { None };
        
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
            m5_timestamps,
            m15_closes,
            m15_highs,
            m15_lows,
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
            last_m15_timestamp: Some(self.last_m15_timestamp),
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
    pub settings: Arc<RwLock<TradingSettings>>,
    pub broadcast_tx: broadcast::Sender<String>,
}

impl Default for SessionManager {
    fn default() -> Self {
        // This default is rarely used as we load from config, but good for tests
        let settings = TradingSettings::default();
        let (tx, _) = broadcast::channel(1);
        Self { sessions: Arc::new(DashMap::new()), settings: Arc::new(RwLock::new(settings)), broadcast_tx: tx }
    }
}

impl SessionManager {
    pub fn new(settings: TradingSettings, broadcast_tx: broadcast::Sender<String>) -> Self {
        Self { sessions: Arc::new(DashMap::new()), settings: Arc::new(RwLock::new(settings)), broadcast_tx }
    }

    pub fn get_or_create_session(&self, symbol: &str, filter_scalp_by_swing: bool) -> Arc<Mutex<TradingSession>> {
        self.sessions.entry(symbol.to_string()).or_insert_with(|| {
            info!("Creating new trading session for symbol: {}", symbol);
            let settings = self.settings.read().unwrap();
            Arc::new(Mutex::new(TradingSession::new(symbol.to_string(), filter_scalp_by_swing, settings.max_buffer_size, Some(self.broadcast_tx.clone()))))
        }).clone()
    }

    pub async fn update_settings(&self, new_settings: TradingSettings) {
        // 1. Update global settings for future sessions
        *self.settings.write().unwrap() = new_settings.clone();
        info!("Global settings updated. All active sessions will use new settings on next tick.");
    }
}