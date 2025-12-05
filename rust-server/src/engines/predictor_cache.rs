use super::predictor::{self, Predictor};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::info;

type PredictorKey = (String, String);

/// A thread-safe, caching loader for predictor models.
///
/// This struct holds predictor models in memory after their first load,
/// preventing repeated and expensive file I/O and deserialization on every request.
#[derive(Clone, Default)]
pub struct PredictorCache {
    // Arc<dyn Predictor> is used because the trait object needs to be shareable across threads.
    cache: Arc<DashMap<PredictorKey, Arc<dyn Predictor>>>,
}

impl PredictorCache {
    /// Retrieves a predictor from the cache. If not present, it loads the model
    /// from disk, inserts it into the cache, and then returns it.
    ///
    /// The returned predictor is wrapped in an `Arc` for shared ownership.
    pub fn get_or_load(&self, model_type: &str, timeframe: &str) -> Arc<dyn Predictor> {
        let key = (model_type.to_string(), timeframe.to_string());

        // Fast path: if the predictor is already in the cache, clone its Arc and return.
        if let Some(predictor) = self.cache.get(&key) {
            return predictor.clone();
        }

        // Slow path: predictor not in cache. Load it from disk.
        info!(model_type, timeframe, "Loading and caching new predictor model.");
        let predictor = predictor::load_predictor(model_type, timeframe);
        let predictor_arc = Arc::from(predictor);
        self.cache.insert(key, predictor_arc.clone());
        predictor_arc
    }
}