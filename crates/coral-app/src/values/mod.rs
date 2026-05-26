//! App-owned value memory built from successful query results.
//!
//! This is deliberately stored above the engine. The engine executes SQL and
//! returns Arrow batches; the app decides which observed values are worth
//! keeping for future discovery.

mod extract;
mod manager;
mod recorder;
mod service;
mod store;
mod surface;

pub(crate) use manager::ValueMemoryManager;
pub(crate) use recorder::ValueMemoryRecorder;
pub(crate) use service::ValueService;
