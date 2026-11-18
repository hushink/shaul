//! BitTorrent parser and leech-only backend for shaul.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Leech-only BitTorrent backend backed by librqbit.
pub mod backend;
/// Parser for magnet links, info hashes and local `.torrent` files.
pub mod parser;

pub use backend::BtBackend;
pub use parser::TorrentParser;
