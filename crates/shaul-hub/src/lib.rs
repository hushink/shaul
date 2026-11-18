//! Hugging Face and ModelScope link parsers and download backends.

#![forbid(unsafe_code)]

pub mod backend;
mod cache;
pub mod client;
pub mod parser;

pub use backend::HubBackend;
pub use client::{HubClient, HubEntry};
pub use parser::{HuggingFaceParser, ModelScopeParser};
