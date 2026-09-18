//! Maps the `model` a client names to a backend and the model name that
//! backend understands (ADR-0003).

mod live;
mod registry;

#[cfg(test)]
mod tests;

pub use live::LiveCatalog;
pub use registry::{Match, Registry, Resolution, Route};

/// A request hit the configured table or a live passthrough id.
pub enum Routed<'a> {
    Config(Resolution<'a>),
    Live(Route),
}

impl<'a> Routed<'a> {
    pub fn route(&self) -> &Route {
        match self {
            Self::Config(resolution) => resolution.route,
            Self::Live(route) => route,
        }
    }

    pub fn matched(&self) -> Match {
        match self {
            Self::Config(resolution) => resolution.matched,
            Self::Live(_) => Match::Exact,
        }
    }
}
