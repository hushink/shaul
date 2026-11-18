//! Hugging Face and ModelScope link parsers and download backends.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Hub download backend.
pub mod backend;
mod cache;
/// Hugging Face and ModelScope API client.
pub mod client;
/// Parsers for Hub repository and file links.
pub mod parser;

pub use backend::HubBackend;
pub use client::{HubClient, HubEntry};
pub use parser::{HuggingFaceParser, ModelScopeParser};
