use super::predictor::{load_predictor, Predictor};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::info;

/// A thread-safe, concurrent cache for predictor models.
///
/// This struct holds initialized predictor models in memory to avoid the overhead
/// of loading them from disk on every request. It uses a `DashMap` for efficient,
/// lock-free reads and locked writes.
#[derive(Clone)]
pub struct PredictorCache {
    /// The cache stores `Arc<Box<dyn Predictor>>` to allow shared, immutable access
    /// to the models across multiple threads. The key is a string combination
    /// of model type and timeframe, e.g., "lstm_h1".
    cache: Arc<DashMap<String, Arc<Box<dyn Predictor>>>>,
    models_dir: String,
}

impl Default for PredictorCache {
    fn default() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            models_dir: "./models/".to_string(),
        }
    }
}

impl PredictorCache {
    pub fn new(models_dir: String) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            models_dir,
        }
    }
}

impl PredictorCache {
    /// Retrieves a predictor from the cache. If the predictor is not in the cache,
    /// it loads the model, inserts it into the cache, and then returns it.
    ///
    /// This ensures that each model is loaded only once.
    pub fn get_or_load(&self, model_type: &str, timeframe: &str) -> Arc<Box<dyn Predictor>> {
        let key = format!("{}_{}", model_type, timeframe);

        // First, try to get the predictor with a read-only lock.
        if let Some(predictor) = self.cache.get(&key) {
            return predictor.clone();
        }

        // If not found, we'll need to load it.
        // The `entry` API of DashMap handles the complexity of concurrent inserts.
        // The closure inside `or_insert_with` is only executed if the key is truly absent.
        self.cache
            .entry(key.clone())
            .or_insert_with(|| {
                info!("Cache miss for '{}'. Loading model...", key);
                match load_predictor(model_type, timeframe, &self.models_dir) {
                    Ok(predictor) => Arc::new(predictor),
                    Err(e) => {
                        tracing::error!("Failed to load predictor model: {:?}", e);
                        // Insert a no-op predictor so we don't panic and poison locks.
                        Arc::new(Box::new(super::predictor::NoopPredictor::default()) as Box<dyn Predictor>)
                    }
                }
            })
            .clone()
    }

    /// Returns a list of keys currently present in the cache (e.g. "gbm_h1").
    pub fn loaded_keys(&self) -> Vec<String> {
        self.cache.iter().map(|r| r.key().clone()).collect()
    }
}