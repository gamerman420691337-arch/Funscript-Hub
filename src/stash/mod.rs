//! Stash media server integration (GraphQL query client and automatic script linker).

pub mod client;

#[allow(unused_imports)]
pub use client::{build_missing_scenes_query, StashClient, StashConfig, StashFile, StashScene};
