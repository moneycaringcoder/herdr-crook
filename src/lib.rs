//! Shared infrastructure for Herdr plugins.
//!
//! Crook owns protocol and environment mechanics. Plugins retain their domain
//! reducers, state machines, rendering content, and policy.

pub mod client;
pub mod env;
#[cfg(unix)]
pub mod fs;
pub mod rpc;
pub mod snapshot;
#[cfg(all(feature = "test-support", unix))]
pub mod test_support;
