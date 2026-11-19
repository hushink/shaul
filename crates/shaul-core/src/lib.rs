//! Core domain and engine for shaul.
//!
//! The main chain is:
//! `Link -> LinkParser -> Resource -> Router -> DownloadBackend`, driven by
//! [`DownloadEngine`] over [`Task`] / [`FileItem`] / segments.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Archive detection and safe extraction.
pub mod archive;
/// Download backend trait plus the request/outcome types exchanged with it.
pub mod backend;
/// Blocking facade over the asynchronous engine.
pub mod blocking;
/// Checksum algorithms, checksum parsing and file digest computation.
pub mod checksum;
/// Configuration files, sections and resolved download options.
pub mod config;
/// Asynchronous engine that drives tasks through parsers, backends and
/// finalizers.
pub mod engine;
/// Central error type and result alias.
pub mod error;
/// Progress event types and event sinks.
pub mod event;
/// Post-download verification and extraction pipeline.
pub mod finalizer;
/// Validated HTTP headers, authentication and request options.
pub mod header;
/// Raw user input and its validation.
pub mod link;
/// Link parser trait, registry and parse options.
pub mod parser;
/// File-name sanitization and safe path joining.
pub mod path;
/// Byte-range arithmetic used for segmentation and resume.
pub mod range;
/// Backend-agnostic descriptions of the resource to download.
pub mod resource;
/// Routing of links to parsers and resources to backends.
pub mod router;
/// Task persistence traits and the JSON sidecar store.
pub mod store;
/// Task, file and resume state bookkeeping.
pub mod task;
/// Numeric metrics hooks for external monitoring systems.
pub mod telemetry;
mod util;

pub use archive::{
    ArchiveExtractor, BuiltinExtractor, ExtractReport, ExtractorRegistry, ExtractorRequest,
};
pub use backend::{
    BackendCapabilities, DownloadBackend, DownloadOutcome, DownloadRequest, FileOutcome,
    ResumeCheckpoint,
};
pub use blocking::BlockingEngine;
pub use checksum::{Checksum, ChecksumAlgorithm, compute_file_checksum};
pub use config::{BtConfig, Config, DownloadConfig, HubConfig, NetworkConfig, ResolvedOptions};
pub use engine::DownloadEngine;
pub use error::{Error, Result};
pub use event::{DownloadEvent, EventKind, EventSink};
pub use finalizer::{
    ExtractFinalizer, ExtractorFinalizer, FinalizeContext, FinalizeResult, Finalizer,
    FinalizerPipeline, VerifyFinalizer,
};
pub use header::{Auth, Header, RequestOptions, expand_env};
pub use link::Link;
pub use parser::{LinkParser, ParseOptions, ParserRegistry};
pub use path::{safe_join, sanitize_file_name, sanitize_relative_path};
pub use range::{ByteRange, completed_bytes, merge_ranges, missing_ranges, split_range};
pub use resource::{
    BitTorrentSource, HubProvider, HubRepoType, Resource, ResourceFile, ResourceKind,
};
pub use router::{RoutedResource, Router, backend_arc, parser_arc};
pub use store::{SidecarStore, TaskStore, TaskUpdate};
pub use task::{
    FileItem, FileResume, FileState, Task, TaskId, TaskOptions, TaskResume, TaskState, Validator,
    VerifyMode, destination_file, part_path,
};
pub use telemetry::{NoopTelemetry, Telemetry, metrics};
pub use tokio_util::sync::CancellationToken;
