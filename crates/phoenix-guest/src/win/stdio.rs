//! Files, directories and stdio.
//!
//! A POSIX path `/x/y` maps to `<root>\x\y`. With no root every path is
//! missing, and `..`, `\` and `:` are refused. Descriptors 0 to 2 are the
//! process's standard streams.
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "every shim dereferences the pointers the library passes, under the contract of the bionic function it stands in for"
)]

use std::ffi::{c_char, c_void};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::libc::{cbytes, cstr_lossy, guest_strdup, vformat};
use super::{errno, set_errno};
use crate::errno::{
    EACCES, EBADF, EEXIST, EINVAL, EIO, EISDIR, EMFILE, ENAMETOOLONG, ENOENT, ENOMEM, ENOSYS,
    ENOTDIR, ENOTEMPTY, ENOTTY, ERANGE,
};
use crate::loader::caller;
use crate::log::glog;
use crate::valist::{VaList, variadic};

// ---------- paths ----------

fn win_errno(e: &std::io::Error) -> i32 {
    match e.raw_os_error().unwrap_or(0) {
        2 | 3 | 123 | 161 | 15 => ENOENT,
        5 | 32 => EACCES,
        80 | 183 => EEXIST,
        145 => ENOTEMPTY,
        267 => ENOTDIR,
        8 | 14 => ENOMEM,
        6 => EBADF,
        206 => ENAMETOOLONG,
        _ => match e.kind() {
            std::io::ErrorKind::NotFound => ENOENT,
            std::io::ErrorKind::PermissionDenied => EACCES,
            std::io::ErrorKind::AlreadyExists => EEXIST,
            _ => EIO,
        },
    }
}

/// A POSIX path to a path under the root, or a Linux errno.
fn map_path(p: &[u8]) -> Result<PathBuf, i32> {
    let Some(root) = super::root() else {
        return Err(ENOENT);
    };
    if p.is_empty() {
        return Err(ENOENT);
    }
    if p.contains(&b'\\') || p.contains(&b':') {
        return Err(EACCES);
    }
    if p.split(|&c| c == b'/').any(|c| c == b"..") {
        return Err(EACCES);
    }
    let rel = String::from_utf8_lossy(p);
    let mut out = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        out.push(part);
    }
    if out.as_os_str().len() > 1024 {
        return Err(ENAMETOOLONG);
    }
    Ok(out)
}

/// bionic's `struct stat`.
#[repr(C)]
struct Stat {
    dev: u64,
    ino: u64,
    nlink: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    pad: u32,
    rdev: u64,
    size: i64,
    blksize: i64,
    blocks: i64,
    times: [i64; 6],
    reserved: [i64; 3],
}
const _: () = assert!(size_of::<Stat>() == 144);

fn ft_unix(ft: u64) -> (i64, i64) {
    let t = ft as i64 - 116_444_736_000_000_000;
    (t.div_euclid(10_000_000), t.rem_euclid(10_000_000) * 100)
}

fn fill_stat(st: *mut Stat, m: &Metadata, ino: u64) {
    let dir = m.is_dir();
    let size = if dir { 4096 } else { m.len() as i64 };
    let (a, an) = ft_unix(m.last_access_time());
    let (w, wn) = ft_unix(m.last_write_time());
    let (c, cn) = ft_unix(m.creation_time());
    unsafe {
        st.write(Stat {
            dev: 0x801,
            ino,
            nlink: 1,
            mode: if dir { 0o040755 } else { 0o100644 },
            uid: 1000,
            gid: 1000,
            pad: 0,
            rdev: 0,
            size,
            blksize: 4096,
            blocks: (size + 511) / 512,
            times: [a, an, w, wn, c, cn],
            reserved: [0; 3],
        });
    }
}

fn path_ino(p: &std::path::Path) -> u64 {
    p.to_string_lossy()
        .bytes()
        .fold(0u64, |h, c| h.wrapping_mul(131).wrapping_add(u64::from(c)))
}

// ---------- descriptors ----------

struct Fd {
    file: Option<File>,
    flags: i32,
    /// The POSIX path it was opened by.
    path: Vec<u8>,
}

const NFD: usize = 256;

fn fds() -> MutexGuard<'static, Vec<Option<Fd>>> {
    static FDS: OnceLock<Mutex<Vec<Option<Fd>>>> = OnceLock::new();
    FDS.get_or_init(|| Mutex::new((0..NFD).map(|_| None).collect()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fd_new(fd: Fd) -> i32 {
    let mut t = fds();
    match (3..NFD).find(|&i| t[i].is_none()) {
        Some(i) => {
            t[i] = Some(fd);
            i as i32
        }
        None => -1,
    }
}

/// Runs `f` on an open descriptor, or fails with EBADF.
fn with_fd<R>(fd: i32, f: impl FnOnce(&mut Fd) -> Result<R, i32>) -> Result<R, i32> {
    let mut t = fds();
    match usize::try_from(fd)
        .ok()
        .filter(|&i| i >= 3)
        .and_then(|i| t.get_mut(i))
        .and_then(Option::as_mut)
    {
        Some(e) => f(e),
        None => Err(EBADF),
    }
}

fn ret<T: From<i8>>(r: Result<T, i32>) -> T {
    r.unwrap_or_else(|e| {
        set_errno(e);
        T::from(-1)
    })
}

const O_ACCMODE: i32 = 3;
const O_CREAT: i32 = 0o100;
const O_EXCL: i32 = 0o200;
const O_TRUNC: i32 = 0o1000;
const O_APPEND: i32 = 0o2000;
const O_DIRECTORY: i32 = 0o200000;
const AT_FDCWD: i32 = -100;
const AT_REMOVEDIR: i32 = 0x200;
const AT_EMPTY_PATH: i32 = 0x1000;

/// A path relative to a directory descriptor, made absolute.
fn at_path(dfd: i32, p: *const c_char) -> Result<Vec<u8>, i32> {
    let p = cbytes(p);
    if p.first() == Some(&b'/') || dfd == AT_FDCWD {
        return Ok(p.to_vec());
    }
    with_fd(dfd, |d| {
        let mut v = d.path.clone();
        v.push(b'/');
        v.extend_from_slice(p);
        Ok(v)
    })
}

fn do_open(p: &[u8], flags: i32) -> Result<i32, i32> {
    let w = map_path(p)?;
    let meta = std::fs::metadata(&w).ok();
    let is_dir = meta.as_ref().is_some_and(Metadata::is_dir);
    if flags & O_DIRECTORY != 0 && !is_dir {
        return Err(if meta.is_none() { ENOENT } else { ENOTDIR });
    }
    let file = if is_dir {
        None
    } else {
        let mut o = OpenOptions::new();
        match flags & O_ACCMODE {
            0 => o.read(true),
            1 => o.write(true),
            _ => o.read(true).write(true),
        };
        if flags & O_CREAT != 0 {
            if flags & O_EXCL != 0 {
                o.create_new(true);
            } else {
                o.create(true);
            }
        }
        if flags & O_TRUNC != 0 {
            o.truncate(true);
        }
        let mut f = o.open(&w).map_err(|e| win_errno(&e))?;
        if flags & O_APPEND != 0 {
            let _ = f.seek(SeekFrom::End(0));
        }
        Some(f)
    };
    match fd_new(Fd {
        file,
        flags,
        path: p.to_vec(),
    }) {
        -1 => Err(EMFILE),
        fd => Ok(fd),
    }
}

extern "sysv64" fn s_open(p: *const c_char, flags: i32, _mode: i32) -> i32 {
    let r = ret(do_open(cbytes(p), flags));
    glog!(
        "[file] open({}, {flags:#x}) -> {r} from {}\n",
        cstr_lossy(p),
        caller()
    );
    r
}

extern "sysv64" fn s_open_2(p: *const c_char, flags: i32) -> i32 {
    s_open(p, flags, 0)
}

extern "sysv64" fn s_openat(dfd: i32, p: *const c_char, flags: i32, _mode: i32) -> i32 {
    let r = ret(at_path(dfd, p).and_then(|q| do_open(&q, flags)));
    glog!(
        "[file] openat({dfd}, {}, {flags:#x}) -> {r} from {}\n",
        cstr_lossy(p),
        caller()
    );
    r
}

extern "sysv64" fn s_read(fd: i32, buf: *mut u8, n: usize) -> i64 {
    if n == 0 {
        return 0;
    }
    let b = unsafe { std::slice::from_raw_parts_mut(buf, n.min(0x7fff_ffff)) };
    ret(with_fd(fd, |e| match e.file.as_mut() {
        None => Err(EISDIR),
        Some(f) => f.read(b).map(|n| n as i64).map_err(|e| win_errno(&e)),
    }))
}

extern "sysv64" fn s_read_chk(fd: i32, buf: *mut u8, n: usize, _bl: usize) -> i64 {
    s_read(fd, buf, n)
}

extern "sysv64" fn s_write(fd: i32, buf: *const u8, n: usize) -> i64 {
    let b = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(buf, n) }
    };
    if fd == 1 || fd == 2 {
        let _ = if fd == 1 {
            std::io::stdout().write_all(b)
        } else {
            std::io::stderr().write_all(b)
        };
        let nl = if b.last() == Some(&b'\n') { "" } else { "\n" };
        glog!("[fd{fd}] {}{nl}", String::from_utf8_lossy(b));
        return n as i64;
    }
    ret(with_fd(fd, |e| match e.file.as_mut() {
        None => Err(EBADF),
        Some(f) => f
            .write(&b[..n.min(0x7fff_ffff)])
            .map(|n| n as i64)
            .map_err(|e| win_errno(&e)),
    }))
}

extern "sysv64" fn s_write_chk(fd: i32, buf: *const u8, n: usize, _bl: usize) -> i64 {
    s_write(fd, buf, n)
}

#[repr(C)]
struct Iovec {
    base: *const u8,
    len: usize,
}

extern "sysv64" fn s_writev(fd: i32, v: *const Iovec, cnt: i32) -> i64 {
    let mut t = 0;
    for i in 0..usize::try_from(cnt).unwrap_or(0) {
        let io = unsafe { &*v.add(i) };
        let r = s_write(fd, io.base, io.len);
        if r < 0 {
            return if t > 0 { t } else { -1 };
        }
        t += r;
    }
    t
}

extern "sysv64" fn s_close(fd: i32) -> i32 {
    if (0..3).contains(&fd) {
        return 0;
    }
    let mut t = fds();
    if usize::try_from(fd)
        .ok()
        .and_then(|i| t.get_mut(i))
        .and_then(Option::take)
        .is_some()
    {
        return 0;
    }
    drop(t);
    set_errno(EBADF);
    -1
}

extern "sysv64" fn s_lseek64(fd: i32, off: i64, wh: i32) -> i64 {
    let pos = match wh {
        0 => SeekFrom::Start(off as u64),
        1 => SeekFrom::Current(off),
        2 => SeekFrom::End(off),
        _ => {
            set_errno(EINVAL);
            return -1;
        }
    };
    ret(with_fd(fd, |e| match e.file.as_mut() {
        None => Err(EINVAL),
        Some(f) => f.seek(pos).map(|p| p as i64).map_err(|_| EINVAL),
    }))
}

fn do_stat(p: &[u8], st: *mut Stat) -> Result<(), i32> {
    let w = map_path(p)?;
    let m = std::fs::metadata(&w).map_err(|e| win_errno(&e))?;
    fill_stat(st, &m, path_ino(&w));
    Ok(())
}

extern "sysv64" fn s_fstat(fd: i32, st: *mut Stat) -> i32 {
    if (0..3).contains(&fd) {
        unsafe {
            st.write_bytes(0, 1);
            (*st).mode = 0o020620;
            (*st).blksize = 1024;
        }
        return 0;
    }
    let r = with_fd(fd, |e| match e.file.as_ref() {
        Some(f) => {
            let m = f.metadata().map_err(|_| EBADF)?;
            let ino = map_path(&e.path).map_or(0, |w| path_ino(&w));
            fill_stat(st, &m, ino);
            Ok(0)
        }
        None => Ok(1),
    });
    match r {
        Ok(0) => 0,
        // A directory: no handle, so stat it by path.
        Ok(_) => {
            let p = with_fd(fd, |e| Ok(e.path.clone())).unwrap_or_default();
            ret(do_stat(&p, st).map(|()| 0))
        }
        Err(e) => {
            set_errno(e);
            -1
        }
    }
}

extern "sysv64" fn s_stat(p: *const c_char, st: *mut Stat) -> i32 {
    let r = ret(do_stat(cbytes(p), st).map(|()| 0));
    glog!("[file] stat({}) -> {r} from {}\n", cstr_lossy(p), caller());
    r
}

extern "sysv64" fn s_lstat(p: *const c_char, st: *mut Stat) -> i32 {
    let r = ret(do_stat(cbytes(p), st).map(|()| 0));
    glog!("[file] lstat({}) -> {r} from {}\n", cstr_lossy(p), caller());
    r
}

extern "sysv64" fn s_fstatat(dfd: i32, p: *const c_char, st: *mut Stat, flags: i32) -> i32 {
    if cbytes(p).is_empty() && flags & AT_EMPTY_PATH != 0 {
        return s_fstat(dfd, st);
    }
    let r = ret(at_path(dfd, p).and_then(|q| do_stat(&q, st)).map(|()| 0));
    glog!(
        "[file] fstatat({dfd}, {}) -> {r} from {}\n",
        cstr_lossy(p),
        caller()
    );
    r
}

extern "sysv64" fn s_mkdir(p: *const c_char, _mode: u32) -> i32 {
    let r = ret(map_path(cbytes(p))
        .and_then(|w| std::fs::create_dir(w).map_err(|e| win_errno(&e)))
        .map(|()| 0));
    glog!("[file] mkdir({}) -> {r} from {}\n", cstr_lossy(p), caller());
    r
}

extern "sysv64" fn s_unlinkat(dfd: i32, p: *const c_char, flags: i32) -> i32 {
    let r = ret(at_path(dfd, p)
        .and_then(|q| map_path(&q))
        .and_then(|w| {
            let r = if flags & AT_REMOVEDIR != 0 {
                std::fs::remove_dir(w)
            } else {
                std::fs::remove_file(w)
            };
            r.map_err(|e| win_errno(&e))
        })
        .map(|()| 0));
    glog!(
        "[file] unlinkat({dfd}, {}, {flags:#x}) -> {r} from {}\n",
        cstr_lossy(p),
        caller()
    );
    r
}

extern "sysv64" fn s_unlink(p: *const c_char) -> i32 {
    s_unlinkat(AT_FDCWD, p, 0)
}

extern "sysv64" fn s_rename(a: *const c_char, b: *const c_char) -> i32 {
    let r = ret(map_path(cbytes(a))
        .and_then(|wa| map_path(cbytes(b)).map(|wb| (wa, wb)))
        .and_then(|(wa, wb)| std::fs::rename(wa, wb).map_err(|e| win_errno(&e)))
        .map(|()| 0));
    glog!(
        "[file] rename({}, {}) -> {r} from {}\n",
        cstr_lossy(a),
        cstr_lossy(b),
        caller()
    );
    r
}

extern "sysv64" fn s_readlink(p: *const c_char, _buf: *mut u8, _n: usize) -> i64 {
    glog!(
        "[file] readlink({}) -> -1 from {}\n",
        cstr_lossy(p),
        caller()
    );
    let exists = map_path(cbytes(p)).is_ok_and(|w| w.exists());
    set_errno(if exists { EINVAL } else { ENOENT });
    -1
}

extern "sysv64" fn s_getcwd(buf: *mut u8, n: usize) -> *mut u8 {
    if buf.is_null() {
        return guest_strdup(b"/").cast();
    }
    if n < 2 {
        set_errno(ERANGE);
        return std::ptr::null_mut();
    }
    unsafe {
        *buf = b'/';
        *buf.add(1) = 0;
    }
    buf
}

extern "sysv64" fn s_realpath(p: *const c_char, out: *mut u8) -> *mut u8 {
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in cbytes(p).split(|&c| c == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut norm = Vec::new();
    for c in &parts {
        norm.push(b'/');
        norm.extend_from_slice(c);
    }
    if norm.is_empty() {
        norm.push(b'/');
    }
    let ok = map_path(&norm).is_ok_and(|w| w.exists());
    let shown = if ok {
        String::from_utf8_lossy(&norm).into_owned()
    } else {
        "(fail)".into()
    };
    glog!(
        "[file] realpath({}) -> {shown} from {}\n",
        cstr_lossy(p),
        caller()
    );
    if !ok {
        set_errno(ENOENT);
        return std::ptr::null_mut();
    }
    if out.is_null() {
        return guest_strdup(&norm).cast();
    }
    unsafe {
        std::ptr::copy_nonoverlapping(norm.as_ptr(), out, norm.len());
        *out.add(norm.len()) = 0;
    }
    out
}

extern "sysv64" fn s_fsync(fd: i32) -> i32 {
    let _ = with_fd(fd, |e| {
        if let Some(f) = e.file.as_ref() {
            let _ = f.sync_all();
        }
        Ok(())
    });
    0
}

extern "sysv64" fn s_flock(_fd: i32, _op: i32) -> i32 {
    0
}

extern "sysv64" fn s_dup(fd: i32) -> i32 {
    let copy = with_fd(fd, |e| {
        let file = match &e.file {
            Some(f) => Some(f.try_clone().map_err(|_| EBADF)?),
            None => None,
        };
        Ok(Fd {
            file,
            flags: e.flags,
            path: e.path.clone(),
        })
    });
    ret(copy.and_then(|f| match fd_new(f) {
        -1 => Err(EMFILE),
        n => Ok(n),
    }))
}

extern "sysv64" fn s_fcntl(fd: i32, cmd: i32, _arg: i64) -> i32 {
    let flags = with_fd(fd, |e| Ok(e.flags));
    if flags.is_err() && !(0..3).contains(&fd) {
        set_errno(EBADF);
        return -1;
    }
    match cmd {
        0 | 1030 => s_dup(fd),
        1 => 1,
        2 | 4 => 0,
        3 => flags.unwrap_or(2),
        _ => {
            glog!("[file] fcntl({fd}, {cmd}) unsupported from {}\n", caller());
            set_errno(EINVAL);
            -1
        }
    }
}

extern "sysv64" fn s_ioctl(fd: i32, req: u64, _arg: usize) -> i32 {
    glog!("[file] ioctl({fd}, {req:#x}) -> ENOTTY from {}\n", caller());
    set_errno(ENOTTY);
    -1
}

extern "sysv64" fn s_sendfile(_o: i32, _i: i32, _off: *mut i64, _n: usize) -> i64 {
    set_errno(ENOSYS);
    -1
}

// ---------- directories ----------

/// bionic's `struct dirent`.
#[repr(C)]
struct Dirent {
    ino: u64,
    off: i64,
    reclen: u16,
    kind: u8,
    name: [u8; 256],
}

struct Dir {
    entries: Vec<(Vec<u8>, bool)>,
    pos: usize,
    fd: i32,
    ent: Dirent,
}

fn open_dir(p: &[u8], fd: i32) -> Result<*mut Dir, i32> {
    let w = map_path(p)?;
    let mut entries = vec![(b".".to_vec(), true), (b"..".to_vec(), true)];
    for e in std::fs::read_dir(&w).map_err(|e| win_errno(&e))?.flatten() {
        let dir = e.file_type().is_ok_and(|t| t.is_dir());
        entries.push((
            e.file_name().to_string_lossy().into_owned().into_bytes(),
            dir,
        ));
    }
    Ok(Box::into_raw(Box::new(Dir {
        entries,
        pos: 0,
        fd,
        ent: Dirent {
            ino: 0,
            off: 0,
            reclen: 0,
            kind: 0,
            name: [0; 256],
        },
    })))
}

extern "sysv64" fn s_opendir(p: *const c_char) -> *mut Dir {
    let d = do_open(cbytes(p), O_DIRECTORY).and_then(|fd| {
        open_dir(cbytes(p), fd).inspect_err(|_| {
            s_close(fd);
        })
    });
    let d = d.unwrap_or_else(|e| {
        set_errno(e);
        std::ptr::null_mut()
    });
    glog!(
        "[file] opendir({}) -> {d:?} from {}\n",
        cstr_lossy(p),
        caller()
    );
    d
}

extern "sysv64" fn s_fdopendir(fd: i32) -> *mut Dir {
    let r = with_fd(fd, |e| {
        if e.file.is_some() {
            Err(ENOTDIR)
        } else {
            Ok(e.path.clone())
        }
    })
    .and_then(|p| open_dir(&p, fd));
    r.unwrap_or_else(|e| {
        set_errno(e);
        std::ptr::null_mut()
    })
}

extern "sysv64" fn s_readdir(d: *mut Dir) -> *mut Dirent {
    let d = unsafe { &mut *d };
    let Some((name, dir)) = d.entries.get(d.pos) else {
        return std::ptr::null_mut();
    };
    d.pos += 1;
    let n = name.len().min(255);
    d.ent = Dirent {
        ino: 1,
        off: 0,
        reclen: size_of::<Dirent>() as u16,
        kind: if *dir { 4 } else { 8 },
        name: [0; 256],
    };
    d.ent.name[..n].copy_from_slice(&name[..n]);
    &raw mut d.ent
}

extern "sysv64" fn s_closedir(d: *mut Dir) -> i32 {
    let d = unsafe { Box::from_raw(d) };
    s_close(d.fd);
    0
}

extern "sysv64" fn s_dirfd(d: *mut Dir) -> i32 {
    unsafe { (*d).fd }
}

// ---------- stdio ----------

/// bionic's `__sF[3]`: `FILE` is 152 bytes on LP64. Pointers into it are
/// the standard streams; any other `FILE *` is a [`Stream`] of ours.
static FAKE_SF: [u8; 3 * 152] = [0; 3 * 152];

/// The `stderr` variable: a pointer to `__sF[2]`.
struct FakeStderr(usize);
static FAKE_STDERR: OnceLock<FakeStderr> = OnceLock::new();

pub(super) fn fake_sf() -> usize {
    (&raw const FAKE_SF) as usize
}

pub(super) fn fake_stderr() -> usize {
    let p = FAKE_STDERR.get_or_init(|| FakeStderr(fake_sf() + 2 * 152));
    (&raw const p.0) as usize
}

/// A `FILE` from `fopen`.
struct Stream {
    file: File,
    back: Vec<u8>,
}

enum F<'a> {
    Std(usize),
    File(&'a mut Stream),
}

fn tf<'a>(f: *mut c_void) -> F<'a> {
    let (a, base) = (f as usize, fake_sf());
    if (base..base + 3 * 152).contains(&a) {
        F::Std((a - base) / 152)
    } else {
        F::File(unsafe { &mut *f.cast::<Stream>() })
    }
}

fn put(f: *mut c_void, b: &[u8]) -> bool {
    match tf(f) {
        F::Std(0) => false,
        F::Std(1) => std::io::stdout().write_all(b).is_ok(),
        F::Std(_) => std::io::stderr().write_all(b).is_ok(),
        F::File(s) => s.file.write_all(b).is_ok(),
    }
}

fn get(f: *mut c_void, b: &mut [u8]) -> usize {
    match tf(f) {
        F::Std(0) => std::io::stdin().read(b).unwrap_or(0),
        F::Std(_) => 0,
        F::File(s) => {
            let mut n = 0;
            while n < b.len() {
                if let Some(c) = s.back.pop() {
                    b[n] = c;
                    n += 1;
                    continue;
                }
                match s.file.read(&mut b[n..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => n += k,
                }
            }
            n
        }
    }
}

extern "sysv64" fn s_fwrite(p: *const u8, s: usize, n: usize, f: *mut c_void) -> usize {
    let len = s * n;
    if len == 0 {
        return 0;
    }
    if put(f, unsafe { std::slice::from_raw_parts(p, len) }) {
        n
    } else {
        0
    }
}

extern "sysv64" fn s_fread(p: *mut u8, s: usize, n: usize, f: *mut c_void) -> usize {
    let len = s * n;
    if len == 0 {
        return 0;
    }
    get(f, unsafe { std::slice::from_raw_parts_mut(p, len) }) / s
}

extern "sysv64" fn s_fputc(c: i32, f: *mut c_void) -> i32 {
    if put(f, &[c as u8]) { c & 0xff } else { -1 }
}

extern "sysv64" fn s_fputwc(c: u32, f: *mut c_void) -> u32 {
    let mut b = Vec::new();
    crate::printf::utf8_push(&mut b, c);
    if put(f, &b) { c } else { u32::MAX }
}

extern "sysv64" fn s_fflush(_f: *mut c_void) -> i32 {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    0
}

extern "sysv64" fn s_getc(f: *mut c_void) -> i32 {
    let mut b = [0u8];
    if get(f, &mut b) == 1 {
        i32::from(b[0])
    } else {
        -1
    }
}

extern "sysv64" fn s_ungetc(c: i32, f: *mut c_void) -> i32 {
    match tf(f) {
        F::File(s) if c != -1 => {
            s.back.push(c as u8);
            c & 0xff
        }
        _ => -1,
    }
}

extern "sysv64" fn s_getwc(f: *mut c_void) -> u32 {
    match s_getc(f) {
        -1 => u32::MAX,
        c => c as u32,
    }
}

extern "sysv64" fn s_ungetwc(c: u32, f: *mut c_void) -> u32 {
    if s_ungetc(c as i32, f) == -1 {
        u32::MAX
    } else {
        c
    }
}

extern "sysv64" fn s_fclose(f: *mut c_void) -> i32 {
    if let F::File(_) = tf(f) {
        drop(unsafe { Box::from_raw(f.cast::<Stream>()) });
    }
    0
}

extern "sysv64" fn s_fseeko(f: *mut c_void, off: i64, wh: i32) -> i32 {
    let F::File(s) = tf(f) else {
        set_errno(EBADF);
        return -1;
    };
    let pos = match wh {
        0 => SeekFrom::Start(off as u64),
        1 => SeekFrom::Current(off - s.back.len() as i64),
        _ => SeekFrom::End(off),
    };
    s.back.clear();
    if s.file.seek(pos).is_ok() { 0 } else { -1 }
}

extern "sysv64" fn s_ftello(f: *mut c_void) -> i64 {
    match tf(f) {
        F::File(s) => s
            .file
            .stream_position()
            .map_or(-1, |p| p as i64 - s.back.len() as i64),
        F::Std(_) => -1,
    }
}

extern "sysv64" fn s_perror(s: *const c_char) {
    let s = cbytes(s);
    let mut line = String::new();
    if !s.is_empty() {
        line.push_str(&String::from_utf8_lossy(s));
        line.push_str(": ");
    }
    line.push_str(&crate::errno::message(errno()));
    line.push('\n');
    let _ = std::io::stderr().write_all(line.as_bytes());
}

extern "sysv64" fn s_fopen(p: *const c_char, mode: *const c_char) -> *mut c_void {
    let m = cbytes(mode);
    let plus = m.contains(&b'+');
    let mut o = OpenOptions::new();
    match m.first() {
        Some(b'r') => o.read(true).write(plus),
        Some(b'w') => o.write(true).read(plus).create(true).truncate(true),
        Some(b'a') => o.append(true).read(plus).create(true),
        _ => {
            set_errno(EINVAL);
            return std::ptr::null_mut();
        }
    };
    if m.contains(&b'x') {
        o.create_new(true);
    }
    let r = map_path(cbytes(p)).and_then(|w| o.open(w).map_err(|e| win_errno(&e)));
    let f = match r {
        Ok(file) => Box::into_raw(Box::new(Stream {
            file,
            back: Vec::new(),
        }))
        .cast(),
        Err(e) => {
            set_errno(e);
            std::ptr::null_mut()
        }
    };
    glog!(
        "[file] fopen({}, {}) -> {f:?} from {}\n",
        cstr_lossy(p),
        cstr_lossy(mode),
        caller()
    );
    f
}

extern "sysv64" fn s_vfprintf(f: *mut c_void, fmt: *const c_char, ap: *mut VaList) -> i32 {
    let b = vformat(fmt, ap);
    if !b.is_empty() {
        put(f, &b);
    }
    b.len() as i32
}
variadic!(s_fprintf, 2, "rdx", s_vfprintf);

extern "sysv64" fn vprintf_imp(fmt: *const c_char, ap: *mut VaList) -> i32 {
    s_vfprintf((fake_sf() + 152) as *mut c_void, fmt, ap)
}
variadic!(s_printf, 1, "rsi", vprintf_imp);

macro_rules! s {
    ($($name:ident),* $(,)?) => {
        vec![$((&stringify!($name)[2..], $name as *const () as usize, false)),*]
    };
}

pub(super) fn table() -> Vec<(&'static str, usize, bool)> {
    let mut t = s![
        s_open,
        s_openat,
        s_read,
        s_write,
        s_writev,
        s_close,
        s_lseek64,
        s_fstat,
        s_stat,
        s_lstat,
        s_fstatat,
        s_mkdir,
        s_unlinkat,
        s_unlink,
        s_rename,
        s_readlink,
        s_getcwd,
        s_realpath,
        s_fsync,
        s_flock,
        s_dup,
        s_fcntl,
        s_ioctl,
        s_sendfile,
        s_opendir,
        s_fdopendir,
        s_readdir,
        s_closedir,
        s_dirfd,
        s_fwrite,
        s_fread,
        s_fputc,
        s_fputwc,
        s_fflush,
        s_getc,
        s_ungetc,
        s_getwc,
        s_ungetwc,
        s_fclose,
        s_fseeko,
        s_ftello,
        s_perror,
        s_fopen,
        s_vfprintf,
        s_fprintf,
        s_printf,
    ];
    t.extend([
        ("__open_2", s_open_2 as *const () as usize, false),
        ("__read_chk", s_read_chk as *const () as usize, false),
        ("__write_chk", s_write_chk as *const () as usize, false),
        ("fseek", s_fseeko as *const () as usize, false),
    ]);
    t
}
