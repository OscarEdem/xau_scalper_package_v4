use std::sync::Arc;
use tokio::fs;
use tokio::sync::broadcast;
use chrono::Utc;
use tracing::info;
use crate::fetch_calendar_events;
use crate::state::ApplicationStateWithTicks;

/// Spawns the background task for cleaning up stale signals in active sessions.
pub fn spawn_stale_signal_cleanup_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Stale signal cleanup task shutting down.");
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(60)) => {}
            }

            let now = Utc::now().timestamp();
            const STALE_THRESHOLD_SECONDS: i64 = 300; // 5 minutes

            let sessions = &state.inner.session_manager.sessions;
            let mut stale_symbols = Vec::new();
            for r in sessions.iter() {
                let symbol = r.key();
                let session_arc = r.value();
                let session = session_arc.lock().await;
                if (now - session.get_last_eval_timestamp()) > STALE_THRESHOLD_SECONDS {
                    stale_symbols.push(symbol.clone());
                }
            }

            for symbol in stale_symbols {
                if let Some(session_arc) = sessions.get(&symbol) {
                    let mut session = session_arc.lock().await;
                    session.invalidate_signals();
                    info!("Invalidated stale signals for symbol: {}", symbol);
                }
            }
        }
    });
}

/// Spawns the background task for cleaning up old signal history.
pub fn spawn_history_cleanup_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("History cleanup task shutting down.");
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(3600)) => {}
            }

            let now = Utc::now().timestamp();
            const TWELVE_HOURS_IN_SECONDS: i64 = 12 * 60 * 60;

            let mut history = state.inner.signal_history.lock().await;
            let original_len = history.len();
            history.retain(|hs| (now - hs.created_at) < TWELVE_HOURS_IN_SECONDS);
            let removed_count = original_len - history.len();
            if removed_count > 0 {
                info!("Removed {} signals from history older than 12 hours.", removed_count);
            }
        }
    });
}

/// Spawns the background task for fetching news events.
pub fn spawn_news_fetch_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        // Fetch immediately on startup
        info!("Performing initial fetch of weekly news events from Forex Factory...");
        let initial_events = fetch_calendar_events().await;
        *state.inner.news_events.lock().await = initial_events;

        loop {
            // Adjust interval: If we have no news (failed fetch), retry sooner (e.g., 5 mins).
            let fetch_interval_secs = if state.inner.news_events.lock().await.is_empty() {
                300
            } else {
                6 * 3600
            };

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("News fetch task shutting down.");
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(fetch_interval_secs)) => {}
            }

            info!("Periodically fetching weekly news events from Forex Factory...");
            let events = fetch_calendar_events().await;
            // Only update if we actually got new events, to avoid clearing on a failed fetch
            if !events.is_empty() {
                *state.inner.news_events.lock().await = events;
            }
        }
    });
}

/// Spawns the background task for persisting sessions to disk.
pub fn spawn_session_persistence_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        let sessions_dir = state.inner.config.paths.sessions_dir.clone();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Session persistence task shutting down.");
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(60)) => {}
            }

            let _ = fs::create_dir_all(&sessions_dir).await;

            for r in state.inner.session_manager.sessions.iter() {
                let symbol = r.key();
                let session_arc = r.value();
                let session = session_arc.lock().await;
                let json_res = serde_json::to_string(&*session);

                if let Ok(json) = json_res {
                    // Atomic Write: Write to .tmp first, then rename.
                    let tmp_path = format!("{}/{}.tmp", sessions_dir, symbol);
                    let file_path = format!("{}/{}.json", sessions_dir, symbol);
                    if fs::write(&tmp_path, json).await.is_ok() {
                        let _ = fs::rename(&tmp_path, &file_path).await;
                    }
                }
            }
        }
    });
}