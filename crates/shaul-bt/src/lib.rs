//! BitTorrent parser and leech-only backend for shaul.

pub mod backend;
pub mod parser;

pub use backend::BtBackend;
pub use parser::TorrentParser;
