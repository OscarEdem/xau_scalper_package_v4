# TODO: Implement ScalpEngine and SwingEngine

## Completed
- [ ] Analyze codebase and create plan
- [ ] Get user approval for plan

## In Progress
- [ ] Update indicators.rs: Add mode, current_price to EvalRequest; classification to EvalResponse
- [ ] Create src/engines/mod.rs
- [ ] Create src/engines/scalp.rs with ScalpEngine logic
- [ ] Create src/engines/swing.rs with SwingEngine logic
- [ ] Create src/eval.rs with evaluate_signal function
- [ ] Update lib.rs: Add pub mod eval; pub mod engines;
- [ ] Update strategy.rs: Replace evaluate_strategy with evaluate_signal call

## Followup
- [ ] Test compilation
- [ ] Verify logic with sample data
- [ ] Ensure no modifications to existing indicator functions
