//! Pack store: objects and refs in one append-only log of segment files (ADR-0009).
//!
//! * Writes append records to the active segment; nothing is ever rewritten in place.
//! * One `fdatasync` of the active segment makes every object and ref appended so far
//!   durable. A ref record is appended after the objects it names, and recovery trusts the
//!   log only up to its first damaged record, so a surviving ref never names a lost object.
//! * An in-memory index (hash to location) and ref table are rebuilt by scanning the
//!   segments when the store opens.
//! * One process writes at a time (`WRITER` lock, taken on the first write); any number of
//!   processes may read.
//! * Space is reclaimed by compaction: live records are copied into fresh segments, then
//!   the old segments are deleted.

mod record;
mod segment;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime};

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::ObjectKind;
use crate::refs::{validate_ref_name, Refs};
use crate::store::counters::Counters;
use crate::store::{ObjectInfo, ObjectStore, StoreStats, SweepReport};

use record::{Entry, HEADER_LEN, OBJECT_PREFIX};
pub use segment::DEFAULT_SEGMENT_LIMIT;

const WRITER_LOCK: &str = "WRITER";

/// Where an object's data lives.
#[derive(Clone, Copy, Debug)]
struct Loc {
    segment: u32,
    offset: u64,
    len: u32,
    kind: ObjectKind,
}

/// Everything rebuilt from the log.
#[derive(Default)]
struct State {
    index: HashMap<Hash, Loc>,
    refs: BTreeMap<String, Hash>,
    segments: BTreeMap<u32, Arc<File>>,
}

/// The append position, held only by the process that owns the `WRITER` lock.
struct Writer {
    _lock: File,
    segment: u32,
    file: Arc<File>,
    offset: u64,
    /// Data appended since the last `fdatasync`.
    unsynced: bool,
    /// A segment file was created since the directory was last fsynced.
    new_segment: bool,
    /// Offset proven durable by the last `fdatasync`, to be recorded as a `synced` marker in
    /// front of the next record (see `record`).
    marker: Option<u64>,
}

pub struct PackStore {
    dir: PathBuf,
    verify_reads: bool,
    segment_limit: u64,
    state: RwLock<State>,
    writer: Mutex<Option<Writer>>,
    counters: Counters,
}

impl PackStore {
    /// Open (creating if needed) the log in `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let (state, _) = load(&dir)?;
        Ok(PackStore {
            dir,
            verify_reads: true,
            segment_limit: DEFAULT_SEGMENT_LIMIT,
            state: RwLock::new(state),
            writer: Mutex::new(None),
            counters: Counters::default(),
        })
    }

    /// Start a new segment once the active one would exceed `bytes` (tests use small values).
    pub fn with_segment_limit(mut self, bytes: u64) -> Self {
        self.segment_limit = bytes.max(1);
        self
    }

    /// Number of segment files (for tests and diagnostics).
    pub fn segment_count(&self) -> usize {
        self.state.read().unwrap().segments.len()
    }

    /// The writer, taking the `WRITER` lock and reloading the log on first use.
    fn writer<'a>(&self, guard: &'a mut MutexGuard<'_, Option<Writer>>) -> Result<&'a mut Writer> {
        if guard.is_none() {
            **guard = Some(self.become_writer()?);
        }
        Ok(guard.as_mut().expect("writer was just set"))
    }

    fn become_writer(&self) -> Result<Writer> {
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(WRITER_LOCK))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(Error::Locked),
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        // Another writer may have appended since this store was opened: reload, then cut
        // off a torn tail so new records follow a valid prefix.
        let (mut state, tail) = load(&self.dir)?;
        let writer = match tail {
            Some((id, valid_len)) => {
                let file = Arc::clone(&state.segments[&id]);
                if file.metadata()?.len() > valid_len {
                    file.set_len(valid_len)?;
                    file.sync_all()?;
                }
                Writer {
                    _lock: lock,
                    segment: id,
                    file,
                    offset: valid_len,
                    unsynced: false,
                    new_segment: false,
                    marker: None,
                }
            }
            None => {
                let file = Arc::new(segment::create(&self.dir, 1)?);
                state.segments.insert(1, Arc::clone(&file));
                Writer {
                    _lock: lock,
                    segment: 1,
                    file,
                    offset: 0,
                    unsynced: false,
                    new_segment: true,
                    marker: None,
                }
            }
        };
        *self.state.write().unwrap() = state;
        Ok(writer)
    }

    /// Append one record; returns its segment and start offset.
    fn append(&self, w: &mut Writer, bytes: &[u8]) -> Result<(u32, u64)> {
        if w.offset > 0 && w.offset + bytes.len() as u64 > self.segment_limit {
            self.rotate(w)?;
        }
        if let Some(upto) = w.marker.take() {
            let marker = record::synced(upto);
            w.file.write_all_at(&marker, w.offset)?;
            w.offset += marker.len() as u64;
        }
        w.file.write_all_at(bytes, w.offset)?;
        let start = w.offset;
        w.offset += bytes.len() as u64;
        w.unsynced = true;
        Ok((w.segment, start))
    }

    /// Seal the active segment (durable) and continue in a new one.
    fn rotate(&self, w: &mut Writer) -> Result<()> {
        w.file.sync_data()?;
        let id = w.segment + 1;
        let file = Arc::new(segment::create(&self.dir, id)?);
        self.state
            .write()
            .unwrap()
            .segments
            .insert(id, Arc::clone(&file));
        w.segment = id;
        w.file = file;
        w.offset = 0;
        w.unsynced = false;
        w.new_segment = true;
        w.marker = None;
        Ok(())
    }

    /// Make everything appended so far durable: one `fdatasync`, plus a directory fsync if a
    /// segment was created.
    fn sync_writer(&self, w: &mut Writer) -> Result<()> {
        if w.unsynced {
            w.file.sync_data()?;
            w.unsynced = false;
            w.marker = Some(w.offset);
        }
        if w.new_segment {
            File::open(&self.dir)?.sync_all()?;
            w.new_segment = false;
        }
        Ok(())
    }

    /// Append a ref record and make it durable together with all earlier records.
    fn append_ref(&self, record: &[u8]) -> Result<()> {
        let mut guard = self.writer.lock().unwrap();
        let w = self.writer(&mut guard)?;
        self.append(w, record)?;
        self.sync_writer(w)
    }

    fn lookup(&self, hash: &Hash) -> Option<(Loc, Arc<File>)> {
        let state = self.state.read().unwrap();
        let loc = *state.index.get(hash)?;
        Some((loc, Arc::clone(&state.segments[&loc.segment])))
    }
}

/// Rebuild the state from every segment. Returns the last segment and its valid length.
fn load(dir: &Path) -> Result<(State, Option<(u32, u64)>)> {
    let mut state = State::default();
    let ids = segment::list(dir)?;
    let mut tail = None;
    for (i, &id) in ids.iter().enumerate() {
        let file = segment::open(dir, id)?;
        let active = i + 1 == ids.len();
        let scan = record::scan(&file, active)?;
        if let Some(at) = scan.corrupt_at {
            return Err(Error::corrupt(format!(
                "pack segment {id} is damaged at byte {at}, inside data that was already durable"
            )));
        }
        if !active && scan.valid_len != file.metadata()?.len() {
            return Err(Error::corrupt(format!(
                "sealed pack segment {id} is damaged at byte {}",
                scan.valid_len
            )));
        }
        apply(&mut state, id, scan.entries);
        if active {
            tail = Some((id, scan.valid_len));
        }
        state.segments.insert(id, Arc::new(file));
    }
    Ok((state, tail))
}

fn apply(state: &mut State, segment: u32, entries: Vec<Entry>) {
    for entry in entries {
        match entry {
            Entry::Object {
                hash,
                kind,
                data_offset,
                data_len,
            } => {
                state.index.insert(
                    hash,
                    Loc {
                        segment,
                        offset: data_offset,
                        len: data_len,
                        kind,
                    },
                );
            }
            Entry::RefSet { name, hash } => {
                state.refs.insert(name, hash);
            }
            Entry::RefDelete { name } => {
                state.refs.remove(&name);
            }
            Entry::Synced { .. } => {}
        }
    }
}

impl ObjectStore for PackStore {
    fn put(&self, kind: ObjectKind, payload: &[u8]) -> Result<Hash> {
        let hash = Hash::of_object(kind, payload);
        let stored_len = payload.len() as u64 + 1;
        if self.has(&hash)? {
            self.counters.deduplicated(stored_len);
            return Ok(hash);
        }
        let mut guard = self.writer.lock().unwrap();
        let w = self.writer(&mut guard)?;
        // Check again under the writer lock: another thread, or the reload done when this
        // process became the writer, may have added it.
        if self.has(&hash)? {
            self.counters.deduplicated(stored_len);
            return Ok(hash);
        }
        let (segment, start) = self.append(w, &record::object(&hash, kind, payload))?;
        self.state.write().unwrap().index.insert(
            hash,
            Loc {
                segment,
                offset: start + HEADER_LEN + OBJECT_PREFIX,
                len: payload.len() as u32,
                kind,
            },
        );
        self.counters.stored_new(stored_len);
        Ok(hash)
    }

    fn get(&self, hash: &Hash) -> Result<(ObjectKind, Vec<u8>)> {
        self.counters.read();
        let (loc, file) = self.lookup(hash).ok_or(Error::NotFound(*hash))?;
        let mut data = vec![0u8; loc.len as usize];
        file.read_exact_at(&mut data, loc.offset)?;
        if self.verify_reads {
            let actual = Hash::of_object(loc.kind, &data);
            if actual != *hash {
                return Err(Error::HashMismatch {
                    expected: *hash,
                    actual,
                });
            }
        }
        Ok((loc.kind, data))
    }

    fn has(&self, hash: &Hash) -> Result<bool> {
        Ok(self.state.read().unwrap().index.contains_key(hash))
    }

    fn delete(&self, _hash: &Hash) -> Result<()> {
        Err(Error::Invalid(
            "the pack store reclaims space only through gc compaction".into(),
        ))
    }

    fn list(&self) -> Result<Vec<ObjectInfo>> {
        let state = self.state.read().unwrap();
        let mut mtimes = HashMap::new();
        for (&id, file) in &state.segments {
            mtimes.insert(id, file.metadata()?.modified()?);
        }
        Ok(state
            .index
            .iter()
            .map(|(hash, loc)| ObjectInfo {
                hash: *hash,
                size: u64::from(loc.len) + 1,
                modified: mtimes
                    .get(&loc.segment)
                    .copied()
                    .unwrap_or(SystemTime::UNIX_EPOCH),
            })
            .collect())
    }

    fn list_prefix(&self, hex_prefix: &str) -> Result<Vec<Hash>> {
        if hex_prefix.len() < 2 || !hex_prefix.is_ascii() {
            return Err(Error::Invalid(
                "hash prefix must be at least 2 hex characters".into(),
            ));
        }
        let state = self.state.read().unwrap();
        let mut out: Vec<Hash> = state
            .index
            .keys()
            .filter(|h| h.to_hex().starts_with(hex_prefix))
            .copied()
            .collect();
        out.sort();
        Ok(out)
    }

    fn sync(&self) -> Result<()> {
        let mut guard = self.writer.lock().unwrap();
        match guard.as_mut() {
            Some(w) => self.sync_writer(w),
            None => Ok(()),
        }
    }

    /// Compaction. The writer lock guarantees no concurrent writer, so no grace period is
    /// needed: every unreachable object can go.
    fn sweep(&self, keep: &HashSet<Hash>, _grace: Duration, dry_run: bool) -> Result<SweepReport> {
        let mut report = SweepReport::default();
        let live: Vec<(Hash, Loc)> = {
            let state = self.state.read().unwrap();
            for (hash, loc) in &state.index {
                report.examined += 1;
                if keep.contains(hash) {
                    report.reachable += 1;
                } else {
                    report.deleted += 1;
                    report.bytes_freed += u64::from(loc.len) + 1;
                }
            }
            let mut live: Vec<(Hash, Loc)> = state
                .index
                .iter()
                .filter(|(h, _)| keep.contains(*h))
                .map(|(h, l)| (*h, *l))
                .collect();
            live.sort_by_key(|(_, l)| (l.segment, l.offset));
            live
        };
        if dry_run || report.deleted == 0 {
            return Ok(report);
        }
        self.compact(&live)?;
        Ok(report)
    }

    fn stats(&self) -> StoreStats {
        self.counters.snapshot()
    }
}

impl PackStore {
    /// Copy `live` objects and all refs into fresh segments, then drop the old segments.
    /// A crash before the old segments are deleted leaves duplicates, which are harmless:
    /// the later copy wins when the log is replayed.
    fn compact(&self, live: &[(Hash, Loc)]) -> Result<()> {
        let mut guard = self.writer.lock().unwrap();
        let w = self.writer(&mut guard)?;
        self.sync_writer(w)?;
        self.rotate(w)?;
        let first_new = w.segment;

        let refs = self.state.read().unwrap().refs.clone();
        for (hash, loc) in live {
            let file = Arc::clone(&self.state.read().unwrap().segments[&loc.segment]);
            let mut data = vec![0u8; loc.len as usize];
            file.read_exact_at(&mut data, loc.offset)?;
            self.append(w, &record::object(hash, loc.kind, &data))?;
        }
        for (name, hash) in &refs {
            self.append(w, &record::ref_set(name, hash))?;
        }
        self.sync_writer(w)?;

        let old: Vec<u32> = segment::list(&self.dir)?
            .into_iter()
            .filter(|&id| id < first_new)
            .collect();
        for id in old {
            fs::remove_file(segment::path(&self.dir, id))?;
        }
        File::open(&self.dir)?.sync_all()?;

        let (state, _) = load(&self.dir)?;
        w.file = Arc::clone(&state.segments[&w.segment]);
        *self.state.write().unwrap() = state;
        Ok(())
    }
}

impl Refs for PackStore {
    fn read(&self, name: &str) -> Result<Option<Hash>> {
        validate_ref_name(name)?;
        Ok(self.state.read().unwrap().refs.get(name).copied())
    }

    fn write(&self, name: &str, hash: &Hash) -> Result<()> {
        validate_ref_name(name)?;
        self.append_ref(&record::ref_set(name, hash))?;
        self.state
            .write()
            .unwrap()
            .refs
            .insert(name.to_string(), *hash);
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        if Refs::read(self, name)?.is_none() {
            return Err(Error::RefNotFound(name.to_string()));
        }
        self.append_ref(&record::ref_delete(name))?;
        self.state.write().unwrap().refs.remove(name);
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<(String, Hash)>> {
        validate_ref_name(prefix)?;
        let prefix = format!("{prefix}/");
        let state = self.state.read().unwrap();
        Ok(state
            .refs
            .range(prefix.clone()..)
            .take_while(|(name, _)| name.starts_with(&prefix))
            .map(|(name, hash)| (name[prefix.len()..].to_string(), *hash))
            .collect())
    }
}
