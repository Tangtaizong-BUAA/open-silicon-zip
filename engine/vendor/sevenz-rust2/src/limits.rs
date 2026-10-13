//! Bounds for APIs that collect untrusted decoded bytes in memory. Streaming
//! extraction intentionally has no per-file 256 MiB cutoff.
use std::io::Read;
use crate::Error;

pub(crate) const MAX_HEADER_BYTES: usize = 64 << 20;
pub(crate) const MAX_BUFFERED_BYTES: u64 = 256 << 20;
pub(crate) const MAX_ENTRIES: usize = 100_000;
pub(crate) const MAX_NAME_BYTES: usize = 16 << 20;

pub(crate) fn resource_limit(what: &str) -> Error {
    Error::other(format!("7z resource budget exceeded: {what}"))
}

pub(crate) fn checked_size(size: u64) -> Result<usize, Error> {
    usize::try_from(size).map_err(|_| resource_limit("size exceeds address space"))
}

pub(crate) fn check_name(name: &str) -> Result<(), Error> {
    if name.len() > 4096 || name.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".").count() > 64 {
        return Err(resource_limit("path exceeds 4096 bytes or 64 components"));
    }
    Ok(())
}

pub(crate) struct BufferedBudget { remaining: u64 }

impl BufferedBudget {
    pub(crate) fn new(remaining: u64) -> Self { Self { remaining } }

    pub(crate) fn charge(&mut self, size: u64) -> Result<(), Error> {
        self.remaining = self.remaining.checked_sub(size).ok_or_else(|| resource_limit("cumulative decoded bytes"))?;
        Ok(())
    }

    pub(crate) fn read(&mut self, reader: &mut dyn Read, size: u64) -> Result<Vec<u8>, Error> {
        self.charge(size)?;
        let capacity = checked_size(size)?;
        let mut data = Vec::new();
        let mut block = [0u8; 64 << 10];
        loop {
            let n = reader.read(&mut block)?;
            if n == 0 { break; }
            if n > capacity.saturating_sub(data.len()) { return Err(resource_limit("decoded data exceeds declared size")); }
            // Avoid Vec's geometric growth allocating twice the configured cap.
            if data.len() + n > data.capacity() {
                let target = data.capacity().max(block.len()).saturating_mul(2).min(capacity);
                data.try_reserve_exact(target - data.len()).map_err(|_| resource_limit("buffer allocation"))?;
            }
            data.extend_from_slice(&block[..n]);
        }
        if data.len() != capacity { return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()); }
        Ok(data)
    }
}
