//! Persistent request statistics: one JSON line per `/v1/messages` exchange,
//! aggregated on demand (ADR-0011).

mod aggregate;
mod log;
mod record;

#[cfg(test)]
mod tests;

pub use aggregate::{Range, Report, Row, aggregate};
pub use log::StatsLog;
pub use record::{SCHEMA, StatsRecord};
