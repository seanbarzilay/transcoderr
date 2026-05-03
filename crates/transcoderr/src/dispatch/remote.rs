//! `RemoteRunner` — opens `step_dispatch` over the worker's WS,
//! awaits `step_complete`, maps `step_progress` to the engine's
//! on_progress callback. Filled in Task 9.
