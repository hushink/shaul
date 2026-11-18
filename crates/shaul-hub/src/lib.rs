//! Hugging Face and ModelScope link parsers and download backends.

pub mod backend;
pub mod cache;
pub mod client;
pub mod parser;

pub use backend::HubBackend;
pub use client::{HubClient, HubEntry};
pub use parser::{HuggingFaceParser, ModelScopeParser};
