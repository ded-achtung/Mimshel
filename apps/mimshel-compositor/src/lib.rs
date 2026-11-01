#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
//! Nested Wayland compositor with protocol-independent window policy.

/// Backend implementations.
pub mod backend;

/// Title bar drawing and hit-testing.
pub mod decoration;

/// Forward host input and update focus and interactive drags.
pub mod input;

/// Window policy.
pub mod model;

/// Damage-based rendering.
pub mod render;

/// Shell protocol policy.
pub mod protocols;

/// Wayland protocol state.
pub mod state;

/// Error propagated to the compositor entry point.
pub type Error = Box<dyn std::error::Error>;

/// XWayland process and X11 window policy.
pub mod xwayland;

mod selection;

mod keyboard;
