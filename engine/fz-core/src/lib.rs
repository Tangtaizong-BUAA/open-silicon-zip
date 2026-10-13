//! fz-core: FastZip concurrency-first archive engine.
//!
//! Staged pipeline (enumerate -> read -> codec -> write -> finalize) with
//! bounded channels, work-stealing codec pool and cooperative cancellation.
pub mod cancel;
pub mod codepage;
pub mod enumerate;
pub mod error;
pub mod formats;
pub mod hash;
mod limits;
pub mod pathsafe;
pub mod recover;
pub mod smart;
pub mod skiplist;
pub mod sys;
pub mod zip;

pub use cancel::{CancelToken, Progress};
pub use error::{Error, ErrorKind, Result};
