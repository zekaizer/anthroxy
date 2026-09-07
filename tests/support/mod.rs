//! Shared harness: a scriptable mock backend and a router bound to an
//! ephemeral port.

#![allow(dead_code, unused_imports)]

pub mod mock_upstream;
pub mod router;
pub mod tls;

pub use mock_upstream::MockUpstream;
pub use router::TestRouter;
