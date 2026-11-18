//! Core domain and engine for shaul.
//!
//! The main chain is:
//! `Link -> LinkParser -> Resource -> Router -> DownloadBackend`, driven by
//! [`DownloadEngine`] over [`Task`] / [`FileItem`] / segments.

pub mod archive;
pub mod backend;
pub mod blocking;
pub mod checksum;
pub mod config;
pub mod engine;
pub mod error;
pub mod event;
pub mod finalizer;
pub mod header;
pub mod link;
pub mod parser;
pub mod path;
pub mod range;
pub mod resource;
pub mod router;
pub mod store;
pub mod task;
pub mod util;

pub use backend::{
    BackendCapabilities, DownloadBackend, DownloadOutcome, DownloadRequest, FileOutcome,
    ResumeCheckpoint,
};
pub use blocking::BlockingEngine;
pub use checksum::{Checksum, ChecksumAlgorithm, compute_file_checksum};
pub use config::{Config, ResolvedOptions};
pub use engine::DownloadEngine;
pub use error::{Error, Result};
pub use event::{DownloadEvent, EventKind, EventSink};
pub use finalizer::{FinalizeContext, FinalizeResult, Finalizer, FinalizerPipeline};
pub use header::{Auth, Header, RequestOptions, expand_env};
pub use link::Link;
pub use parser::{LinkParser, ParseOptions, ParserRegistry};
pub use path::{safe_join, sanitize_file_name, sanitize_relative_path};
pub use range::{ByteRange, completed_bytes, merge_ranges, missing_ranges, split_range};
pub use resource::{
    BitTorrentSource, HubProvider, HubRepoType, Resource, ResourceFile, ResourceKind,
};
pub use router::{RoutedResource, Router, backend_arc, parser_arc};
pub use store::{SidecarStore, TaskStore};
pub use task::{
    FileItem, FileResume, FileState, Task, TaskId, TaskOptions, TaskResume, TaskState, Validator,
    VerifyMode, destination_file, part_path,
};
