//! The plugin log: a ring buffer of every plugin's records, shared by the
//! runtime, each plugin's stores and whatever shows the log.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::model;
use crate::sourcemap::SourceMap;

/// Shared ring buffer of plugin log records.
#[derive(Clone, Default)]
pub struct Logs(Arc<Mutex<LogBuf>>);

#[derive(Default)]
struct LogBuf {
    records: VecDeque<model::LogRecord>,
    capacity: usize,
    /// Total records ever pushed; lets readers poll for new ones by sequence.
    total: u64,
    /// Source maps for JS plugins, by id: their stack frames are rewritten
    /// to the original files as the records come in.
    maps: HashMap<String, Arc<SourceMap>>,
}

impl Logs {
    pub(super) fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(LogBuf {
            capacity,
            ..Default::default()
        })))
    }

    /// Rewrite stack frames in `plugin`'s log lines with `map`, or stop
    /// rewriting them with `None`.
    pub fn set_source_map(&self, plugin: &str, map: Option<Arc<SourceMap>>) {
        let mut b = self.0.lock().unwrap();
        match map {
            Some(m) => b.maps.insert(plugin.to_string(), m),
            None => b.maps.remove(plugin),
        };
    }

    /// Add a record, and pass it to `tracing`. The oldest record goes once
    /// the buffer is full.
    pub fn push(&self, mut rec: model::LogRecord) {
        if rec.message.contains("entry.mjs:")
            && let Some(map) = self.0.lock().unwrap().maps.get(&rec.plugin)
        {
            rec.message = map.rewrite(&rec.message);
        }
        match rec.level {
            model::LogLevel::Error => tracing::error!(plugin = %rec.plugin, "{}", rec.message),
            model::LogLevel::Warn => tracing::warn!(plugin = %rec.plugin, "{}", rec.message),
            model::LogLevel::Info => tracing::info!(plugin = %rec.plugin, "{}", rec.message),
            model::LogLevel::Debug => tracing::debug!(plugin = %rec.plugin, "{}", rec.message),
            model::LogLevel::Trace => tracing::trace!(plugin = %rec.plugin, "{}", rec.message),
        }
        let mut b = self.0.lock().unwrap();
        if b.records.len() == b.capacity.max(1) {
            b.records.pop_front();
        }
        b.records.push_back(rec);
        b.total += 1;
    }

    /// Every record the buffer holds, oldest first.
    pub fn snapshot(&self) -> Vec<model::LogRecord> {
        self.0.lock().unwrap().records.iter().cloned().collect()
    }

    /// Records with sequence number >= `since`, and the next sequence number.
    pub fn since(&self, since: u64) -> (Vec<model::LogRecord>, u64) {
        let b = self.0.lock().unwrap();
        let first = b.total - b.records.len() as u64;
        let skip = since.saturating_sub(first) as usize;
        (b.records.iter().skip(skip).cloned().collect(), b.total)
    }
}
