//! Read-cache strategies for buffered file objects.
//!
//! Mirrors fsspec's `caching.py` — a file holds a [`Cache`] that sits
//! between the user's `read()` calls and the backend's range-fetch
//! primitive.  Each strategy trades off memory, latency, and I/O
//! volume differently.
//!
//! # Supported strategies
//!
//! | Name          | Struct              | Behaviour |
//! |---------------|---------------------|-----------|
//! | `"readahead"` | [`ReadAheadCache`]  | Single-block lookahead; great for sequential reads. |
//! | `"block"`     | [`BlockCache`]      | Fixed-size blocks with LRU eviction. |
//! | `"all"`       | [`AllBytesCache`]   | Fetch entire file on first access. |
//! | `"none"`      | [`NoCache`]         | Pass-through, no caching at all. |

use crate::error::FsResult;

// ============================================================================
// Fetcher type
// ============================================================================

/// A function that retrieves the byte range `[start, end)` from the backend.
pub type Fetcher = Box<dyn FnMut(u64, u64) -> FsResult<Vec<u8>> + Send>;

// ============================================================================
// CacheType enum
// ============================================================================

/// Identifies a caching strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheType {
    None,
    ReadAhead,
    Block,
    All,
}

impl CacheType {
    /// Parse from a string (case-insensitive).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "readahead" | "read_ahead" => Some(Self::ReadAhead),
            "block" | "blockcache" => Some(Self::Block),
            "all" | "bytes" => Some(Self::All),
            _ => None,
        }
    }
}

// ============================================================================
// Cache trait
// ============================================================================

/// A read cache that wraps a `Fetcher`.
pub trait Cache: Send {
    /// Fetch the byte range `[start, end)`.
    ///
    /// The cache decides whether to call the underlying fetcher or
    /// return already-cached data.
    fn fetch(&mut self, start: u64, end: u64) -> FsResult<Vec<u8>>;

    /// The file size (may be unknown for some caches).
    fn size(&self) -> Option<u64>;
}

// ============================================================================
// NoCache — pass-through
// ============================================================================

/// No caching at all; every `fetch` calls the underlying fetcher.
pub struct NoCache {
    fetcher: Fetcher,
    file_size: Option<u64>,
}

impl NoCache {
    pub fn new(fetcher: Fetcher, file_size: Option<u64>) -> Self {
        Self { fetcher, file_size }
    }
}

impl Cache for NoCache {
    fn fetch(&mut self, start: u64, end: u64) -> FsResult<Vec<u8>> {
        let end = match self.file_size {
            Some(sz) => end.min(sz),
            None => end,
        };
        if start >= end {
            return Ok(Vec::new());
        }
        (self.fetcher)(start, end)
    }

    fn size(&self) -> Option<u64> {
        self.file_size
    }
}

// ============================================================================
// ReadAheadCache — single-block lookahead
// ============================================================================

/// Holds at most one block of data. On a miss, fetches `blocksize`
/// bytes ahead of the requested range. Best for sequential reading.
pub struct ReadAheadCache {
    fetcher: Fetcher,
    file_size: Option<u64>,
    blocksize: u64,
    /// Cached span: data covers `[cache_start, cache_start + data.len())`.
    cache_start: u64,
    data: Vec<u8>,
}

impl ReadAheadCache {
    pub fn new(fetcher: Fetcher, file_size: Option<u64>, blocksize: u64) -> Self {
        Self {
            fetcher,
            file_size,
            blocksize: blocksize.max(1),
            cache_start: 0,
            data: Vec::new(),
        }
    }
}

impl Cache for ReadAheadCache {
    fn fetch(&mut self, start: u64, end: u64) -> FsResult<Vec<u8>> {
        let end = match self.file_size {
            Some(sz) => end.min(sz),
            None => end,
        };
        if start >= end {
            return Ok(Vec::new());
        }

        let cache_end = self.cache_start + self.data.len() as u64;

        // Full hit: [start, end) ⊆ [cache_start, cache_end)
        if start >= self.cache_start && end <= cache_end {
            let lo = (start - self.cache_start) as usize;
            let hi = (end - self.cache_start) as usize;
            return Ok(self.data[lo..hi].to_vec());
        }

        // Partial hit: start is in cache, but end extends beyond
        if start >= self.cache_start && start < cache_end && end > cache_end {
            // Fetch from cache_end to (end + readahead)
            let fetch_end = match self.file_size {
                Some(sz) => (end + self.blocksize).min(sz),
                None => end + self.blocksize,
            };
            let new_data = (self.fetcher)(cache_end, fetch_end)?;

            // Extend cache
            self.data.extend_from_slice(&new_data);

            let new_cache_end = self.cache_start + self.data.len() as u64;
            let actual_end = end.min(new_cache_end);
            let lo = (start - self.cache_start) as usize;
            let hi = (actual_end - self.cache_start) as usize;
            return Ok(self.data[lo..hi].to_vec());
        }

        // Miss: discard cache, fetch [start, start + max(len, blocksize))
        let requested = end - start;
        let fetch_size = requested.max(self.blocksize);
        let fetch_end = match self.file_size {
            Some(sz) => (start + fetch_size).min(sz),
            None => start + fetch_size,
        };
        self.data = (self.fetcher)(start, fetch_end)?;
        self.cache_start = start;

        let available = self.data.len() as u64;
        let actual_end = (end - start).min(available) as usize;
        Ok(self.data[..actual_end].to_vec())
    }

    fn size(&self) -> Option<u64> {
        self.file_size
    }
}

// ============================================================================
// BlockCache — fixed blocks with LRU eviction
// ============================================================================

/// Fixed-size block cache with LRU eviction.
///
/// Data is fetched in `blocksize`-aligned blocks. Up to `max_blocks`
/// blocks are retained; least-recently-used blocks are evicted when
/// the limit is reached.
pub struct BlockCache {
    fetcher: Fetcher,
    file_size: Option<u64>,
    blocksize: u64,
    max_blocks: usize,
    /// (block_index, access_order, data) — sorted by access recency.
    blocks: Vec<CachedBlock>,
    access_counter: u64,
}

struct CachedBlock {
    index: u64,
    last_access: u64,
    data: Vec<u8>,
}

impl BlockCache {
    pub fn new(
        fetcher: Fetcher,
        file_size: Option<u64>,
        blocksize: u64,
        max_blocks: usize,
    ) -> Self {
        Self {
            fetcher,
            file_size,
            blocksize: blocksize.max(1),
            max_blocks: max_blocks.max(1),
            blocks: Vec::new(),
            access_counter: 0,
        }
    }

    /// Get or fetch the block at the given block index. Returns offset
    /// within the block and the data slice.
    fn get_block(&mut self, block_idx: u64) -> FsResult<&[u8]> {
        self.access_counter += 1;
        let counter = self.access_counter;

        // Check if already cached
        if let Some(pos) = self.blocks.iter().position(|b| b.index == block_idx) {
            self.blocks[pos].last_access = counter;
            return Ok(&self.blocks[pos].data);
        }

        // Fetch the block
        let start = block_idx * self.blocksize;
        let end = match self.file_size {
            Some(sz) => (start + self.blocksize).min(sz),
            None => start + self.blocksize,
        };
        let data = (self.fetcher)(start, end)?;

        // Evict LRU if at capacity
        if self.blocks.len() >= self.max_blocks {
            let lru_pos = self
                .blocks
                .iter()
                .enumerate()
                .min_by_key(|(_, b)| b.last_access)
                .map(|(i, _)| i)
                .unwrap();
            self.blocks.swap_remove(lru_pos);
        }

        self.blocks.push(CachedBlock {
            index: block_idx,
            last_access: counter,
            data,
        });

        Ok(&self.blocks.last().unwrap().data)
    }
}

impl Cache for BlockCache {
    fn fetch(&mut self, start: u64, end: u64) -> FsResult<Vec<u8>> {
        let end = match self.file_size {
            Some(sz) => end.min(sz),
            None => end,
        };
        if start >= end {
            return Ok(Vec::new());
        }

        let first_block = start / self.blocksize;
        let last_block = (end - 1) / self.blocksize;
        let blocksize = self.blocksize;

        let mut result = Vec::with_capacity((end - start) as usize);

        for block_idx in first_block..=last_block {
            let block_data = self.get_block(block_idx)?;
            let block_start = block_idx * blocksize;

            let lo = if block_idx == first_block {
                (start - block_start) as usize
            } else {
                0
            };
            let hi = if block_idx == last_block {
                ((end - block_start) as usize).min(block_data.len())
            } else {
                block_data.len()
            };

            if lo < block_data.len() {
                result.extend_from_slice(&block_data[lo..hi]);
            }
        }

        Ok(result)
    }

    fn size(&self) -> Option<u64> {
        self.file_size
    }
}

// ============================================================================
// AllBytesCache — fetch entire file on first access
// ============================================================================

/// Eagerly fetches the entire file on the first read.
/// All subsequent reads are slices of the in-memory buffer.
pub struct AllBytesCache {
    fetcher: Fetcher,
    file_size: Option<u64>,
    data: Option<Vec<u8>>,
}

impl AllBytesCache {
    pub fn new(fetcher: Fetcher, file_size: Option<u64>) -> Self {
        Self {
            fetcher,
            file_size,
            data: None,
        }
    }
}

impl Cache for AllBytesCache {
    fn fetch(&mut self, start: u64, end: u64) -> FsResult<Vec<u8>> {
        // Lazily fetch the entire file
        if self.data.is_none() {
            let sz = self.file_size.unwrap_or(0);
            let all = if sz > 0 {
                (self.fetcher)(0, sz)?
            } else {
                Vec::new()
            };
            self.file_size = Some(all.len() as u64);
            self.data = Some(all);
        }

        let data = self.data.as_ref().unwrap();
        let file_len = data.len() as u64;
        let end = end.min(file_len);
        if start >= end {
            return Ok(Vec::new());
        }
        Ok(data[start as usize..end as usize].to_vec())
    }

    fn size(&self) -> Option<u64> {
        self.file_size
    }
}

// ============================================================================
// Factory
// ============================================================================

/// Build a [`Cache`] from a strategy name and parameters.
pub fn make_cache(
    cache_type: CacheType,
    fetcher: Fetcher,
    file_size: Option<u64>,
    blocksize: u64,
    max_blocks: usize,
) -> Box<dyn Cache> {
    match cache_type {
        CacheType::None => Box::new(NoCache::new(fetcher, file_size)),
        CacheType::ReadAhead => Box::new(ReadAheadCache::new(fetcher, file_size, blocksize)),
        CacheType::Block => Box::new(BlockCache::new(fetcher, file_size, blocksize, max_blocks)),
        CacheType::All => Box::new(AllBytesCache::new(fetcher, file_size)),
    }
}
