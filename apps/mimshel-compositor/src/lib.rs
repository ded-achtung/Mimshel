#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
//! Nested Wayland compositor with protocol-independent window policy.

/// Backend implementations.
pub mod backend;

/// Forward host input and update focus and interactive drags.
pub mod input;

/// Window policy.
pub mod model;

/// Damage-based rendering.
pub mod render;

/// Wayland protocol state.
pub mod state;

/// Error propagated to the compositor entry point.
pub type Error = Box<dyn std::error::Error>;
