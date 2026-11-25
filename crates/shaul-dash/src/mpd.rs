use std::collections::BTreeMap;

use globset::{Glob, GlobSet, GlobSetBuilder};
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};
use sha2::{Digest, Sha256};
use shaul_core::{ByteRange, Error, Result};
use url::Url;

const MAX_SEGMENTS: u64 = 1_000_000;

/// A parsed DASH manifest.
#[derive(Clone, Debug)]
pub struct DashManifest {
    media_duration: Option<f64>,
    dynamic: bool,
    minimum_update_period: Option<f64>,
    periods: Vec<Period>,
}

/// One initialization or media segment selected from a representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentRef {
    /// Absolute segment URL.
    pub url: Url,
    /// Optional exact byte range within `url`.
    pub range: Option<ByteRange>,
    /// Presentation timing from a SegmentTemplate, when known.
    pub timing: Option<SegmentTiming>,
}

/// Timing of one template-expanded segment, in `timescale` units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentTiming {
    /// Presentation start time.
    pub start: u64,
    /// Duration in timescale units.
    pub duration: u64,
    /// Timescale of `start` and `duration`.
    pub timescale: u64,
}

impl SegmentTiming {
    /// Duration in seconds.
    pub fn seconds(&self) -> f64 {
        self.duration as f64 / self.timescale as f64
    }

    /// Exclusive presentation end time.
    pub fn end(&self) -> Option<u64> {
        self.start.checked_add(self.duration)
    }
}

/// The representation chosen for a DASH download.
#[derive(Clone, Debug)]
pub struct SelectedRepresentation {
    /// Representation `id`.
    pub id: String,
    /// Declared bandwidth in bits per second.
    pub bandwidth: Option<u64>,
    /// Effective MIME type.
    pub mime_type: Option<String>,
    /// Effective codecs string.
    pub codecs: Option<String>,
    /// Default output extension (`mp4` or `webm`).
    pub extension: &'static str,
    /// Whether the manifest was dynamic (`type="dynamic"`).
    pub live: bool,
    /// `minimumUpdatePeriod` in seconds for dynamic manifests.
    pub minimum_update_period_secs: Option<f64>,
    /// Optional initialization segment.
    pub initialization: Option<SegmentRef>,
    /// Media segments in presentation order.
    pub segments: Vec<SegmentRef>,
}

#[derive(Clone, Debug)]
struct Period {
    duration: Option<f64>,
    adaptation_sets: Vec<AdaptationSet>,
}

#[derive(Clone, Debug)]
struct AdaptationSet {
    id: Option<String>,
    content_type: Option<String>,
    mime_type: Option<String>,
    codecs: Option<String>,
    lang: Option<String>,
    width: Option<u64>,
    height: Option<u64>,
    template: Option<Template>,
    list: Option<SegmentList>,
    representations: Vec<Representation>,
}

#[derive(Clone, Debug)]
struct Representation {
    id: String,
    bandwidth: Option<u64>,
    mime_type: Option<String>,
    codecs: Option<String>,
    lang: Option<String>,
    width: Option<u64>,
    height: Option<u64>,
    base_url: Url,
    has_base: bool,
    template: Option<Template>,
    list: Option<SegmentList>,
}

#[derive(Clone, Debug, Default)]
struct Template {
    attributes: BTreeMap<String, String>,
    has_timeline: bool,
    timeline: Vec<TimelineEntry>,
}

#[derive(Clone, Debug)]
struct TimelineEntry {
    t: Option<u64>,
    d: u64,
    r: u64,
}

#[derive(Clone, Debug, Default)]
struct SegmentList {
    initialization: Option<(Option<String>, Option<String>)>,
    urls: Vec<(Option<String>, Option<String>)>,
}

#[derive(Clone, Debug)]
struct Node {
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Node>,
    text: String,
}

impl DashManifest {
    /// Parse an MPD document fetched from `url`.
    pub fn parse(url: &Url, text: &str) -> Result<Self> {
        let root = parse_xml(text)?;
        if root.name != "MPD" {
            return Err(Error::Backend("DASH manifest root is not MPD".to_string()));
        }
        if root.attributes.contains_key("href") {
            return Err(Error::Backend(
                "DASH xlink:href is not supported".to_string(),
            ));
        }
        let manifest_type = root.attr("type").unwrap_or("static");
        let dynamic = match manifest_type {
            "static" => false,
            "dynamic" => true,
            other => {
                return Err(Error::Backend(format!(
                    "unsupported DASH manifest type: {other}"
                )));
            }
        };
        let media_duration = root
            .attr("mediaPresentationDuration")
            .map(parse_iso_duration)
            .transpose()?;
        let minimum_update_period = root
            .attr("minimumUpdatePeriod")
            .map(parse_iso_duration)
            .transpose()?;
        let base_url = resolve_base(url, &root)?;
        let has_base = root.child("BaseURL").is_some();

        let period_nodes: Vec<&Node> = root.children("Period").collect();
        if period_nodes.len() != 1 {
            return Err(Error::Backend(format!(
                "DASH manifests must contain exactly one Period (found {})",
                period_nodes.len()
            )));
        }
        let period_node = period_nodes[0];
        if period_node.attributes.contains_key("href") {
            return Err(Error::Backend(
                "DASH xlink:href is not supported".to_string(),
            ));
        }
        let period_duration = period_node
            .attr("duration")
            .map(parse_iso_duration)
            .transpose()?;
        let period_base = resolve_base(&base_url, period_node)?;
        let period_has_base = has_base || period_node.child("BaseURL").is_some();

        let mut adaptation_sets = Vec::new();
        for set_node in period_node.children("AdaptationSet") {
            if set_node.attributes.contains_key("href") {
                return Err(Error::Backend(
                    "DASH xlink:href is not supported".to_string(),
                ));
            }
            if set_node.child("ContentProtection").is_some() {
                return Err(Error::Backend(
                    "DASH ContentProtection (DRM) is not supported".to_string(),
                ));
            }
            let set_base = resolve_base(&period_base, set_node)?;
            let set_has_base = period_has_base || set_node.child("BaseURL").is_some();
            let template = parse_template(set_node)?;
            let list = parse_segment_list(set_node)?;

            let mut representations = Vec::new();
            for rep_node in set_node.children("Representation") {
                if rep_node.attributes.contains_key("href") {
                    return Err(Error::Backend(
                        "DASH xlink:href is not supported".to_string(),
                    ));
                }
                if rep_node.child("ContentProtection").is_some() {
                    return Err(Error::Backend(
                        "DASH ContentProtection (DRM) is not supported".to_string(),
                    ));
                }
                let id = rep_node.attr("id").ok_or_else(|| {
                    Error::Backend("DASH Representation is missing required id".to_string())
                })?;
                let rep_base = resolve_base(&set_base, rep_node)?;
                representations.push(Representation {
                    id: id.to_string(),
                    bandwidth: parse_u64_attr(rep_node, "bandwidth")?,
                    mime_type: rep_node.attr("mimeType").map(str::to_string),
                    codecs: rep_node.attr("codecs").map(str::to_string),
                    lang: rep_node.attr("lang").map(str::to_string),
                    width: parse_u64_attr(rep_node, "width")?,
                    height: parse_u64_attr(rep_node, "height")?,
                    base_url: rep_base,
                    has_base: set_has_base || rep_node.child("BaseURL").is_some(),
                    template: parse_template(rep_node)?,
                    list: parse_segment_list(rep_node)?,
                });
            }
            adaptation_sets.push(AdaptationSet {
                id: set_node.attr("id").map(str::to_string),
                content_type: set_node.attr("contentType").map(str::to_string),
                mime_type: set_node.attr("mimeType").map(str::to_string),
                codecs: set_node.attr("codecs").map(str::to_string),
                lang: set_node.attr("lang").map(str::to_string),
                width: parse_u64_attr(set_node, "width")?,
                height: parse_u64_attr(set_node, "height")?,
                template,
                list,
                representations,
            });
        }
        if adaptation_sets.is_empty() {
            return Err(Error::Backend(
                "DASH manifest contains no AdaptationSet".to_string(),
            ));
        }

        Ok(Self {
            media_duration,
            dynamic,
            minimum_update_period,
            periods: vec![Period {
                duration: period_duration,
                adaptation_sets,
            }],
        })
    }

    /// Select the best audio/video representation that matches `include` and
    /// does not match `exclude`. Video is preferred over audio; higher
    /// bandwidth wins within the same category.
    pub fn select(&self, include: &[String], exclude: &[String]) -> Result<SelectedRepresentation> {
        let include = glob_set(include)?;
        let exclude = glob_set(exclude)?;
        let period = &self.periods[0];
        let total_duration = period.duration.or(self.media_duration);
        let mut best: Option<(&AdaptationSet, &Representation, u8, u64)> = None;
        for set in &period.adaptation_sets {
            for representation in &set.representations {
                let Some(category) = media_category(set, representation) else {
                    continue;
                };
                if matches_any(&exclude, set, representation) {
                    continue;
                }
                if !include.is_empty() && !matches_any(&include, set, representation) {
                    continue;
                }
                let bandwidth = representation.bandwidth.unwrap_or(0);
                let better = best.is_none_or(|(_, _, best_category, best_bandwidth)| {
                    category < best_category
                        || (category == best_category && bandwidth > best_bandwidth)
                });
                if better {
                    best = Some((set, representation, category, bandwidth));
                }
            }
        }
        let Some((set, representation, _, _)) = best else {
            return Err(Error::InvalidLink(
                "no DASH representation matched --include/--exclude".to_string(),
            ));
        };
        resolve_selected(
            set,
            representation,
            total_duration,
            self.dynamic,
            self.minimum_update_period,
        )
    }

    /// Resolve a previously selected representation by its stable `id`.
    /// Dynamic live captures use this so a newly added higher-bandwidth
    /// representation does not change the recording mid-stream.
    pub fn select_representation(&self, id: &str) -> Result<SelectedRepresentation> {
        let period = &self.periods[0];
        let total_duration = period.duration.or(self.media_duration);
        for set in &period.adaptation_sets {
            for representation in &set.representations {
                if representation.id == id {
                    return resolve_selected(
                        set,
                        representation,
                        total_duration,
                        self.dynamic,
                        self.minimum_update_period,
                    );
                }
            }
        }
        Err(Error::Backend(format!(
            "DASH representation {id} disappeared from the manifest"
        )))
    }

    /// Whether the manifest was declared with `type="dynamic"`.
    pub fn is_dynamic(&self) -> bool {
        self.dynamic
    }
}

/// Return a stable SHA-256 fingerprint for an MPD body.
pub fn fingerprint(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn resolve_selected(
    set: &AdaptationSet,
    representation: &Representation,
    total_duration: Option<f64>,
    dynamic: bool,
    minimum_update_period: Option<f64>,
) -> Result<SelectedRepresentation> {
    let template = merge_template(set.template.as_ref(), representation.template.as_ref());
    let list = merge_segment_list(set.list.as_ref(), representation.list.as_ref());
    if dynamic {
        let Some(template) = template
            .as_ref()
            .filter(|template| template.attributes.contains_key("media"))
        else {
            return Err(Error::Backend(
                "dynamic DASH requires a SegmentTemplate with media".to_string(),
            ));
        };
        if !template.has_timeline {
            return Err(Error::Backend(
                "dynamic DASH SegmentTemplate requires SegmentTimeline".to_string(),
            ));
        }
    }
    let (initialization, segments) = if let Some(template) = &template
        && template.attributes.contains_key("media")
    {
        resolve_template(
            &representation.base_url,
            representation,
            template,
            total_duration,
            dynamic,
        )?
    } else if let Some(list) = &list
        && !list.urls.is_empty()
    {
        resolve_segment_list(&representation.base_url, list)?
    } else {
        resolve_base_object(representation)?
    };

    if segments.is_empty() && !dynamic {
        return Err(Error::Backend(format!(
            "DASH representation {} has no media segments",
            representation.id
        )));
    }
    let mime_type = representation
        .mime_type
        .clone()
        .or_else(|| set.mime_type.clone());
    let codecs = representation.codecs.clone().or_else(|| set.codecs.clone());
    let extension = if mime_type
        .as_deref()
        .is_some_and(|mime| mime.contains("webm"))
    {
        "webm"
    } else {
        "mp4"
    };
    Ok(SelectedRepresentation {
        id: representation.id.clone(),
        bandwidth: representation.bandwidth,
        mime_type,
        codecs,
        extension,
        live: dynamic,
        minimum_update_period_secs: dynamic.then_some(minimum_update_period).flatten(),
        initialization,
        segments,
    })
}

fn resolve_template(
    base: &Url,
    representation: &Representation,
    template: &Template,
    total_duration: Option<f64>,
    dynamic: bool,
) -> Result<(Option<SegmentRef>, Vec<SegmentRef>)> {
    let media = template
        .attributes
        .get("media")
        .ok_or_else(|| Error::Backend("SegmentTemplate is missing media".to_string()))?;
    let timescale = template
        .attributes
        .get("timescale")
        .map(|value| parse_u64(value, "timescale"))
        .transpose()?
        .unwrap_or(1);
    if timescale == 0 {
        return Err(Error::Backend(
            "SegmentTemplate timescale must be greater than zero".to_string(),
        ));
    }
    let start_number = template
        .attributes
        .get("startNumber")
        .map(|value| parse_u64(value, "startNumber"))
        .transpose()?
        .unwrap_or(1);
    let time_offset = template
        .attributes
        .get("presentationTimeOffset")
        .map(|value| parse_u64(value, "presentationTimeOffset"))
        .transpose()?
        .unwrap_or(0);
    let duration = template
        .attributes
        .get("duration")
        .map(|value| parse_u64(value, "duration"))
        .transpose()?;

    let initialization = template
        .attributes
        .get("initialization")
        .map(|value| {
            let expanded = expand_template(
                value,
                &representation.id,
                representation.bandwidth,
                start_number,
                time_offset,
            )?;
            Ok::<SegmentRef, Error>(SegmentRef {
                url: join_url(base, &expanded)?,
                range: None,
                timing: None,
            })
        })
        .transpose()?;

    let mut segments = Vec::new();
    if template.has_timeline && template.timeline.is_empty() {
        if !dynamic {
            return Err(Error::Backend(
                "SegmentTimeline has no S entries".to_string(),
            ));
        }
    } else if !template.timeline.is_empty() {
        let mut number = start_number;
        let mut previous_end: Option<u64> = None;
        for entry in &template.timeline {
            let mut time = entry.t.unwrap_or(previous_end.unwrap_or(0));
            for _ in 0..=entry.r {
                if segments.len() as u64 >= MAX_SEGMENTS {
                    return Err(Error::Backend(
                        "DASH SegmentTimeline contains too many segments".to_string(),
                    ));
                }
                let expanded = expand_template(
                    media,
                    &representation.id,
                    representation.bandwidth,
                    number,
                    time,
                )?;
                segments.push(SegmentRef {
                    url: join_url(base, &expanded)?,
                    range: None,
                    timing: Some(SegmentTiming {
                        start: time,
                        duration: entry.d,
                        timescale,
                    }),
                });
                number = number.checked_add(1).ok_or_else(|| {
                    Error::Backend("DASH segment number overflows u64".to_string())
                })?;
                time = time
                    .checked_add(entry.d)
                    .ok_or_else(|| Error::Backend("DASH segment time overflows u64".to_string()))?;
            }
            previous_end = Some(time);
        }
    } else {
        let end_number = template
            .attributes
            .get("endNumber")
            .map(|value| parse_u64(value, "endNumber"))
            .transpose()?;
        let count = if let Some(end) = end_number {
            end.checked_sub(start_number)
                .and_then(|count| count.checked_add(1))
                .ok_or_else(|| {
                    Error::Backend("SegmentTemplate endNumber is before startNumber".to_string())
                })?
        } else {
            let duration = duration.ok_or_else(|| {
                Error::Backend(
                    "SegmentTemplate needs SegmentTimeline, endNumber or a constant duration"
                        .to_string(),
                )
            })?;
            if duration == 0 {
                return Err(Error::Backend(
                    "SegmentTemplate duration must be greater than zero".to_string(),
                ));
            }
            let total = total_duration.ok_or_else(|| {
                Error::Backend(
                    "SegmentTemplate with a constant duration needs the period or manifest duration"
                        .to_string(),
                )
            })?;
            let count = (total * timescale as f64 / duration as f64).ceil();
            if !count.is_finite() || count < 1.0 || count > MAX_SEGMENTS as f64 {
                return Err(Error::Backend(
                    "SegmentTemplate produces an invalid segment count".to_string(),
                ));
            }
            count as u64
        };
        if count == 0 || count > MAX_SEGMENTS {
            return Err(Error::Backend(
                "SegmentTemplate produces an invalid segment count".to_string(),
            ));
        }
        if duration.is_none() && media.contains("$Time") {
            return Err(Error::Backend(
                "SegmentTemplate $Time$ requires a constant duration".to_string(),
            ));
        }
        for index in 0..count {
            let number = start_number
                .checked_add(index)
                .ok_or_else(|| Error::Backend("DASH segment number overflows u64".to_string()))?;
            let time = match duration {
                Some(duration) => time_offset
                    .checked_add(index.checked_mul(duration).ok_or_else(|| {
                        Error::Backend("DASH segment time overflows u64".to_string())
                    })?)
                    .ok_or_else(|| Error::Backend("DASH segment time overflows u64".to_string()))?,
                None => time_offset,
            };
            let expanded = expand_template(
                media,
                &representation.id,
                representation.bandwidth,
                number,
                time,
            )?;
            segments.push(SegmentRef {
                url: join_url(base, &expanded)?,
                range: None,
                timing: duration.map(|duration| SegmentTiming {
                    start: time,
                    duration,
                    timescale,
                }),
            });
        }
    }
    Ok((initialization, segments))
}

fn resolve_segment_list(
    base: &Url,
    list: &SegmentList,
) -> Result<(Option<SegmentRef>, Vec<SegmentRef>)> {
    let initialization = list
        .initialization
        .as_ref()
        .map(|(source, range)| {
            let url = match source {
                Some(source) => join_url(base, source)?,
                None => base.clone(),
            };
            Ok::<SegmentRef, Error>(SegmentRef {
                url,
                range: range.as_deref().map(parse_byte_range).transpose()?,
                timing: None,
            })
        })
        .transpose()?;
    let mut segments = Vec::new();
    for (media, range) in &list.urls {
        let url = match media {
            Some(media) => join_url(base, media)?,
            None => base.clone(),
        };
        segments.push(SegmentRef {
            url,
            range: range.as_deref().map(parse_byte_range).transpose()?,
            timing: None,
        });
    }
    Ok((initialization, segments))
}

fn resolve_base_object(
    representation: &Representation,
) -> Result<(Option<SegmentRef>, Vec<SegmentRef>)> {
    if !representation.has_base {
        return Err(Error::Backend(format!(
            "DASH representation {} has no supported segment addressing",
            representation.id
        )));
    }
    Ok((
        None,
        vec![SegmentRef {
            url: representation.base_url.clone(),
            range: None,
            timing: None,
        }],
    ))
}

fn merge_template(parent: Option<&Template>, child: Option<&Template>) -> Option<Template> {
    match (parent, child) {
        (None, None) => None,
        (Some(template), None) | (None, Some(template)) => Some(template.clone()),
        (Some(parent), Some(child)) => {
            let mut attributes = parent.attributes.clone();
            attributes.extend(child.attributes.clone());
            let (timeline, has_timeline) = if child.has_timeline {
                (child.timeline.clone(), true)
            } else if parent.has_timeline {
                (parent.timeline.clone(), true)
            } else {
                (Vec::new(), false)
            };
            Some(Template {
                attributes,
                has_timeline,
                timeline,
            })
        }
    }
}

fn merge_segment_list(
    parent: Option<&SegmentList>,
    child: Option<&SegmentList>,
) -> Option<SegmentList> {
    match (parent, child) {
        (None, None) => None,
        (Some(list), None) | (None, Some(list)) => Some(list.clone()),
        (Some(parent), Some(child)) => Some(SegmentList {
            initialization: child
                .initialization
                .clone()
                .or_else(|| parent.initialization.clone()),
            urls: if child.urls.is_empty() {
                parent.urls.clone()
            } else {
                child.urls.clone()
            },
        }),
    }
}

fn resolve_base(parent: &Url, node: &Node) -> Result<Url> {
    let Some(base) = node.child("BaseURL") else {
        return Ok(parent.clone());
    };
    let value = base.text.trim();
    if value.is_empty() {
        return Ok(parent.clone());
    }
    parent
        .join(value)
        .map_err(|error| Error::Backend(format!("invalid DASH BaseURL {value}: {error}")))
}

fn join_url(base: &Url, value: &str) -> Result<Url> {
    base.join(value)
        .map_err(|error| Error::Backend(format!("invalid DASH segment URL {value}: {error}")))
}

fn parse_byte_range(value: &str) -> Result<ByteRange> {
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| Error::Backend(format!("invalid DASH byte range: {value}")))?;
    let start = parse_u64(start, "byte range start")?;
    let end = parse_u64(end, "byte range end")?
        .checked_add(1)
        .ok_or_else(|| Error::Backend(format!("DASH byte range overflows u64: {value}")))?;
    ByteRange::new(start, end)
        .map_err(|error| Error::Backend(format!("invalid DASH byte range {value}: {error}")))
}

fn parse_u64(value: &str, name: &str) -> Result<u64> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| Error::Backend(format!("invalid DASH {name}: {value}")))
}

fn parse_u64_attr(node: &Node, name: &str) -> Result<Option<u64>> {
    node.attr(name)
        .map(|value| parse_u64(value, name))
        .transpose()
}

fn media_category(set: &AdaptationSet, representation: &Representation) -> Option<u8> {
    let mime = representation
        .mime_type
        .as_deref()
        .or(set.mime_type.as_deref())
        .unwrap_or("");
    let content = set.content_type.as_deref().unwrap_or("");
    if mime.starts_with("video/") || content == "video" {
        return Some(0);
    }
    if mime.starts_with("audio/") || content == "audio" {
        return Some(1);
    }
    if mime.starts_with("text/")
        || mime.starts_with("image/")
        || mime.starts_with("application/")
        || matches!(content, "text" | "image")
    {
        return None;
    }
    Some(2)
}

fn glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern)
            .map_err(|error| Error::Config(format!("invalid glob {pattern}: {error}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| Error::Config(format!("invalid glob set: {error}")))
}

fn matches_any(set: &GlobSet, adaptation: &AdaptationSet, representation: &Representation) -> bool {
    if set.is_empty() {
        return false;
    }
    let mut candidates = vec![representation.id.clone()];
    if let Some(id) = &adaptation.id {
        candidates.push(id.clone());
    }
    for mime in [&representation.mime_type, &adaptation.mime_type]
        .into_iter()
        .flatten()
    {
        candidates.push(mime.clone());
    }
    for codecs in [&representation.codecs, &adaptation.codecs]
        .into_iter()
        .flatten()
    {
        candidates.push(codecs.clone());
    }
    for lang in [&representation.lang, &adaptation.lang]
        .into_iter()
        .flatten()
    {
        candidates.push(lang.clone());
    }
    let width = representation.width.or(adaptation.width);
    let height = representation.height.or(adaptation.height);
    if let (Some(width), Some(height)) = (width, height) {
        candidates.push(format!("{width}x{height}"));
    }
    if let Some(bandwidth) = representation.bandwidth {
        candidates.push(bandwidth.to_string());
    }
    if let Some(content) = &adaptation.content_type {
        candidates.push(content.clone());
    }
    candidates.iter().any(|candidate| set.is_match(candidate))
}

fn expand_template(
    template: &str,
    representation_id: &str,
    bandwidth: Option<u64>,
    number: u64,
    time: u64,
) -> Result<String> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(after) = after.strip_prefix('$') {
            output.push('$');
            rest = after;
            continue;
        }
        let Some(end) = after.find('$') else {
            return Err(Error::Backend(format!(
                "unterminated DASH template placeholder: {template}"
            )));
        };
        output.push_str(&format_placeholder(
            &after[..end],
            representation_id,
            bandwidth,
            number,
            time,
        )?);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn format_placeholder(
    token: &str,
    representation_id: &str,
    bandwidth: Option<u64>,
    number: u64,
    time: u64,
) -> Result<String> {
    let (name, format) = match token.split_once('%') {
        Some((name, format)) => (name, Some(format)),
        None => (token, None),
    };
    match name {
        "RepresentationID" => {
            if format.is_some() {
                return Err(Error::Backend(
                    "$RepresentationID$ does not support a format specifier".to_string(),
                ));
            }
            Ok(representation_id.to_string())
        }
        "Bandwidth" => {
            if format.is_some() {
                return Err(Error::Backend(
                    "$Bandwidth$ does not support a format specifier".to_string(),
                ));
            }
            bandwidth
                .map(|value| value.to_string())
                .ok_or_else(|| Error::Backend("$Bandwidth$ used without bandwidth".to_string()))
        }
        "Number" => format_number(number, format),
        "Time" => format_number(time, format),
        other => Err(Error::Backend(format!(
            "unsupported DASH template placeholder: ${other}$"
        ))),
    }
}

fn format_number(value: u64, format: Option<&str>) -> Result<String> {
    let Some(format) = format else {
        return Ok(value.to_string());
    };
    let Some(width) = format.strip_suffix('d') else {
        return Err(Error::Backend(format!(
            "unsupported DASH number format: %{format}"
        )));
    };
    if width.is_empty() {
        return Ok(value.to_string());
    }
    let Some(width) = width.strip_prefix('0') else {
        return Err(Error::Backend(format!(
            "unsupported DASH number format: %{format}"
        )));
    };
    let width = width
        .parse::<usize>()
        .map_err(|_| Error::Backend(format!("unsupported DASH number format: %{format}")))?;
    if width > 20 {
        return Err(Error::Backend(format!(
            "DASH number format width is too large: %{format}"
        )));
    }
    Ok(format!("{value:0width$}"))
}

fn parse_iso_duration(value: &str) -> Result<f64> {
    let value = value.trim();
    let Some(rest) = value.strip_prefix('P') else {
        return Err(Error::Backend(format!(
            "invalid ISO 8601 duration: {value}"
        )));
    };
    let mut seconds = 0.0;
    let mut number = String::new();
    let mut in_time = false;
    for character in rest.chars() {
        match character {
            'T' => {
                if !number.is_empty() {
                    return Err(Error::Backend(format!(
                        "invalid ISO 8601 duration: {value}"
                    )));
                }
                in_time = true;
            }
            '0'..='9' | '.' => number.push(character),
            'D' if !in_time => {
                seconds += parse_duration_number(&number, value)? * 86_400.0;
                number.clear();
            }
            'H' if in_time => {
                seconds += parse_duration_number(&number, value)? * 3_600.0;
                number.clear();
            }
            'M' if in_time => {
                seconds += parse_duration_number(&number, value)? * 60.0;
                number.clear();
            }
            'S' if in_time => {
                seconds += parse_duration_number(&number, value)?;
                number.clear();
            }
            _ => {
                return Err(Error::Backend(format!(
                    "unsupported ISO 8601 duration: {value}"
                )));
            }
        }
    }
    if !number.is_empty() || !seconds.is_finite() || seconds < 0.0 {
        return Err(Error::Backend(format!(
            "invalid ISO 8601 duration: {value}"
        )));
    }
    Ok(seconds)
}

fn parse_duration_number(number: &str, original: &str) -> Result<f64> {
    if number.is_empty() {
        return Err(Error::Backend(format!(
            "invalid ISO 8601 duration: {original}"
        )));
    }
    number
        .parse::<f64>()
        .map_err(|_| Error::Backend(format!("invalid ISO 8601 duration: {original}")))
}

fn parse_template(node: &Node) -> Result<Option<Template>> {
    let Some(template) = node.child("SegmentTemplate") else {
        return Ok(None);
    };
    if template.attributes.contains_key("href") {
        return Err(Error::Backend(
            "DASH xlink:href is not supported".to_string(),
        ));
    }
    let attributes = template.attributes.clone();
    let has_timeline = template.child("SegmentTimeline").is_some();
    let timeline = match template.child("SegmentTimeline") {
        Some(timeline) => timeline
            .children("S")
            .map(|entry| {
                let d = entry
                    .attr("d")
                    .ok_or_else(|| Error::Backend("SegmentTimeline S is missing d".to_string()))
                    .and_then(|value| parse_u64(value, "SegmentTimeline d"))?;
                let t = entry
                    .attr("t")
                    .map(|value| parse_u64(value, "SegmentTimeline t"))
                    .transpose()?;
                let r = entry
                    .attr("r")
                    .map(|value| value.trim().parse::<i64>())
                    .transpose()
                    .map_err(|_| Error::Backend("invalid SegmentTimeline r".to_string()))?
                    .unwrap_or(0);
                if r < 0 {
                    return Err(Error::Backend(
                        "negative SegmentTimeline repeat counts are not supported".to_string(),
                    ));
                }
                Ok(TimelineEntry { t, d, r: r as u64 })
            })
            .collect::<Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    Ok(Some(Template {
        attributes,
        has_timeline,
        timeline,
    }))
}

fn parse_segment_list(node: &Node) -> Result<Option<SegmentList>> {
    let Some(list) = node.child("SegmentList") else {
        return Ok(None);
    };
    if list.attributes.contains_key("href") {
        return Err(Error::Backend(
            "DASH xlink:href is not supported".to_string(),
        ));
    }
    let initialization = list.child("Initialization").map(|init| {
        if init.attributes.contains_key("href") {
            return Err(Error::Backend(
                "DASH xlink:href is not supported".to_string(),
            ));
        }
        Ok((
            init.attr("sourceURL").map(str::to_string),
            init.attr("range").map(str::to_string),
        ))
    });
    let initialization = initialization.transpose()?;
    let urls = list
        .children("SegmentURL")
        .map(|url| {
            if url.attributes.contains_key("href") {
                return Err(Error::Backend(
                    "DASH xlink:href is not supported".to_string(),
                ));
            }
            Ok((
                url.attr("media").map(str::to_string),
                url.attr("mediaRange").map(str::to_string),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(SegmentList {
        initialization,
        urls,
    }))
}

fn parse_xml(text: &str) -> Result<Node> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => stack.push(node_from_start(&start)?),
            Ok(Event::Empty(start)) => {
                let node = node_from_start(&start)?;
                attach_node(&mut stack, &mut root, node)?;
            }
            Ok(Event::End(_)) => {
                let Some(node) = stack.pop() else {
                    return Err(Error::Backend(
                        "invalid DASH manifest: unmatched closing tag".to_string(),
                    ));
                };
                attach_node(&mut stack, &mut root, node)?;
            }
            Ok(Event::Text(text)) => {
                if let Some(parent) = stack.last_mut() {
                    let decoded = text.xml_content(XmlVersion::Implicit1_0).map_err(|error| {
                        Error::Backend(format!("invalid DASH manifest text: {error}"))
                    })?;
                    parent.text.push_str(&decoded);
                }
            }
            Ok(Event::CData(data)) => {
                if let Some(parent) = stack.last_mut() {
                    let decoded = data.decode().map_err(|error| {
                        Error::Backend(format!("invalid DASH manifest CDATA: {error}"))
                    })?;
                    parent.text.push_str(&decoded);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(parent) = stack.last_mut() {
                    parent.text.push_str(&resolve_reference(&reference)?);
                }
            }
            Ok(Event::DocType(_)) => {
                return Err(Error::Backend(
                    "DASH manifests with a DOCTYPE are not supported".to_string(),
                ));
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(Error::Backend(format!(
                    "invalid DASH manifest XML: {error}"
                )));
            }
        }
    }
    if !stack.is_empty() {
        return Err(Error::Backend(
            "invalid DASH manifest: unclosed element".to_string(),
        ));
    }
    root.ok_or_else(|| Error::Backend("DASH manifest is empty".to_string()))
}

fn node_from_start(start: &quick_xml::events::BytesStart<'_>) -> Result<Node> {
    let name = local_name(start.name().as_ref());
    let mut attributes = BTreeMap::new();
    for attribute in start.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Backend(format!("invalid DASH manifest attribute: {error}")))?;
        let key = local_name(attribute.key.as_ref());
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|error| {
                Error::Backend(format!("invalid DASH manifest attribute value: {error}"))
            })?
            .into_owned();
        attributes.insert(key, value);
    }
    Ok(Node {
        name,
        attributes,
        children: Vec::new(),
        text: String::new(),
    })
}

fn local_name(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    let local = name.rsplit(':').next().unwrap_or(name.as_ref());
    local.to_string()
}

fn attach_node(stack: &mut [Node], root: &mut Option<Node>, node: Node) -> Result<()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.is_none() {
        *root = Some(node);
    } else {
        return Err(Error::Backend(
            "DASH manifest contains multiple root elements".to_string(),
        ));
    }
    Ok(())
}

fn resolve_reference(reference: &[u8]) -> Result<String> {
    let reference = std::str::from_utf8(reference).map_err(|_| {
        Error::Backend("DASH manifest contains a non-UTF-8 entity reference".to_string())
    })?;
    match reference {
        "amp" => Ok("&".to_string()),
        "lt" => Ok("<".to_string()),
        "gt" => Ok(">".to_string()),
        "quot" => Ok("\"".to_string()),
        "apos" => Ok("'".to_string()),
        _ => {
            let code = reference
                .strip_prefix("#x")
                .or_else(|| reference.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    reference
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse::<u32>().ok())
                })
                .and_then(char::from_u32)
                .ok_or_else(|| {
                    Error::Backend(format!("unsupported DASH manifest entity: &{reference};"))
                })?;
            Ok(code.to_string())
        }
    }
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/media/manifest.mpd").unwrap()
    }

    fn template_manifest() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static"
     mediaPresentationDuration="PT20S">
  <Period>
    <AdaptationSet id="video" mimeType="video/mp4" contentType="video">
      <SegmentTemplate timescale="1000" duration="4000" startNumber="1"
          initialization="init-$RepresentationID$.mp4"
          media="seg-$Number%05d$.m4s"/>
      <Representation id="720p" bandwidth="800000" width="1280" height="720" codecs="avc1.4d401f"/>
      <Representation id="1080p" bandwidth="2400000" width="1920" height="1080" codecs="avc1.640028"/>
    </AdaptationSet>
    <AdaptationSet id="audio" mimeType="audio/mp4" contentType="audio" lang="en">
      <SegmentTemplate timescale="1000" duration="4000"
          initialization="audio/init.mp4" media="audio/seg-$Number$.m4s"/>
      <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>
"#
    }

    #[test]
    fn selects_the_highest_bandwidth_video_and_expands_templates() {
        let manifest = DashManifest::parse(&base(), template_manifest()).unwrap();
        let selected = manifest.select(&[], &[]).unwrap();
        assert_eq!(selected.id, "1080p");
        assert_eq!(selected.extension, "mp4");
        assert_eq!(
            selected.initialization.unwrap().url.as_str(),
            "https://example.com/media/init-1080p.mp4"
        );
        assert_eq!(selected.segments.len(), 5);
        assert_eq!(
            selected.segments[0].url.as_str(),
            "https://example.com/media/seg-00001.m4s"
        );
        assert_eq!(
            selected.segments[4].url.as_str(),
            "https://example.com/media/seg-00005.m4s"
        );
    }

    #[test]
    fn selects_a_matching_representation() {
        let manifest = DashManifest::parse(&base(), template_manifest()).unwrap();
        let selected = manifest.select(&["*720*".to_string()], &[]).unwrap();
        assert_eq!(selected.id, "720p");
        let selected = manifest
            .select(&[], &["*avc1.640028*".to_string()])
            .unwrap();
        assert_eq!(selected.id, "720p");
    }

    #[test]
    fn parses_a_segment_timeline_with_time_templates() {
        let text = r#"<MPD type="static"><Period>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v" bandwidth="1">
              <SegmentTemplate media="t$Time$.m4s" initialization="init.mp4" timescale="1000">
                <SegmentTimeline>
                  <S t="0" d="4000" r="1"/>
                  <S d="2000"/>
                </SegmentTimeline>
              </SegmentTemplate>
            </Representation>
          </AdaptationSet>
        </Period></MPD>"#;
        let manifest = DashManifest::parse(&base(), text).unwrap();
        let selected = manifest.select(&[], &[]).unwrap();
        let urls: Vec<&str> = selected
            .segments
            .iter()
            .map(|segment| segment.url.as_str())
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://example.com/media/t0.m4s",
                "https://example.com/media/t4000.m4s",
                "https://example.com/media/t8000.m4s"
            ]
        );
    }

    #[test]
    fn uses_end_number_without_a_total_duration() {
        let text = r#"<MPD type="static"><Period><AdaptationSet mimeType="video/mp4">
          <Representation id="v" bandwidth="1">
            <SegmentTemplate media="s-$Number$.m4s" initialization="init.mp4"
                timescale="1000" duration="4000" startNumber="3" endNumber="5"/>
          </Representation>
        </AdaptationSet></Period></MPD>"#;
        let manifest = DashManifest::parse(&base(), text).unwrap();
        let selected = manifest.select(&[], &[]).unwrap();
        assert_eq!(selected.segments.len(), 3);
        assert_eq!(
            selected.segments[0].url.as_str(),
            "https://example.com/media/s-3.m4s"
        );
        assert_eq!(
            selected.segments[2].url.as_str(),
            "https://example.com/media/s-5.m4s"
        );
    }

    #[test]
    fn accepts_dynamic_manifests_with_a_segment_timeline() {
        let text = r#"<MPD type="dynamic" minimumUpdatePeriod="PT2S"><Period>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v" bandwidth="1">
              <SegmentTemplate media="s$Time$.m4s" initialization="init.mp4" timescale="1000">
                <SegmentTimeline>
                  <S t="0" d="2000" r="1"/>
                </SegmentTimeline>
              </SegmentTemplate>
            </Representation>
          </AdaptationSet>
        </Period></MPD>"#;
        let manifest = DashManifest::parse(&base(), text).unwrap();
        assert!(manifest.is_dynamic());
        let selected = manifest.select(&[], &[]).unwrap();
        assert!(selected.live);
        assert_eq!(selected.minimum_update_period_secs, Some(2.0));
        assert_eq!(selected.segments.len(), 2);
        assert_eq!(
            selected.segments[0].url.as_str(),
            "https://example.com/media/s0.m4s"
        );
        let timing = selected.segments[1].timing.unwrap();
        assert_eq!(timing.start, 2000);
        assert_eq!(timing.seconds(), 2.0);
        let again = manifest.select_representation("v").unwrap();
        assert_eq!(again.segments[1].url, selected.segments[1].url);
        assert!(manifest.select_representation("missing").is_err());
    }

    #[test]
    fn rejects_dynamic_manifests_without_a_timeline() {
        let text = r#"<MPD type="dynamic"><Period>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v" bandwidth="1">
              <SegmentTemplate media="s-$Number$.m4s" duration="2000" timescale="1000"/>
            </Representation>
          </AdaptationSet>
        </Period></MPD>"#;
        let manifest = DashManifest::parse(&base(), text).unwrap();
        assert!(manifest.select(&[], &[]).is_err());
    }

    #[test]
    fn parses_segment_list_ranges_and_base_url_inheritance() {
        let text = r#"<MPD type="static"><Period>
          <BaseURL>video/</BaseURL>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v" bandwidth="1">
              <BaseURL>1080/</BaseURL>
              <SegmentList>
                <Initialization sourceURL="init.mp4" range="0-99"/>
                <SegmentURL media="seg-1.m4s" mediaRange="100-199"/>
                <SegmentURL media="https://cdn.example.com/seg-2.m4s"/>
              </SegmentList>
            </Representation>
          </AdaptationSet>
        </Period></MPD>"#;
        let manifest = DashManifest::parse(&base(), text).unwrap();
        let selected = manifest.select(&[], &[]).unwrap();
        let init = selected.initialization.unwrap();
        assert_eq!(
            init.url.as_str(),
            "https://example.com/media/video/1080/init.mp4"
        );
        assert_eq!(init.range.unwrap(), ByteRange::new(0, 100).unwrap());
        assert_eq!(
            selected.segments[0].url.as_str(),
            "https://example.com/media/video/1080/seg-1.m4s"
        );
        assert_eq!(
            selected.segments[0].range.unwrap(),
            ByteRange::new(100, 200).unwrap()
        );
        assert_eq!(
            selected.segments[1].url.as_str(),
            "https://cdn.example.com/seg-2.m4s"
        );
    }

    #[test]
    fn treats_segment_base_as_a_single_base_url_object() {
        let text = r#"<MPD type="static"><Period>
          <AdaptationSet mimeType="video/mp4">
            <Representation id="v" bandwidth="1">
              <BaseURL>movie.mp4</BaseURL>
              <SegmentBase indexRange="0-999"/>
            </Representation>
          </AdaptationSet>
        </Period></MPD>"#;
        let manifest = DashManifest::parse(&base(), text).unwrap();
        let selected = manifest.select(&[], &[]).unwrap();
        assert!(selected.initialization.is_none());
        assert_eq!(selected.segments.len(), 1);
        assert_eq!(
            selected.segments[0].url.as_str(),
            "https://example.com/media/movie.mp4"
        );
    }

    #[test]
    fn rejects_unsupported_manifests() {
        let dynamic = r#"<MPD type="dynamic"><Period/></MPD>"#;
        assert!(DashManifest::parse(&base(), dynamic).is_err());

        let multi = r#"<MPD type="static"><Period/><Period/></MPD>"#;
        assert!(DashManifest::parse(&base(), multi).is_err());

        let drm = r#"<MPD type="static"><Period><AdaptationSet>
          <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011"/>
          <Representation id="v"/>
        </AdaptationSet></Period></MPD>"#;
        assert!(DashManifest::parse(&base(), drm).is_err());

        let xlink = r#"<MPD type="static"><Period><AdaptationSet href="remote.xml">
        </AdaptationSet></Period></MPD>"#;
        assert!(DashManifest::parse(&base(), xlink).is_err());
    }

    #[test]
    fn parses_iso_durations() {
        assert_eq!(parse_iso_duration("PT20S").unwrap(), 20.0);
        assert_eq!(parse_iso_duration("PT1H2M3.5S").unwrap(), 3723.5);
        assert_eq!(parse_iso_duration("P1DT1S").unwrap(), 86_401.0);
        assert!(parse_iso_duration("20").is_err());
        assert!(parse_iso_duration("P1Y").is_err());
    }

    #[test]
    fn expands_templates_and_formats_numbers() {
        assert_eq!(
            expand_template(
                "$RepresentationID$/seg-$Number%05d$-$Time$-$Bandwidth$.m4s",
                "v",
                Some(42),
                7,
                9000
            )
            .unwrap(),
            "v/seg-00007-9000-42.m4s"
        );
        assert_eq!(expand_template("a$$b", "v", None, 1, 0).unwrap(), "a$b");
        assert!(expand_template("$Unknown$", "v", None, 1, 0).is_err());
    }

    #[test]
    fn rejects_dtds_and_unknown_entities() {
        let dtd = r#"<!DOCTYPE MPD [<!ENTITY x "y">]><MPD type="static"><Period/></MPD>"#;
        assert!(DashManifest::parse(&base(), dtd).is_err());
        let entity = r#"<MPD type="static"><Period><BaseURL>&bogus;</BaseURL></Period></MPD>"#;
        assert!(DashManifest::parse(&base(), entity).is_err());
    }
}
