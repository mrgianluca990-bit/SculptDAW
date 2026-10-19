//! A bounded shared PCM page pool with one disk worker. Registration, preparation and shutdown
//! belong to the host/control thread. `StreamView::try_window`, window sampling and guard drop
//! only use preallocated pages and atomics: they do not open files, wait, or allocate.

use crate::stream::{MAX_READ_FRAMES, WavReader};
use crate::{AudioError, AudioInfo, Result};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

const WRITING: usize = 1usize << (usize::BITS - 1);
const INACTIVE: u64 = u64::MAX;
const MAX_VIEWS: usize = 4096;
const MAX_OPEN_FILES: usize = 64;
const MAX_POOL_BYTES: usize = 256 << 20;
/// Bound on pages pinned by one render window; larger spans must be split into smaller blocks.
pub const MAX_WINDOW_PAGES: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct CacheConfig {
    pub page_count: usize,
    pub samples_per_page: usize,
    pub lookahead_pages: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { page_count: 512, samples_per_page: 16_384, lookahead_pages: 2 }
    }
}

impl CacheConfig {
    pub fn pcm_bytes(self) -> Option<usize> {
        self.page_count.checked_mul(self.samples_per_page)?.checked_mul(4)
    }

    fn validate(self) -> Result<Self> {
        if !(2..=4096).contains(&self.page_count)
            || !(1024..=MAX_READ_FRAMES).contains(&self.samples_per_page)
            || self.lookahead_pages > 8
            || self.pcm_bytes().is_none_or(|n| n > MAX_POOL_BYTES)
        {
            return Err(AudioError::TooLarge("invalid page cache configuration (PCM budget must not exceed 256 MiB)".into()));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheError {
    NotReady,
    ReadFailed,
    MediaChanged,
    Capacity,
    Stopped,
    InvalidRange,
    Timeout,
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotReady => "audio pages are not ready",
            Self::ReadFailed => "audio file read failed",
            Self::MediaChanged => "audio file changed after registration",
            Self::Capacity => "active audio windows exceed the cache budget",
            Self::Stopped => "audio cache worker is stopped",
            Self::InvalidRange => "audio window exceeds the supported page span",
            Self::Timeout => "audio preparation timed out",
        })
    }
}
impl std::error::Error for CacheError {}

#[derive(Debug, Default)]
struct Counters {
    read_bytes: AtomicU64,
    decoded_pages: AtomicU64,
    read_failures: AtomicU64,
    cache_misses: AtomicU64,
    ticks: AtomicU64,
    views: AtomicUsize,
    stop: AtomicBool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub read_bytes: u64,
    pub decoded_pages: u64,
    pub read_failures: u64,
    /// Failed realtime window acquisitions, distinct from physical output underruns.
    pub cache_misses: u64,
    pub pcm_bytes: usize,
}

struct Page {
    readers: AtomicUsize,
    source: AtomicU64,
    number: AtomicU64,
    frames: AtomicUsize,
    used: AtomicU64,
    data: Box<[AtomicU32]>,
}

impl Page {
    fn new(samples: usize) -> Result<Self> {
        let mut data = Vec::new();
        data.try_reserve_exact(samples).map_err(|e| AudioError::TooLarge(format!("cannot allocate cache page: {e}")))?;
        data.resize_with(samples, || AtomicU32::new(0));
        Ok(Self {
            readers: AtomicUsize::new(0),
            source: AtomicU64::new(0),
            number: AtomicU64::new(0),
            frames: AtomicUsize::new(0),
            used: AtomicU64::new(0),
            data: data.into_boxed_slice(),
        })
    }

    fn key(&self) -> (u64, u64) {
        (self.source.load(Ordering::Acquire), self.number.load(Ordering::Relaxed))
    }

    #[allow(deprecated)] // fetch_update supports the workspace Rust 1.90 minimum.
    fn pin(&self, key: (u64, u64)) -> Option<PageGuard<'_>> {
        if self.key() != key {
            return None;
        }
        self.readers.fetch_update(Ordering::Acquire, Ordering::Relaxed, |n| (n < WRITING - 1).then_some(n + 1)).ok()?;
        let guard = PageGuard(self);
        if self.key() == key { Some(guard) } else { None }
    }
}

struct PageGuard<'a>(&'a Page);
impl Drop for PageGuard<'_> {
    fn drop(&mut self) {
        self.0.readers.fetch_sub(1, Ordering::Release);
    }
}

struct WriteGuard<'a>(&'a Page);
impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.0.readers.store(0, Ordering::Release);
    }
}

struct Pool {
    config: CacheConfig,
    pages: Box<[Page]>,
    counters: Arc<Counters>,
}

impl Pool {
    fn contains(&self, key: (u64, u64)) -> bool {
        self.pages.iter().any(|p| p.key() == key && p.readers.load(Ordering::Acquire) != WRITING)
    }

    fn victim(&self, protected: &BTreeSet<(u64, u64)>) -> Option<WriteGuard<'_>> {
        let page = self
            .pages
            .iter()
            .filter(|p| p.readers.load(Ordering::Acquire) == 0 && !protected.contains(&p.key()))
            .min_by_key(|p| if p.source.load(Ordering::Relaxed) == 0 { 0 } else { p.used.load(Ordering::Relaxed).saturating_add(1) })?;
        page.readers.compare_exchange(0, WRITING, Ordering::Acquire, Ordering::Relaxed).ok()?;
        page.source.store(0, Ordering::Release);
        Some(WriteGuard(page))
    }
}

/// A host-owned pool. Keep an owning reference outside the callback; lifecycle and shutdown are
/// control-thread operations. Old sources/views may share it through session/undo snapshots.
pub struct StreamCache {
    pool: Arc<Pool>,
    registrations: SyncSender<Weak<ViewState>>,
    next_source: AtomicU64,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl StreamCache {
    pub fn new(config: CacheConfig) -> Result<Arc<Self>> {
        let config = config.validate()?;
        let counters = Arc::new(Counters::default());
        let mut pages = Vec::new();
        pages.try_reserve_exact(config.page_count).map_err(|e| AudioError::TooLarge(e.to_string()))?;
        for _ in 0..config.page_count {
            pages.push(Page::new(config.samples_per_page)?);
        }
        let pool = Arc::new(Pool { config, pages: pages.into_boxed_slice(), counters });
        let (tx, rx) = sync_channel(MAX_VIEWS);
        let worker_pool = Arc::clone(&pool);
        let worker = std::thread::Builder::new()
            .name("sculpt-disk-cache".into())
            .spawn(move || run_worker(worker_pool, rx))
            .map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(Arc::new(Self { pool, registrations: tx, next_source: AtomicU64::new(1), worker: Mutex::new(Some(worker)) }))
    }

    /// Header-only registration; each active worker rechecks the file stamp and header on open.
    #[allow(deprecated)] // fetch_update supports the workspace Rust 1.90 minimum.
    pub fn open_source(self: &Arc<Self>, path: impl AsRef<Path>) -> Result<StreamSource> {
        if self.pool.counters.stop.load(Ordering::Acquire) {
            return Err(AudioError::Io("audio cache worker is stopped".into()));
        }
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).map_err(|e| AudioError::Io(format!("{}: {e}", path.display())))?;
        let stamp = Stamp::from(&file.metadata().map_err(|e| AudioError::Io(e.to_string()))?);
        let reader = WavReader::from_reader(file)?;
        let info = reader.info().clone();
        let id = self
            .next_source
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1).filter(|&next| next != INACTIVE))
            .map_err(|_| AudioError::TooLarge("stream source identity exhausted".into()))?;
        Ok(StreamSource(Arc::new(SourceState { cache: Arc::clone(self), id, path, info, stamp })))
    }

    pub fn stats(&self) -> CacheStats {
        let c = &self.pool.counters;
        CacheStats {
            read_bytes: c.read_bytes.load(Ordering::Relaxed),
            decoded_pages: c.decoded_pages.load(Ordering::Relaxed),
            read_failures: c.read_failures.load(Ordering::Relaxed),
            cache_misses: c.cache_misses.load(Ordering::Relaxed),
            pcm_bytes: self.pool.config.pcm_bytes().unwrap_or(0),
        }
    }

    /// Stop without joining the disk thread from a callback. A bounded join is available only
    /// for the control thread; a stalled OS read never blocks the render thread.
    pub fn shutdown(&self, timeout: Duration) -> std::result::Result<(), CacheError> {
        self.pool.counters.stop.store(true, Ordering::Release);
        let deadline = Instant::now();
        loop {
            let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
            if worker.as_ref().is_none_or(JoinHandle::is_finished) {
                if let Some(handle) = worker.take() {
                    handle.join().map_err(|_| CacheError::ReadFailed)?;
                }
                return Ok(());
            }
            drop(worker);
            if deadline.elapsed() >= timeout {
                return Err(CacheError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for StreamCache {
    fn drop(&mut self) {
        self.pool.counters.stop.store(true, Ordering::Release);
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: Option<SystemTime>,
}
impl From<&Metadata> for Stamp {
    fn from(m: &Metadata) -> Self {
        Self { length: m.len(), modified: m.modified().ok() }
    }
}

struct SourceState {
    cache: Arc<StreamCache>,
    id: u64,
    path: PathBuf,
    info: AudioInfo,
    stamp: Stamp,
}

#[derive(Clone)]
pub struct StreamSource(Arc<SourceState>);

impl StreamSource {
    pub fn info(&self) -> &AudioInfo {
        &self.0.info
    }
    pub fn path(&self) -> &Path {
        &self.0.path
    }

    /// Independent playback position. Clones share requests; use distinct views for concurrent
    /// clips/voices reading different positions of the same source.
    #[allow(deprecated)] // fetch_update supports the workspace Rust 1.90 minimum.
    pub fn view(&self) -> std::result::Result<StreamView, CacheError> {
        let counters = &self.0.cache.pool.counters;
        if counters.stop.load(Ordering::Acquire) {
            return Err(CacheError::Stopped);
        }
        counters.views.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| (n < MAX_VIEWS).then_some(n + 1)).map_err(|_| CacheError::Capacity)?;
        let state = Arc::new(ViewState {
            source: Arc::clone(&self.0),
            wanted: AtomicU64::new(INACTIVE),
            anchor: AtomicU64::new(INACTIVE),
            revision: AtomicU64::new(0),
            ready_revision: AtomicU64::new(INACTIVE),
            failure: AtomicU8::new(0),
            problem: Mutex::new(None),
        });
        self.0.cache.registrations.try_send(Arc::downgrade(&state)).map_err(|_| CacheError::Capacity)?;
        Ok(StreamView(state))
    }
}

struct ViewState {
    source: Arc<SourceState>,
    /// Atomic packed first-page/count; no torn start/end pair when the worker reads a request.
    wanted: AtomicU64,
    anchor: AtomicU64,
    revision: AtomicU64,
    ready_revision: AtomicU64,
    failure: AtomicU8,
    problem: Mutex<Option<String>>,
}

impl Drop for ViewState {
    fn drop(&mut self) {
        self.source.cache.pool.counters.views.fetch_sub(1, Ordering::Release);
    }
}

impl ViewState {
    fn page_frames(&self) -> usize {
        self.source.cache.pool.config.samples_per_page / usize::from(self.source.info.channels).max(1)
    }

    fn encode(&self, start: u64, count: usize) -> std::result::Result<u64, CacheError> {
        let end = start.saturating_add(count as u64).min(self.source.info.frames);
        if start >= end {
            return Ok(INACTIVE);
        }
        let span = self.page_frames() as u64;
        let first = start / span;
        let pages = (end - 1) / span - first + 1;
        if pages > MAX_WINDOW_PAGES as u64 {
            return Err(CacheError::InvalidRange);
        }
        first.checked_mul(64).and_then(|v| v.checked_add(pages)).filter(|&v| v != INACTIVE).ok_or(CacheError::InvalidRange)
    }

    fn failed(&self) -> Option<CacheError> {
        match self.failure.load(Ordering::Acquire) {
            1 => Some(CacheError::ReadFailed),
            2 => Some(CacheError::MediaChanged),
            3 => Some(CacheError::Capacity),
            _ => None,
        }
    }

    fn fail(&self, error: CacheError, message: String) {
        let code = match error {
            CacheError::ReadFailed => 1,
            CacheError::MediaChanged => 2,
            _ => 3,
        };
        if self.failure.compare_exchange(0, code, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            *self.problem.lock().unwrap_or_else(PoisonError::into_inner) = Some(message);
        }
    }
}

#[derive(Clone)]
pub struct StreamView(Arc<ViewState>);

impl StreamView {
    /// Atomic-only request update, safe on the render thread. Prepare before starting/locating.
    pub fn request(&self, start: u64, frames: usize) -> std::result::Result<(), CacheError> {
        let value = self.0.encode(start, frames)?;
        if self.0.wanted.swap(value, Ordering::AcqRel) != value {
            self.0.revision.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// Keep a loop/seek destination ready in addition to the rolling playback position.
    pub fn set_anchor(&self, range: Option<(u64, usize)>) -> std::result::Result<(), CacheError> {
        let encoded = match range {
            Some((start, frames)) => self.0.encode(start, frames)?,
            None => INACTIVE,
        };
        if self.0.anchor.swap(encoded, Ordering::AcqRel) != encoded {
            self.0.revision.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// Atomic-only release of prefetch demand. Failures stay latched on this view; prepare a
    /// new view for an explicit retry rather than racing a late error against a reset.
    pub fn deactivate(&self) {
        if self.0.wanted.swap(INACTIVE, Ordering::AcqRel) != INACTIVE {
            self.0.revision.fetch_add(1, Ordering::Release);
        }
        if self.0.anchor.swap(INACTIVE, Ordering::AcqRel) != INACTIVE {
            self.0.revision.fetch_add(1, Ordering::Release);
        }
    }

    pub fn problem(&self) -> Option<String> {
        self.0.problem.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn prepare(&self, start: u64, frames: usize, timeout: Duration) -> std::result::Result<(), CacheError> {
        self.request(start, frames)?;
        let began = Instant::now();
        loop {
            match self.pin(start, frames) {
                Ok(_)
                    if (self.0.wanted.load(Ordering::Acquire) == INACTIVE && self.0.anchor.load(Ordering::Acquire) == INACTIVE)
                        || self.0.ready_revision.load(Ordering::Acquire) == self.0.revision.load(Ordering::Acquire) =>
                {
                    return Ok(());
                }
                Ok(_) => {}
                Err(CacheError::NotReady) => {}
                Err(error) => return Err(error),
            }
            if began.elapsed() >= timeout {
                return Err(CacheError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Pin every page in the window or return an explicit failure; never return partial audio.
    pub fn try_window(&self, start: u64, frames: usize) -> std::result::Result<CacheWindow<'_>, CacheError> {
        self.request(start, frames)?;
        let result = self.pin(start, frames);
        if matches!(result, Err(CacheError::NotReady)) {
            self.0.source.cache.pool.counters.cache_misses.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn pin(&self, start: u64, frames: usize) -> std::result::Result<CacheWindow<'_>, CacheError> {
        let pool = &self.0.source.cache.pool;
        if pool.counters.stop.load(Ordering::Acquire) {
            return Err(CacheError::Stopped);
        }
        if let Some(error) = self.0.failed() {
            return Err(error);
        }
        let encoded = self.0.encode(start, frames)?;
        let mut window = CacheWindow {
            pages: std::array::from_fn(|_| None),
            page_frames: self.0.page_frames(),
            channels: usize::from(self.0.source.info.channels),
            start,
            end: start.saturating_add(frames as u64).min(self.0.source.info.frames),
        };
        if encoded == INACTIVE {
            return Ok(window);
        }
        let first = encoded / 64;
        let count = (encoded % 64) as usize;
        for (slot, number) in window.pages.iter_mut().take(count).zip(first..first + count as u64) {
            let guard = pool.pages.iter().find_map(|p| p.pin((self.0.source.id, number))).ok_or(CacheError::NotReady)?;
            guard.0.used.store(pool.counters.ticks.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            *slot = Some(guard);
        }
        Ok(window)
    }
}

pub struct CacheWindow<'a> {
    pages: [Option<PageGuard<'a>>; MAX_WINDOW_PAGES],
    page_frames: usize,
    channels: usize,
    start: u64,
    end: u64,
}

impl CacheWindow<'_> {
    pub fn frames(&self) -> usize {
        self.end.saturating_sub(self.start) as usize
    }

    pub fn sample(&self, channel: usize, frame: u64) -> f32 {
        if channel >= self.channels || frame < self.start || frame >= self.end {
            return 0.0;
        }
        let number = frame / self.page_frames as u64;
        let local = (frame % self.page_frames as u64) as usize;
        for guard in self.pages.iter().flatten() {
            let p = guard.0;
            if p.number.load(Ordering::Relaxed) == number && local < p.frames.load(Ordering::Relaxed) {
                return p.data.get(channel * self.page_frames + local).map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed)));
            }
        }
        0.0
    }
}

struct MeasuredFile {
    file: File,
    counters: Arc<Counters>,
}
impl Read for MeasuredFile {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(out)?;
        self.counters.read_bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}
impl Seek for MeasuredFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(to)
    }
}

struct OpenFile {
    source: u64,
    reader: WavReader<MeasuredFile>,
    used: u64,
}

struct ViewRequest {
    view: Arc<ViewState>,
    revision: u64,
    wanted: u64,
    anchor: u64,
}
impl ViewRequest {
    fn snapshot(view: Arc<ViewState>) -> Option<Self> {
        let revision = view.revision.load(Ordering::Acquire);
        let wanted = view.wanted.load(Ordering::Acquire);
        let anchor = view.anchor.load(Ordering::Acquire);
        if view.revision.load(Ordering::Acquire) != revision || (wanted == INACTIVE && anchor == INACTIVE) {
            return None;
        }
        Some(Self { view, revision, wanted, anchor })
    }
    fn current(&self) -> bool {
        self.view.revision.load(Ordering::Acquire) == self.revision
            && self.view.wanted.load(Ordering::Acquire) == self.wanted
            && self.view.anchor.load(Ordering::Acquire) == self.anchor
    }
    fn pages(&self) -> impl Iterator<Item = u64> {
        [self.wanted, self.anchor].into_iter().filter(|&v| v != INACTIVE).flat_map(|v| v / 64..v / 64 + v % 64)
    }
}

struct ReadPlan {
    protected: BTreeSet<(u64, u64)>,
    order: Vec<(u64, u64)>,
}

fn requests(active: &[ViewRequest], pool: &Pool) -> Option<ReadPlan> {
    let mut required = BTreeSet::new();
    for request in active.iter().filter(|r| r.view.failed().is_none()) {
        for page in request.pages() {
            required.insert((request.view.source.id, page));
            if required.len() > pool.pages.len() {
                if active.iter().all(ViewRequest::current) {
                    for r in active {
                        r.view.fail(
                            CacheError::Capacity,
                            "Active audio windows exceed the fixed PCM page budget; increase the cache or reduce simultaneous windows.".into(),
                        );
                    }
                }
                return None;
            }
        }
    }
    let mut order: Vec<_> = required.iter().copied().collect();
    // Round-robin lookahead, admitted only after all required windows and anchors fit.
    for ahead in 0..pool.config.lookahead_pages as u64 {
        for request in active.iter().filter(|r| r.view.failed().is_none()) {
            for value in [request.wanted, request.anchor] {
                if value == INACTIVE || required.len() >= pool.pages.len() {
                    continue;
                }
                let page = value / 64 + value % 64 + ahead;
                if page.checked_mul(request.view.page_frames() as u64).is_some_and(|start| start < request.view.source.info.frames)
                    && required.insert((request.view.source.id, page))
                {
                    order.push((request.view.source.id, page));
                }
            }
        }
    }
    Some(ReadPlan { protected: required, order })
}

fn fill(
    pool: &Pool,
    source: &SourceState,
    number: u64,
    slot: &WriteGuard<'_>,
    files: &mut Vec<OpenFile>,
) -> std::result::Result<(), (CacheError, String)> {
    let now = pool.counters.ticks.fetch_add(1, Ordering::Relaxed);
    let index = match files.iter().position(|f| f.source == source.id) {
        Some(index) => index,
        None => {
            let file = File::open(&source.path).map_err(|e| (CacheError::ReadFailed, format!("{}: {e}", source.path.display())))?;
            let stamp = Stamp::from(&file.metadata().map_err(|e| (CacheError::ReadFailed, e.to_string()))?);
            if stamp != source.stamp {
                return Err((CacheError::MediaChanged, format!("{} changed after registration", source.path.display())));
            }
            let reader = WavReader::from_reader(MeasuredFile { file, counters: Arc::clone(&pool.counters) })
                .map_err(|e| (CacheError::ReadFailed, e.to_string()))?;
            if reader.info() != &source.info {
                return Err((CacheError::MediaChanged, "Audio header changed after registration".into()));
            }
            if files.len() >= MAX_OPEN_FILES
                && let Some((oldest, _)) = files.iter().enumerate().min_by_key(|(_, f)| f.used)
            {
                files.swap_remove(oldest);
            }
            files.push(OpenFile { source: source.id, reader, used: now });
            files.len() - 1
        }
    };
    let file = files.get_mut(index).ok_or_else(|| (CacheError::ReadFailed, "Audio file slot is unavailable".into()))?;
    file.used = now;
    let channels = usize::from(source.info.channels);
    let frames = pool.config.samples_per_page / channels.max(1);
    let start = number.checked_mul(frames as u64).ok_or_else(|| (CacheError::ReadFailed, "Audio page offset overflow".into()))?;
    let decoded = file.reader.read_frames(start, frames).map_err(|e| (CacheError::ReadFailed, format!("{}: {e}", source.path.display())))?;
    for (channel, input) in decoded.channels.iter().enumerate() {
        for (frame, &value) in input.iter().enumerate() {
            if let Some(out) = slot.0.data.get(channel * frames + frame) {
                out.store(value.to_bits(), Ordering::Relaxed);
            }
        }
    }
    slot.0.frames.store(decoded.frames(), Ordering::Relaxed);
    slot.0.number.store(number, Ordering::Relaxed);
    slot.0.used.store(now, Ordering::Relaxed);
    slot.0.source.store(source.id, Ordering::Relaxed);
    pool.counters.decoded_pages.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn run_worker(pool: Arc<Pool>, rx: Receiver<Weak<ViewState>>) {
    let mut views = Vec::<Weak<ViewState>>::new();
    let mut files = Vec::<OpenFile>::new();
    while !pool.counters.stop.load(Ordering::Acquire) {
        for view in rx.try_iter().take(MAX_VIEWS) {
            views.push(view);
        }
        views.retain(|v| v.strong_count() > 0);
        let active: Vec<_> = views.iter().filter_map(Weak::upgrade).filter(|v| v.failed().is_none()).filter_map(ViewRequest::snapshot).collect();
        if let Some(plan) = requests(&active, &pool) {
            for key in &plan.order {
                if pool.counters.stop.load(Ordering::Acquire) {
                    break;
                }
                if pool.contains(*key) {
                    continue;
                }
                let Some(source) = active.iter().find(|r| r.view.source.id == key.0 && r.view.failed().is_none()).map(|r| &r.view.source) else {
                    continue;
                };
                // A pinned obsolete page can temporarily delay replacement; retry next pass.
                // Do not report capacity failure just because a render still holds a guard.
                let Some(slot) = pool.victim(&plan.protected) else { continue };
                if let Err((code, message)) = fill(&pool, source, key.1, &slot, &mut files) {
                    pool.counters.read_failures.fetch_add(1, Ordering::Relaxed);
                    for r in active.iter().filter(|r| r.view.source.id == key.0) {
                        r.view.fail(code, message.clone());
                    }
                }
            }
            for request in &active {
                if request.current() && request.view.failed().is_none() && request.pages().all(|p| pool.contains((request.view.source.id, p))) {
                    request.view.ready_revision.store(request.revision, Ordering::Release);
                }
            }
        }
        files.retain(|f| active.iter().any(|r| r.view.source.id == f.source && r.view.failed().is_none()));
        let idle = active.is_empty();
        drop(active);
        std::thread::sleep(Duration::from_millis(if idle { 10 } else { 2 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioBuffer, BitDepth, EncodeOptions};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(channels: usize) -> Self {
            let path = std::env::temp_dir().join(format!("sculpt-cache-{}-{}.wav", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            let audio = AudioBuffer { sample_rate: 48_000, channels: (0..channels).map(|c| (0..30_000).map(|f| value(c, f)).collect()).collect() };
            let bytes = crate::encode(&audio, &EncodeOptions { bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() }).unwrap();
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn value(channel: usize, frame: usize) -> f32 {
        ((frame * 13 + channel * 71) as f32 * 0.015).sin() * 0.75
    }
    fn cache(pages: usize) -> Arc<StreamCache> {
        StreamCache::new(CacheConfig { page_count: pages, samples_per_page: 1024, lookahead_pages: 0 }).unwrap()
    }
    fn ready(view: &StreamView, start: u64, frames: usize) {
        view.prepare(start, frames, Duration::from_secs(3)).unwrap();
    }
    fn verify(window: &CacheWindow<'_>, channels: usize, start: u64) {
        for c in 0..channels {
            for f in start..start + window.frames() as u64 {
                assert_eq!(window.sample(c, f).to_bits(), value(c, f as usize).to_bits());
            }
        }
    }
    #[test]
    fn multichannel_windows_anchor_eviction_and_bounded_reads() {
        for channels in [1, 2, 7] {
            let fixture = Fixture::new(channels);
            let cache = cache(12);
            let source = cache.open_source(&fixture.0).unwrap();
            let view = source.view().unwrap();
            view.set_anchor(Some((20_000, 210))).unwrap();
            ready(&view, 137, 410);
            verify(&view.try_window(137, 410).unwrap(), channels, 137);
            verify(&view.pin(20_000, 210).unwrap(), channels, 20_000);
            for start in [6000, 12000, 18000, 137] {
                ready(&view, start, 410);
                verify(&view.try_window(start, 410).unwrap(), channels, start);
            }
            assert_eq!(cache.stats().pcm_bytes, 12 * 1024 * 4);
            assert!(cache.stats().read_bytes < std::fs::metadata(&fixture.0).unwrap().len());
            cache.shutdown(Duration::from_secs(1)).unwrap();
            assert!(matches!(view.try_window(0, 1), Err(CacheError::Stopped)));
        }
    }
    #[test]
    fn pinned_obsolete_pages_delay_replacement_without_corrupting_audio() {
        let fixture = Fixture::new(1);
        let cache = cache(2);
        let source = cache.open_source(&fixture.0).unwrap();
        let view = source.view().unwrap();
        ready(&view, 0, 2048);
        let old = view.try_window(0, 2048).unwrap();
        assert_eq!(view.prepare(8192, 1500, Duration::from_millis(40)), Err(CacheError::Timeout));
        verify(&old, 1, 0);
        assert!(view.0.failed().is_none());
        drop(old);
        ready(&view, 8192, 1500);
        verify(&view.try_window(8192, 1500).unwrap(), 1, 8192);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn capacity_failure_is_explicit_and_latched_new_view_can_retry() {
        let fixture = Fixture::new(1);
        let cache = cache(2);
        let source = cache.open_source(&fixture.0).unwrap();
        let view = source.view().unwrap();
        assert_eq!(view.prepare(0, 3072, Duration::from_secs(3)), Err(CacheError::Capacity));
        view.deactivate();
        assert_eq!(view.prepare(0, 10, Duration::from_secs(1)), Err(CacheError::Capacity));
        view.deactivate();
        let retry = source.view().unwrap();
        ready(&retry, 0, 10);
        verify(&retry.try_window(0, 10).unwrap(), 1, 0);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn changed_and_truncated_media_do_not_produce_silent_success() {
        let fixture = Fixture::new(2);
        let cache = cache(4);
        let source = cache.open_source(&fixture.0).unwrap();
        File::options().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
        let view = source.view().unwrap();
        assert_eq!(view.prepare(0, 10, Duration::from_secs(3)), Err(CacheError::MediaChanged));
        cache.shutdown(Duration::from_secs(1)).unwrap();
        let fixture = Fixture::new(2);
        let cache = self::cache(4);
        let source = cache.open_source(&fixture.0).unwrap();
        let view = source.view().unwrap();
        ready(&view, 0, 10);
        File::options().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
        assert_eq!(view.prepare(20_000, 10, Duration::from_secs(3)), Err(CacheError::ReadFailed));
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn independent_views_and_sources_never_alias() {
        let a = Fixture::new(1);
        let b = Fixture::new(7);
        let cache = cache(12);
        let sa = cache.open_source(&a.0).unwrap();
        let sb = cache.open_source(&b.0).unwrap();
        let va = sa.view().unwrap();
        let va2 = sa.view().unwrap();
        let vb = sb.view().unwrap();
        ready(&va, 0, 100);
        ready(&va2, 20_000, 100);
        ready(&vb, 8000, 600);
        for _ in 0..40 {
            verify(&va.try_window(0, 100).unwrap(), 1, 0);
            verify(&va2.try_window(20_000, 100).unwrap(), 1, 20_000);
            verify(&vb.try_window(8000, 600).unwrap(), 7, 8000);
        }
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn simultaneous_positions_remain_exact_during_repeated_evictions() {
        let fixture = Fixture::new(2);
        let cache = cache(8);
        let source = cache.open_source(&fixture.0).unwrap();
        let a = source.view().unwrap();
        let b = source.view().unwrap();
        std::thread::scope(|scope| {
            let run = |view: &StreamView, offset: u64| {
                for n in 0..60 {
                    let start = offset + (n * 251 % 8000);
                    ready(view, start, 400);
                    let window = view.try_window(start, 400).unwrap();
                    std::thread::yield_now();
                    verify(&window, 2, start);
                }
            };
            scope.spawn(move || run(&a, 0));
            scope.spawn(move || run(&b, 16_000));
        });
        assert!(cache.stats().decoded_pages > 20);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn invalid_config_ranges_and_empty_eof_are_bounded() {
        assert!(StreamCache::new(CacheConfig { page_count: usize::MAX, ..CacheConfig::default() }).is_err());
        let fixture = Fixture::new(1);
        let cache = cache(2);
        let source = cache.open_source(&fixture.0).unwrap();
        let view = source.view().unwrap();
        let many_channels = Fixture::new(7);
        let multichannel = cache.open_source(&many_channels.0).unwrap().view().unwrap();
        assert_eq!(multichannel.request(0, 33 * 1024), Err(CacheError::InvalidRange));
        // EOF is an immediately available empty window, even without disk preparation.
        ready(&view, u64::MAX, 10);
        assert_eq!(view.try_window(u64::MAX, 10).unwrap().frames(), 0);
        assert_eq!(cache.stats().cache_misses, 0);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
}
