//! Operational limits for untrusted archive metadata. These are collection and
//! decoded-stream limits, independent of a codec's dictionary/workspace budget.
use crate::error::{Error, ErrorKind, Result};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ListingLimits {
    pub entries: usize,
    pub names: u64,
    pub metadata: u64,
    pub json: u64,
    pub path_bytes: usize,
    pub path_depth: usize,
    pub tar_decoded: u64,
    pub tar_metadata: u64,
    pub elapsed: Duration,
}

impl Default for ListingLimits {
    fn default() -> Self {
        Self { entries: 100_000, names: 16 << 20, metadata: 128 << 20,
            json: 64 << 20, path_bytes: 4096, path_depth: 64,
            tar_decoded: 16 << 30, tar_metadata: 64 << 10,
            elapsed: Duration::from_secs(60) }
    }
}

pub(crate) fn exceeded(what: &str) -> Error {
    Error::new(ErrorKind::ResourceLimit, format!("archive listing budget exceeded: {what}"))
}

impl ListingLimits {
    pub fn check_path(&self, name: &str) -> Result<()> {
        if name.len() > self.path_bytes { return Err(exceeded("path exceeds 4096 UTF-8 bytes")); }
        if name.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".").count() > self.path_depth {
            return Err(exceeded("path exceeds 64 components"));
        }
        Ok(())
    }
}

pub(crate) struct ListingBudget {
    pub limits: ListingLimits,
    entries: usize,
    names: u64,
    metadata: u64,
    json: u64,
}

impl ListingBudget {
    pub fn new(limits: ListingLimits) -> Self {
        Self { limits, entries: 0, names: 0, metadata: 0, json: 0 }
    }

    /// Charge before copying a record. The conservative estimate includes the
    /// parser's copies, decoded UTF-8, list models, and serialization overhead.
    pub fn reserve(&mut self, name_bytes: usize) -> Result<()> {
        if self.entries >= self.limits.entries { return Err(exceeded("entry count")); }
        let names = self.names.saturating_add(name_bytes as u64);
        let metadata = self.metadata.saturating_add(1024 + (name_bytes as u64).saturating_mul(16));
        if names > self.limits.names { return Err(exceeded("aggregate name/link bytes")); }
        if metadata > self.limits.metadata { return Err(exceeded("parsed metadata memory")); }
        self.entries += 1;
        self.names = names;
        self.metadata = metadata;
        Ok(())
    }

    /// Every list line has fewer than 512 fixed bytes. A JSON string needs at
    /// most six output bytes per UTF-8 byte, including control-character escapes.
    pub fn reserve_json(&mut self, name: &str) -> Result<()> {
        self.limits.check_path(name)?;
        self.json = self.json.saturating_add(512 + (name.len() as u64).saturating_mul(6));
        if self.json > self.limits.json { return Err(exceeded("emitted JSON bytes")); }
        Ok(())
    }
}
