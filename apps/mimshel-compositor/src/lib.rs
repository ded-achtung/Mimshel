pub mod backend;
pub mod input;
pub mod model;
pub mod render;
pub mod state;

pub type Error = Box<dyn std::error::Error>;
