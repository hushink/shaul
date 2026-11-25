//! DASH support for shaul.
//!
//! [`DashParser`] turns HTTP(S) `.mpd` links into a single
//! [`ResourceKind::Custom`](shaul_core::ResourceKind::Custom) resource, and
//! [`DashBackend`] downloads and concatenates the selected representation's
//! initialization segment and media segments into one output file. Segments
//! are an internal detail: the task still produces one
//! [`shaul_core::FileOutcome`].
//!
//! Supports static VOD manifests with one or more Periods, SegmentTemplate,
//! SegmentList or SegmentBase/BaseURL addressing and segment-level resume.
//! Static multi-Period lists concatenate only when MIME/codecs match and every
//! Period has no init or the same init. Bounded dynamic (live) captures use
//! SegmentTemplate + SegmentTimeline or a constant-duration SegmentTemplate;
//! dynamic captures are not resumable. Dynamic multi-Period manifests, DRM,
//! low-latency `availabilityTimeOffset`, subtitles, alternate audio and
//! remuxing are not supported.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// DASH download backend.
pub mod backend;
/// HTTP client used for MPD documents.
pub mod client;
/// MPD parsing and representation selection.
pub mod mpd;
/// Parser for HTTP(S) `.mpd` links.
pub mod parser;

pub use backend::DashBackend;
pub use client::DashClient;
pub use mpd::{
    DashManifest, DynamicTemplate, RepresentationPart, SegmentRef, SegmentTiming,
    SelectedRepresentation,
};
pub use parser::{DASH_PROTOCOL, DashParser};

use std::sync::Arc;

use shaul_core::{Config, ParserRegistry, Result, Router, backend_arc, parser_arc};

/// Build a router containing only the DASH parser and backend.
pub fn default_router(config: &Config) -> Result<Router> {
    let client = Arc::new(DashClient::new(config)?);
    let http = shaul_http::HttpBackend::new(config)?;
    let parsers = ParserRegistry::new().with(parser_arc(DashParser::new(client.clone())));
    Ok(Router::new(
        parsers,
        vec![backend_arc(DashBackend::new(client, http))],
    ))
}
