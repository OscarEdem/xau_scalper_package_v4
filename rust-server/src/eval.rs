use crate::engines::scalp::ScalpEngine;
use crate::engines::swing::SwingEngine;
use crate::{EvalRequest, EvalResponse};

pub fn evaluate_signal(req: &EvalRequest) -> EvalResponse {
    match req.mode.as_str() {
        "scalp" => ScalpEngine::evaluate(req),
        "swing" => SwingEngine::evaluate(req),
        _ => EvalResponse {
            reason: format!("Unknown evaluation mode: '{}'", req.mode),
            ..Default::default()
        },
    }
}
