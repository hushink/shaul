use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A byte range with an exclusive end. Used for HTTP segments and resume
/// manifests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    /// First byte included in the range.
    pub start: u64,
    /// Exclusive.
    pub end: u64,
}

impl ByteRange {
    /// Creates a half-open range, rejecting `start >= end`.
    pub fn new(start: u64, end: u64) -> Result<Self> {
        if start >= end {
            return Err(Error::Other(format!("invalid byte range {start}..{end}")));
        }
        Ok(Self { start, end })
    }

    /// Number of bytes covered by the range.
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    /// Returns `true` when the range covers no bytes.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Returns `true` if `other` lies entirely inside this range.
    pub fn contains(&self, other: &ByteRange) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

/// Sort and coalesce overlapping or adjacent ranges.
pub fn merge_ranges(mut ranges: Vec<ByteRange>) -> Vec<ByteRange> {
    ranges.retain(|range| !range.is_empty());
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => {
                if range.end > last.end {
                    last.end = range.end;
                }
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Complement of `completed` inside `0..total`.
pub fn missing_ranges(total: u64, completed: &[ByteRange]) -> Vec<ByteRange> {
    let completed = merge_ranges(completed.to_vec());
    let mut missing = Vec::new();
    let mut cursor = 0u64;
    for range in completed {
        if range.start > cursor
            && let Ok(gap) = ByteRange::new(cursor, range.start.min(total))
        {
            missing.push(gap);
        }
        cursor = cursor.max(range.end);
        if cursor >= total {
            break;
        }
    }
    if cursor < total
        && let Ok(gap) = ByteRange::new(cursor, total)
    {
        missing.push(gap);
    }
    missing
}

/// Total number of bytes covered, counting overlapping ranges once.
pub fn completed_bytes(completed: &[ByteRange]) -> u64 {
    merge_ranges(completed.to_vec())
        .iter()
        .map(ByteRange::len)
        .sum()
}

/// Drop or clamp ranges that extend past the known file length.
pub fn clamp_to_len(ranges: &[ByteRange], len: u64) -> Vec<ByteRange> {
    ranges
        .iter()
        .filter_map(|range| {
            if range.start >= len {
                None
            } else {
                ByteRange::new(range.start, range.end.min(len)).ok()
            }
        })
        .collect()
}

/// Split a range into chunks of at most `chunk_size` bytes.
pub fn split_range(range: ByteRange, chunk_size: u64) -> Vec<ByteRange> {
    let chunk_size = chunk_size.max(1);
    let mut chunks = Vec::new();
    let mut start = range.start;
    while start < range.end {
        let end = (start + chunk_size).min(range.end);
        if let Ok(chunk) = ByteRange::new(start, end) {
            chunks.push(chunk);
        }
        start = end;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_adjacent_and_overlapping() {
        let merged = merge_ranges(vec![
            ByteRange::new(0, 10).unwrap(),
            ByteRange::new(10, 20).unwrap(),
            ByteRange::new(15, 25).unwrap(),
            ByteRange::new(40, 50).unwrap(),
        ]);
        assert_eq!(
            merged,
            vec![
                ByteRange::new(0, 25).unwrap(),
                ByteRange::new(40, 50).unwrap()
            ]
        );
    }

    #[test]
    fn computes_missing_ranges() {
        let missing = missing_ranges(
            100,
            &[
                ByteRange::new(10, 20).unwrap(),
                ByteRange::new(50, 60).unwrap(),
            ],
        );
        assert_eq!(
            missing,
            vec![
                ByteRange::new(0, 10).unwrap(),
                ByteRange::new(20, 50).unwrap(),
                ByteRange::new(60, 100).unwrap()
            ]
        );
    }

    #[test]
    fn splits_ranges() {
        let chunks = split_range(ByteRange::new(0, 10).unwrap(), 4);
        assert_eq!(
            chunks,
            vec![
                ByteRange::new(0, 4).unwrap(),
                ByteRange::new(4, 8).unwrap(),
                ByteRange::new(8, 10).unwrap()
            ]
        );
    }
}
