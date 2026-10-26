//! One source file written into the lake: every entry's batches to Parquet
//! files by table, shape and partition, each row stamped with its source,
//! and the rows a decoder refused to the quarantine. The files are
//! committed together once the whole file is read, or discarded together:
//! a file goes into the lake whole or not at all.
//!
//! The caller says which entry it reads and what its rows are stamped with,
//! and hands over each block as decoded; how the lake is laid out (which
//! partition a row goes to, when a file is full, what columns a file
//! holds) is this module's alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{ArrowError, Field, Schema, SchemaRef};
use utilake_core::config::Lake;
use utilake_core::{CivilDay, Diagnostic, YearMonth};
use utilake_kit::{Block, LoadMode};

use crate::error::StoreError;
use crate::manifest::EntryShare;
use crate::parquet_hive::ParquetHive;
use crate::partition::{split_by_month, time_bounds};
use crate::provenance::{Stamp, lake_schema, stamped};
use crate::quarantine::QuarantineWriter;
use crate::writer::{FileMeta, LakeWriter, Partition, PartitionKey, WriterId, WrittenFile};

/// The source file a session writes: what every file it writes records of
/// where its rows came from, and the period its rows are of when they carry
/// no time of their own.
#[derive(Debug, Clone)]
pub struct Source {
    /// Its URL.
    pub url: String,
    /// The SHA-256 of its bytes.
    pub sha256: String,
    /// The stem its files are named by.
    pub stem: String,
    /// The month it covers, when it is of one.
    pub period: Option<YearMonth>,
    /// The day it covers, when the source cuts its files by day: a
    /// snapshot it carries is the day's.
    pub day: Option<CivilDay>,
}

/// A shape: a table, the version it was read against and the fingerprint
/// of its header. Two column lists of one version are two shapes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Shape {
    /// The table.
    pub table: String,
    /// Its version.
    pub version: u16,
    /// The header's fingerprint.
    pub fingerprint: u128,
}

impl Shape {
    /// The shape `block` is of.
    pub fn of(block: &Block) -> Self {
        Self {
            table: block.table.name.clone(),
            version: block.version,
            fingerprint: block.fingerprint.0,
        }
    }
}

/// What a session wrote of one shape.
#[derive(Debug, Clone, Default)]
pub struct ShapeOutput {
    /// Its Parquet files, committed when the session was.
    pub outputs: Vec<WrittenFile>,
    /// What each entry gave it, in the order the entries were read.
    pub entries: Vec<EntryShare>,
}

/// What a session wrote.
#[derive(Debug, Default)]
pub struct SessionOutput {
    shapes: HashMap<Shape, ShapeOutput>,
    quarantine: HashMap<String, String>,
}

impl SessionOutput {
    /// What was written of `shape`; nothing when it had no rows.
    pub fn take(&mut self, shape: &Shape) -> ShapeOutput {
        self.shapes.remove(shape).unwrap_or_default()
    }

    /// The quarantine file of `table`, when any of its rows was refused.
    pub fn take_quarantine(&mut self, table: &str) -> Option<String> {
        self.quarantine.remove(table)
    }
}

/// What tells one open writer from another: its shape and partition.
type WriterKey = (Shape, Partition);

/// A writer that is open: its id, when it was last written to (in
/// batches), and the schema of its file.
struct OpenWriter {
    id: WriterId,
    used: u64,
    /// The columns its file takes from batches.
    schema: SchemaRef,
    /// Those, then each row's source and publish time: the file's schema.
    lake: SchemaRef,
}

/// One source file's write into the lake.
pub struct IngestSession {
    writer: ParquetHive,
    /// A file is closed for the next once it holds this many bytes.
    file_bytes: u64,
    /// Open writers by shape and partition.
    writers: HashMap<WriterKey, OpenWriter>,
    /// How many files each key has had opened, so a further file of a
    /// partition gets a name of its own.
    parts: HashMap<WriterKey, u32>,
    /// The schema a key's next file starts with, when its last file was
    /// finished for being too narrow.
    next_schema: HashMap<WriterKey, SchemaRef>,
    /// At most this many writers are open at once.
    max_open: usize,
    /// Counts batches, to tell which writer was used longest ago.
    tick: u64,
    shapes: HashMap<Shape, ShapeOutput>,
    quarantine: HashMap<String, QuarantineWriter>,
    quarantine_root: PathBuf,
    source: Source,
    /// The entry being read; empty for a file read whole.
    entry: String,
    /// What its rows are stamped with.
    stamp: Option<Stamp>,
}

impl IngestSession {
    /// A session writing `source` into the lake at `lake_root` laid out by
    /// `layout`, its refused rows under `quarantine_root`, with at most
    /// `max_open` Parquet files open at once.
    pub fn new(
        lake_root: &Path,
        quarantine_root: &Path,
        layout: Lake,
        max_open: usize,
        source: Source,
    ) -> Self {
        Self {
            writer: ParquetHive::new(lake_root).with_layout(layout),
            file_bytes: u64::from(layout.file_size_mb) << 20,
            writers: HashMap::new(),
            parts: HashMap::new(),
            next_schema: HashMap::new(),
            max_open,
            tick: 0,
            shapes: HashMap::new(),
            quarantine: HashMap::new(),
            quarantine_root: quarantine_root.to_owned(),
            source,
            entry: String::new(),
            stamp: None,
        }
    }

    /// Begins entry `entry` (empty for a file read whole), whose rows are
    /// stamped with `stamp`.
    pub fn begin_entry(&mut self, entry: &str, stamp: Stamp) {
        entry.clone_into(&mut self.entry);
        self.stamp = Some(stamp);
    }

    /// The current entry's share of `shape`.
    fn share(&mut self, shape: &Shape) -> &mut EntryShare {
        let entries = &mut self.shapes.entry(shape.clone()).or_default().entries;
        let at = match entries.iter().rposition(|e| e.entry == self.entry) {
            Some(i) => i,
            None => {
                entries.push(EntryShare {
                    entry: self.entry.clone(),
                    ..EntryShare::default()
                });
                entries.len() - 1
            }
        };
        &mut entries[at]
    }

    /// The partitions `b`'s rows go to, each with its rows.
    fn partitions(&self, b: Block) -> Result<Vec<(Partition, RecordBatch)>, StoreError> {
        let table = &b.table;
        Ok(match (table.load_mode, table.time_column) {
            (LoadMode::Snapshot, _) => vec![(
                match (self.source.day, self.source.period) {
                    (Some(d), _) => Partition::SnapshotDay {
                        year: d.month().year(),
                        month: d.month().month(),
                        day: d.day(),
                    },
                    (None, Some(p)) => Partition::Snapshot {
                        year: p.year(),
                        month: p.month(),
                    },
                    (None, None) => Partition::Unknown,
                },
                b.batch,
            )],
            (LoadMode::Append, Some(tc)) => split_by_month(&b.batch, tc, table.utc_offset_secs)?,
            // No time of its own: the month of the file is all there is.
            (LoadMode::Append, None) => vec![(
                self.source
                    .period
                    .map_or(Partition::Unknown, |p| Partition::Month {
                        year: p.year(),
                        month: p.month(),
                    }),
                b.batch,
            )],
        })
    }

    /// Writes `b`'s rows, each stamped with the current entry's source.
    pub fn write(&mut self, b: Block) -> Result<(), StoreError> {
        let shape = Shape::of(&b);
        let table = b.table.clone();
        let (version, fingerprint, transform) = (b.version, b.fingerprint, b.transform.clone());
        for (partition, batch) in self.partitions(b)? {
            let key: WriterKey = (shape.clone(), partition);
            self.tick += 1;
            // A batch with a column, or a null, its file has no room for
            // finishes that file; the partition goes on in a further one
            // whose schema takes both.
            let open = match self.writers.get_mut(&key) {
                Some(w) => match widened(&w.schema, &batch.schema()) {
                    None => {
                        w.used = self.tick;
                        Some((w.id, w.schema.clone(), w.lake.clone()))
                    }
                    Some(wider) => {
                        let id = w.id;
                        self.writers.remove(&key);
                        self.close_one(&key, id)?;
                        self.next_schema.insert(key.clone(), Arc::new(wider));
                        None
                    }
                },
                None => None,
            };
            let (wid, schema, lake) = match open {
                Some(o) => o,
                None => {
                    // Every open writer holds a file and a row group in
                    // memory. A source whose rows wander over hundreds of
                    // months would hold hundreds of both.
                    if self.writers.len() >= self.max_open {
                        self.close_least_recent()?;
                    }
                    let part = {
                        let n = self.parts.entry(key.clone()).or_insert(0);
                        *n += 1;
                        *n
                    };
                    let pk = PartitionKey {
                        market: table.market.clone(),
                        table_dir: table.dir.clone(),
                        partition,
                    };
                    let meta = FileMeta {
                        transform: (*transform).clone(),
                        part,
                        table: table.name.clone(),
                        source_stem: self.source.stem.clone(),
                        schema_version: version,
                        fingerprint,
                        source_url: self.source.url.clone(),
                        source_sha256: self.source.sha256.clone(),
                        time_column: table.time_column,
                    };
                    // A further file of a key starts as wide as the one
                    // before it ended.
                    let schema = match self.next_schema.remove(&key) {
                        Some(s) => widened(&s, &batch.schema()).map_or(s, Arc::new),
                        None => batch.schema(),
                    };
                    let lake = lake_schema(&schema);
                    let id = self.writer.open(&pk, lake.clone(), &meta)?;
                    self.writers.insert(
                        key.clone(),
                        OpenWriter {
                            id,
                            used: self.tick,
                            schema: schema.clone(),
                            lake: lake.clone(),
                        },
                    );
                    (id, schema, lake)
                }
            };
            let batch = fitted(&batch, &schema)?;
            let bounds = table.time_column.map(|tc| time_bounds(&batch, tc));
            let stamp = self
                .stamp
                .clone()
                .expect("an entry is begun before its rows");
            let batch = stamped(&batch, &stamp, &lake)?;
            self.writer.write(wid, &batch)?;
            let share = self.share(&shape);
            let times = stamp.times();
            share.server_last_modified = times.server_last_modified;
            share.zip_entry_modified = times.zip_entry_modified;
            share.filename_timestamp = times.filename_timestamp;
            share.add(batch.num_rows() as u64, bounds);
            // A file that has grown to its size is closed; the partition
            // goes on in a further one.
            if self.writer.bytes_in_file(wid) >= self.file_bytes {
                self.writers.remove(&key);
                self.close_one(&key, wid)?;
            }
        }
        Ok(())
    }

    /// Keeps a row the decoder refused, `raw` as read, with what was wrong
    /// with it, in its table's quarantine file; `shape` is the one it was
    /// read as, when its block resolved to one.
    pub fn quarantine(
        &mut self,
        table: &str,
        shape: Option<&Shape>,
        raw: &[u8],
        d: &Diagnostic,
    ) -> Result<(), StoreError> {
        if let Some(s) = shape {
            self.share(s).bad_rows += 1;
        }
        let source = format!("{}!{}", self.source.url, self.entry);
        let q = self.quarantine.entry(table.to_owned()).or_insert_with(|| {
            QuarantineWriter::new(&self.quarantine_root, table, &self.source.stem)
        });
        q.write(raw, &source, d)
            .map_err(|e| StoreError::Quarantine {
                path: q.path().to_owned(),
                source: e,
            })
    }

    /// Finishes one writer and files what it wrote under its shape.
    fn close_one(&mut self, key: &WriterKey, wid: WriterId) -> Result<(), StoreError> {
        let written = self.writer.close(wid)?;
        self.shapes
            .entry(key.0.clone())
            .or_default()
            .outputs
            .push(written);
        Ok(())
    }

    /// Finishes the writer that was written to longest ago, to make room
    /// for another. Its file is complete; rows that come for its partition
    /// later go to a further file.
    fn close_least_recent(&mut self) -> Result<(), StoreError> {
        let Some((key, wid)) = self
            .writers
            .iter()
            .min_by_key(|(k, w)| (w.used, (*k).clone()))
            .map(|(k, w)| (k.clone(), w.id))
        else {
            return Ok(());
        };
        self.writers.remove(&key);
        self.close_one(&key, wid)
    }

    /// Finishes every open file, in a fixed order.
    pub fn close(&mut self) -> Result<(), StoreError> {
        zone!(SESSION_CLOSE);
        let mut keys: Vec<_> = self.writers.drain().collect();
        keys.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (key, w) in keys {
            self.close_one(&key, w.id)?;
        }
        Ok(())
    }

    /// The quarantine files, finished.
    fn finish_quarantine(&mut self) -> HashMap<String, String> {
        self.quarantine
            .drain()
            .filter_map(|(table, q)| match q.finish() {
                Ok(Some(p)) => Some((table, p.display().to_string())),
                _ => None,
            })
            .collect()
    }

    /// Puts every file the session wrote in place at once; none is in
    /// place if one is not, and then the output holds no files.
    pub fn commit(mut self) -> (SessionOutput, Result<(), StoreError>) {
        let quarantine = self.finish_quarantine();
        let committed = self.writer.commit();
        if committed.is_err() {
            for s in self.shapes.values_mut() {
                for o in s.outputs.drain(..) {
                    let _ = std::fs::remove_file(&o.path);
                }
            }
        }
        let out = SessionOutput {
            shapes: std::mem::take(&mut self.shapes),
            quarantine,
        };
        (out, committed)
    }

    /// Discards every Parquet file the session wrote: the files still
    /// open, and the ones closed and not yet committed. What it
    /// quarantined stays, to show why.
    pub fn abort(mut self) -> SessionOutput {
        let ids: Vec<WriterId> = self.writers.drain().map(|(_, w)| w.id).collect();
        for w in ids {
            self.writer.abort(w);
        }
        self.writer.discard(0);
        for s in self.shapes.values_mut() {
            s.outputs.clear();
        }
        let quarantine = self.finish_quarantine();
        SessionOutput {
            shapes: std::mem::take(&mut self.shapes),
            quarantine,
        }
    }
}

/// The schema a file must have to take `batch` as well as what it took so
/// far, when `file` is not it: `file`'s columns, nullable where either is,
/// then the columns only `batch` has (a column of values kept as text
/// appears in the batches that need it). `None` when `file` takes `batch`.
fn widened(file: &Schema, batch: &Schema) -> Option<Schema> {
    let mut fields: Vec<Field> = file.fields().iter().map(|f| f.as_ref().clone()).collect();
    let mut wider = false;
    for b in batch.fields() {
        match fields.iter_mut().find(|f| f.name() == b.name()) {
            Some(f) if b.is_nullable() && !f.is_nullable() => {
                *f = f.clone().with_nullable(true);
                wider = true;
            }
            Some(_) => {}
            None => {
                fields.push(b.as_ref().clone());
                wider = true;
            }
        }
    }
    wider.then(|| Schema::new_with_metadata(fields, file.metadata().clone()))
}

/// `batch` in `schema`, which has every column of `batch` and maybe more:
/// those are null.
fn fitted(batch: &RecordBatch, schema: &SchemaRef) -> Result<RecordBatch, ArrowError> {
    zone!(FIT);
    if batch.schema().fields() == schema.fields() {
        return Ok(batch.clone());
    }
    let columns = schema
        .fields()
        .iter()
        .map(|f| match batch.schema().index_of(f.name()) {
            Ok(i) => batch.column(i).clone(),
            Err(_) => arrow_array::new_null_array(f.data_type(), batch.num_rows()),
        })
        .collect();
    RecordBatch::try_new(schema.clone(), columns)
}
