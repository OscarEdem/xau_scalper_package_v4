use crate::indicators::MacroCategory;

pub fn classify_event(title: &str) -> MacroCategory {
    let t = title.to_lowercase();

    if t.contains("interest rate")
        || t.contains("fomc")
        || t.contains("rate decision")
        || t.contains("central bank")
    {
        MacroCategory::MonetaryPolicy
    } else if t.contains("cpi")
        || t.contains("inflation")
        || t.contains("ppi")
    {
        MacroCategory::Inflation
    } else if t.contains("employment")
        || t.contains("nfp")
        || t.contains("unemployment")
        || t.contains("job")
    {
        MacroCategory::Labor
    } else if t.contains("gdp")
        || t.contains("pmi")
        || t.contains("manufacturing")
        || t.contains("services")
    {
        MacroCategory::Growth
    } else if t.contains("risk")
        || t.contains("confidence")
    {
        MacroCategory::Risk
    } else {
        MacroCategory::Other
    }
}