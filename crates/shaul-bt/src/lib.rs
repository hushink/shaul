//! BitTorrent parser and leech-only backend for shaul.

#![forbid(unsafe_code)]

pub mod backend;
pub mod parser;

pub use backend::BtBackend;
pub use parser::TorrentParser;
