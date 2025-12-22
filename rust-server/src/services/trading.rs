use std::sync::Arc;
use std::sync::atomic::Ordering;
use chrono::Utc;
use axum::http::StatusCode;
use expo_push_notification_client::{Expo, ExpoClientOptions, ExpoPushMessage};
use xau_scalper_server::{EvalRequest, EvalResponse};
use crate::state::{ApplicationStateWithTicks, HistoricalSignal, ActiveSignal};

pub struct TradingService {
    state: Arc<ApplicationStateWithTicks>,
}

impl TradingService {
    pub fn new(state: Arc<ApplicationStateWithTicks>) -> Self {
        Self { state }
    }

    pub fn broadcast_tick(&self, tick_json: String) {
         if !tick_json.is_empty() {
            // Send this JSON string to all connected WebSocket clients
            if self.state.tick_tx.send(tick_json).is_err() {
                tracing::debug!("Tick received, but no active WebSocket clients to broadcast to.");
            }
        }
    }

    pub async fn process_eval_request(&self, mut req: EvalRequest<'static>) -> Result<(), StatusCode> {
        self.state.inner.metrics.http_requests.inc();
        
        // Optimization: Only serialize and send if there are actual subscribers
        if self.state.tick_tx.receiver_count() > 0 {
            let tick_payload = serde_json::json!({
                "type": "tick",
                "symbol": req.symbol.clone(),
                "currentPrice": req.current_price,
                "spreadPoints": req.spread_points.unwrap_or(0.0),
                "lastM1Timestamp": req.last_m1_timestamp,
            })
            .to_string();

            let _ = self.state.tick_tx.send(tick_payload);
        }

        // --- NEW: Inject cached news events into the request ---
        let news_guard = self.state.inner.news_events.lock().await;
        let news = news_guard.clone();
        req.upcoming_events = Some(std::borrow::Cow::Owned(news));

        // For now, we assume a global config for filtering. This could also be part of the request.
        let filter_scalp_by_swing = true;

        // --- PERFORMANCE FIX: Offload heavy math to a blocking thread ---
        let state_clone = self.state.clone();
        
        let signals_to_send = tokio::task::spawn_blocking(move || {
            let mut signals: Vec<(EvalResponse, String)> = Vec::new();
            let symbol: String = req.symbol.to_string();

            // Use the helper to get the session Arc, handling the map lock internally
            let session_arc = state_clone.inner.session_manager.get_or_create_session(&symbol, filter_scalp_by_swing);
            state_clone.inner.metrics.active_sessions.set(state_clone.inner.session_manager.sessions.len() as f64);
            
            // Lock the specific session using blocking_lock since we are in a blocking thread
            let mut session = session_arc.blocking_lock();

            // Process data (Heavy CPU work happens here)
            let new_signals = session.on_data(req, &state_clone.inner.predictor_cache, &state_clone.inner.session_manager.settings);

            // Add to history
            let mut history = state_clone.inner.signal_history.blocking_lock();
            let now = Utc::now().timestamp();

            for sig in new_signals {
                signals.push((sig.clone(), symbol.clone()));
                history.push_front(HistoricalSignal {
                    signal: ActiveSignal { symbol: symbol.clone(), signal: sig },
                    created_at: now,
                });
            }
            signals
        })
        .await
        .map_err(|e| {
            tracing::error!("Failed to join blocking task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // --- NEW: Send notifications AFTER releasing the lock ---
        let num_new_signals = signals_to_send.len();
        if num_new_signals > 0 {
            self.state.inner.total_signals_generated.fetch_add(num_new_signals, Ordering::SeqCst);
            self.state.inner.metrics.signal_counter.inc_by(num_new_signals as f64);
        }
        for (signal, symbol) in signals_to_send {
            self.send_push_notification(&signal, &symbol).await;    
        }

        Ok(())
    }

    /// Sends a push notification for a new signal to all registered devices.
    async fn send_push_notification(&self, signal: &EvalResponse, symbol: &str) {
        let tokens = self.state.inner.push_tokens.lock().await.clone();
        if tokens.is_empty() {
            return;
        }

        let messages: Vec<ExpoPushMessage> = tokens
            .into_iter()
            .map(|token| {
                let data = serde_json::json!({ "signalId": signal.signal_id, "symbol": symbol });
                ExpoPushMessage::builder(vec![token])
                    .title(format!("New {} Signal: {} {}", symbol, signal.classification.to_uppercase(), signal.entry_type.to_uppercase()))
                    .body(format!("Entry: {:.5}, SL: {:.5}, TP1: {:.5}, TP2: {:.5}", signal.entry_price, signal.sl_price, signal.tp1_price, signal.tp2_price))
                    .data(&data)
                    .and_then(|builder| builder.build())
            })
            .filter_map(Result::ok)
            .collect();

        let client = Expo::new(ExpoClientOptions::default());
        let _ = client.send_push_notifications(messages).await;
    }
}