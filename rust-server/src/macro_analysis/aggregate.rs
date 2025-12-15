use std::collections::HashMap;
use crate::indicators::NewsEvent;
use super::scoring::{score_event, MacroScore};

pub fn aggregate_scores(events: &[NewsEvent]) -> HashMap<String, MacroScore> {
    let mut map = HashMap::new();

    for event in events {
        let entry = map.entry(event.currency.clone())
            .or_insert_with(MacroScore::default);

        let s = score_event(event);

        entry.hawkish += s.hawkish;
        entry.dovish += s.dovish;
        entry.risk_off += s.risk_off;
    }

    map
}