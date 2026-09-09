//! Running the router as a user service (ADR-0006).

pub mod status;
pub mod systemd;
pub mod transient;

#[cfg(test)]
mod tests;
