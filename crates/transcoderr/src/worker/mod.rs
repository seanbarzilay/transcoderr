//! Worker module. Pre-distributed-transcoding (Piece 1) this just held
//! the in-process job-claim pool at `pool.rs`. The Piece 1 wire
//! protocol skeleton adds `daemon.rs`, `connection.rs`, `protocol.rs`,
//! and `config.rs` as siblings; later pieces wire the local pool
//! through the same registration mechanism remote workers use.
//!
//! `pool::*` is re-exported so existing `use crate::worker::Worker`
//! callsites keep resolving without churn.

pub mod config;
pub mod connection;
pub mod daemon;
pub mod local;
pub mod pool;
pub mod protocol;

pub use pool::*;
