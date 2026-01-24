use std::sync::Arc;
use tokio::sync::broadcast;
use chrono::Utc;
use tracing::info;
use crate::state::ApplicationStateWithTicks;

/// Spawns the background task for cleaning up stale signals in active sessions.
pub fn spawn_stale_signal_cleanup_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!(event = "shutdown", task = "stale_signal_cleanup", "Stale signal cleanup task shutting down.");
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
                    if session.invalidate_signals() {
                        info!(event = "stale_invalidation", symbol = %symbol, "Invalidated stale signals for symbol: {}", symbol);
                    }
                }
            }
        }
    });
}

/// Spawns the background task for cleaning up old signal history.
pub fn spawn_history_cleanup_task(_state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!(event = "shutdown", task = "history_cleanup", "History cleanup task shutting down.");
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(3600)) => {}
            }

            // Automatic 12-hour cleanup is disabled per user request.
            // This task now just sleeps to keep the structure intact or can be removed entirely.
            // We keep it running but doing nothing to avoid breaking main.rs calls.
            // info!(event = "history_cleanup", "Skipping automatic cleanup (disabled).");
        }
    });
}

/// Spawns the background task for persisting sessions to disk.
pub fn spawn_session_persistence_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!(event = "shutdown", task = "session_persistence", "Session persistence task shutting down.");
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(300)) => {}
            }

            for r in state.inner.session_manager.sessions.iter() {
                let session_arc = r.value();
                let session = session_arc.lock().await;
                
                if let Err(e) = crate::db::save_session(&state.inner.db, &session, Some(&state.inner.metrics.db_retries_total)).await {
                    tracing::error!("Failed to persist session for {}: {}", session.symbol, e);
                }
            }
        }
    });
}

/// Spawns the background task for cleaning up old signals from the database (older than 1 year).
pub fn spawn_db_cleanup_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!(event = "shutdown", task = "db_cleanup", "DB cleanup task shutting down.");
                    break;
                }
                // Run once a day (86400 seconds)
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(86400)) => {}
            }

            info!(event = "db_cleanup_start", "Starting database cleanup for old signals...");
            match xau_scalper_server::db::cleanup_old_signals(&state.inner.db, Some(&state.inner.metrics.db_retries_total)).await {
                Ok(count) => info!(event = "db_cleanup_success", count = count, "Deleted {} old signals.", count),
                Err(e) => tracing::error!(event = "db_cleanup_error", error = %e, "Failed to cleanup old signals."),
            }
        }
    });
}