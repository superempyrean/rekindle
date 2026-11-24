//! The loader's log: one file, written unbuffered, so a fault loses nothing.

use std::fmt::Arguments;
use std::fs::File;
use std::io::Write as _;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard};

static LOG: Mutex<Option<File>> = Mutex::new(None);
static TRACE: AtomicI32 = AtomicI32::new(1);

pub(crate) fn open(file: Option<File>) {
    *lock() = file;
}

fn lock() -> MutexGuard<'static, Option<File>> {
    LOG.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Writes formatted text as it is; the caller supplies the newline.
pub(crate) fn write(args: Arguments<'_>) {
    if let Some(f) = lock().as_mut() {
        let _ = f.write_fmt(args);
    }
}

/// Writes formatted text if the log isn't busy. For fault handlers, which
/// may run while this thread holds the lock.
pub(crate) fn try_write(args: Arguments<'_>) {
    if let Ok(mut g) = LOG.try_lock()
        && let Some(f) = g.as_mut()
    {
        let _ = f.write_fmt(args);
    }
}

pub(crate) fn trace() -> i32 {
    TRACE.load(Ordering::Relaxed)
}

pub(crate) fn set_trace(level: i32) {
    TRACE.store(level, Ordering::Relaxed);
}

macro_rules! glog {
    ($($t:tt)*) => { $crate::log::write(format_args!($($t)*)) };
}
pub(crate) use glog;
