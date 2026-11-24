//! pthreads, a futex, semaphores and the scheduler.
//!
//! bionic's `pthread_mutex_t` is 40 bytes, `pthread_cond_t` 48 and
//! `pthread_rwlock_t` 56, all zero-initialized. A Windows SRW lock or
//! condition variable sits inside each at the first 8-aligned address past
//! bionic's state word. `pthread_once` and semaphores run on the futex,
//! which `syscall(SYS_futex)` also reaches.
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "every shim dereferences the pointers the library passes, under the contract of the bionic function it stands in for"
)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::{
    AcquireSRWLockExclusive, AcquireSRWLockShared, CONDITION_VARIABLE, GetActiveProcessorCount,
    GetCurrentThreadId, INFINITE, ReleaseSRWLockExclusive, ReleaseSRWLockShared, SRWLOCK,
    SleepConditionVariableSRW, SwitchToThread, WakeAllConditionVariable,
};

use super::{NKEYS, Stack, run_on_stack, set_errno, ts, ts_free};
use crate::errno::{EAGAIN, EDEADLK, EINVAL, ENOSYS, EPERM, ETIMEDOUT};
use crate::loader::where_;
use crate::log::glog;

// ---------- futex ----------

struct Waiter {
    addr: usize,
    woken: AtomicBool,
    cv: Condvar,
}

type Bucket = Mutex<Vec<Arc<Waiter>>>;
static BUCKETS: [Bucket; 64] = [const { Mutex::new(Vec::new()) }; 64];

fn bucket(a: usize) -> MutexGuard<'static, Vec<Arc<Waiter>>> {
    let i = (((a >> 2) as u32).wrapping_mul(2_654_435_761) >> 26) & 63;
    BUCKETS[i as usize]
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Waits while `*a == val`. `timeout` is relative, `None` for none.
/// Returns 0 or a negative Linux errno.
pub(super) fn futex_wait(a: *const AtomicU32, val: u32, timeout: Option<Duration>) -> i64 {
    let mut g = bucket(a as usize);
    if unsafe { (*a).load(Ordering::SeqCst) } != val {
        return -i64::from(EAGAIN);
    }
    let w = Arc::new(Waiter {
        addr: a as usize,
        woken: AtomicBool::new(false),
        cv: Condvar::new(),
    });
    g.push(w.clone());
    let start = Instant::now();
    loop {
        if w.woken.load(Ordering::Relaxed) {
            return 0;
        }
        match timeout {
            None => {
                g =
                    w.cv.wait(g)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            Some(t) => {
                let Some(left) = t.checked_sub(start.elapsed()).filter(|d| !d.is_zero()) else {
                    g.retain(|x| !Arc::ptr_eq(x, &w));
                    return -i64::from(ETIMEDOUT);
                };
                g =
                    w.cv.wait_timeout(g, left)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
            }
        }
    }
}

/// Wakes up to `n` waiters on `a`. Returns how many it woke.
pub(super) fn futex_wake(a: *const AtomicU32, n: i64) -> i64 {
    let mut g = bucket(a as usize);
    let mut woke = 0;
    g.retain(|w| {
        if woke < n && w.addr == a as usize {
            w.woken.store(true, Ordering::Relaxed);
            w.cv.notify_one();
            woke += 1;
            false
        } else {
            true
        }
    });
    woke
}

#[repr(C)]
pub(super) struct Timespec {
    pub(super) sec: i64,
    pub(super) nsec: i64,
}

/// `syscall(SYS_futex, ...)`.
pub(super) fn sys_futex(a: *const AtomicU32, op: i32, val: u32, ts: *const Timespec) -> i64 {
    let cmd = op & 0x7f;
    match cmd {
        1 | 10 => futex_wake(a, i64::from(val)),
        0 | 9 => {
            let timeout = (!ts.is_null()).then(|| {
                let t = unsafe { &*ts };
                let mut ns = t.sec * 1_000_000_000 + t.nsec;
                if cmd == 9 {
                    // Absolute, on CLOCK_REALTIME with flag 256.
                    let now = if op & 256 != 0 {
                        super::process::now_real_ns()
                    } else {
                        super::process::now_mono_ns()
                    };
                    ns = (ns - now).max(0);
                }
                Duration::from_nanos(ns.max(0) as u64)
            });
            futex_wait(a, val, timeout)
        }
        _ => {
            glog!("[futex] op {op:#x} unsupported\n");
            -i64::from(ENOSYS)
        }
    }
}

// ---------- mutexes, conditions, rwlocks ----------

fn align8(p: usize) -> usize {
    (p + 7) & !7
}

struct Mx {
    srw: *mut SRWLOCK,
    owner: *mut u32,
    count: *mut u32,
    kind: i32,
}

fn mx(m: *mut c_void) -> Mx {
    let a = m as usize;
    Mx {
        srw: align8(a + 4) as *mut SRWLOCK,
        owner: (a + 24) as *mut u32,
        count: (a + 28) as *mut u32,
        kind: (unsafe { *(a as *const i32) } >> 14) & 3,
    }
}

fn me() -> u32 {
    unsafe { GetCurrentThreadId() }
}

extern "sysv64" fn s_pthread_mutex_lock(m: *mut c_void) -> i32 {
    let x = mx(m);
    unsafe {
        if x.kind != 0 && x.owner.read_volatile() == me() {
            if x.kind == 2 {
                return EDEADLK;
            }
            *x.count += 1;
            return 0;
        }
        AcquireSRWLockExclusive(x.srw);
        x.owner.write_volatile(me());
        *x.count = 1;
    }
    0
}

extern "sysv64" fn s_pthread_mutex_unlock(m: *mut c_void) -> i32 {
    let x = mx(m);
    unsafe {
        if x.kind == 1 {
            *x.count -= 1;
            if *x.count != 0 {
                return 0;
            }
        }
        x.owner.write_volatile(0);
        ReleaseSRWLockExclusive(x.srw);
    }
    0
}

extern "sysv64" fn s_pthread_mutex_destroy(_m: *mut c_void) -> i32 {
    0
}

extern "sysv64" fn s_pthread_cond_broadcast(c: *mut c_void) -> i32 {
    unsafe { WakeAllConditionVariable(align8(c as usize) as *mut CONDITION_VARIABLE) };
    0
}

extern "sysv64" fn s_pthread_cond_wait(c: *mut c_void, m: *mut c_void) -> i32 {
    let x = mx(m);
    unsafe {
        let cnt = *x.count;
        x.owner.write_volatile(0);
        SleepConditionVariableSRW(
            align8(c as usize) as *mut CONDITION_VARIABLE,
            x.srw,
            INFINITE,
            0,
        );
        x.owner.write_volatile(me());
        *x.count = cnt;
    }
    0
}

extern "sysv64" fn s_pthread_rwlock_init(l: *mut u8, _a: *const c_void) -> i32 {
    unsafe { l.write_bytes(0, 56) };
    0
}

extern "sysv64" fn s_pthread_rwlock_destroy(_l: *mut c_void) -> i32 {
    0
}

fn rw(l: *mut c_void) -> (*mut SRWLOCK, *mut u32) {
    (
        align8(l as usize) as *mut SRWLOCK,
        (l as usize + 16) as *mut u32,
    )
}

extern "sysv64" fn s_pthread_rwlock_rdlock(l: *mut c_void) -> i32 {
    unsafe { AcquireSRWLockShared(rw(l).0) };
    0
}

extern "sysv64" fn s_pthread_rwlock_wrlock(l: *mut c_void) -> i32 {
    let (s, w) = rw(l);
    unsafe {
        AcquireSRWLockExclusive(s);
        w.write_volatile(1);
    }
    0
}

extern "sysv64" fn s_pthread_rwlock_unlock(l: *mut c_void) -> i32 {
    let (s, w) = rw(l);
    unsafe {
        if w.read_volatile() != 0 {
            w.write_volatile(0);
            ReleaseSRWLockExclusive(s);
        } else {
            ReleaseSRWLockShared(s);
        }
    }
    0
}

extern "sysv64" fn s_pthread_once(o: *const AtomicU32, f: extern "sysv64" fn()) -> i32 {
    let once = unsafe { &*o };
    loop {
        match once.load(Ordering::Acquire) {
            2 => return 0,
            0 if once
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok() =>
            {
                f();
                once.store(2, Ordering::Release);
                futex_wake(o, i64::MAX);
                return 0;
            }
            _ => {
                futex_wait(o, 1, None);
            }
        }
    }
}

// ---------- keys and thread_local destructors ----------

static KEYS: Mutex<[(bool, usize); NKEYS]> = Mutex::new([(false, 0); NKEYS]);

fn keys() -> MutexGuard<'static, [(bool, usize); NKEYS]> {
    KEYS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

extern "sysv64" fn s_pthread_key_create(key: *mut i32, dtor: usize) -> i32 {
    let mut k = keys();
    let Some(i) = k.iter().position(|e| !e.0) else {
        return EAGAIN;
    };
    k[i] = (true, dtor);
    unsafe { *key = i as i32 };
    0
}

extern "sysv64" fn s_pthread_key_delete(k: i32) -> i32 {
    let Some(i) = usize::try_from(k).ok().filter(|&i| i < NKEYS) else {
        return EINVAL;
    };
    keys()[i] = (false, 0);
    0
}

extern "sysv64" fn s_pthread_getspecific(k: i32) -> usize {
    match usize::try_from(k).ok().filter(|&i| i < NKEYS) {
        Some(i) => unsafe { (*ts()).keys[i] },
        None => 0,
    }
}

extern "sysv64" fn s_pthread_setspecific(k: i32, v: usize) -> i32 {
    let Some(i) = usize::try_from(k).ok().filter(|&i| i < NKEYS) else {
        return EINVAL;
    };
    unsafe { (*ts()).keys[i] = v };
    0
}

extern "sysv64" fn s_cxa_thread_atexit_impl(f: usize, obj: usize, _dso: usize) -> i32 {
    unsafe { (*ts()).dtors.push((f, obj)) };
    0
}

/// `thread_local` destructors, then pthread key destructors, as glibc
/// orders them. Runs on the thread's guest stack.
fn thread_exit() {
    let t = ts();
    while let Some((f, obj)) = unsafe { (*t).dtors.pop() } {
        let f: extern "sysv64" fn(usize) = unsafe { std::mem::transmute(f) };
        f(obj);
    }
    for _ in 0..4 {
        let mut any = false;
        for k in 0..NKEYS {
            let v = unsafe { (*t).keys[k] };
            let (used, dtor) = keys()[k];
            if v != 0 && used && dtor != 0 {
                unsafe { (*t).keys[k] = 0 };
                let f: extern "sysv64" fn(usize) = unsafe { std::mem::transmute(dtor) };
                f(v);
                any = true;
            }
        }
        if !any {
            break;
        }
    }
}

// ---------- threads ----------

/// bionic's `pthread_attr_t`.
#[repr(C)]
struct Attr {
    flags: u32,
    stack_base: usize,
    stack_size: usize,
    guard_size: usize,
    policy: i32,
    prio: i32,
    rsv: [u8; 16],
}

extern "sysv64" fn s_pthread_attr_init(a: *mut Attr) -> i32 {
    unsafe {
        a.write(Attr {
            flags: 0,
            stack_base: 0,
            stack_size: 1 << 20,
            guard_size: 4096,
            policy: 0,
            prio: 0,
            rsv: [0; 16],
        });
    }
    0
}

extern "sysv64" fn s_pthread_attr_destroy(_a: *mut Attr) -> i32 {
    0
}

extern "sysv64" fn s_pthread_attr_setstacksize(a: *mut Attr, n: usize) -> i32 {
    if n < 16384 {
        return EINVAL;
    }
    unsafe { (*a).stack_size = n };
    0
}

/// A thread the library started. `pthread_t` points at it.
struct Thr {
    f: extern "sysv64" fn(usize) -> usize,
    arg: usize,
    ret: usize,
    stack: usize,
    handle: Option<JoinHandle<()>>,
    /// The thread and its creator each hold one; the last frees it.
    refs: AtomicI32,
}

fn release(t: *mut Thr) {
    if unsafe { (*t).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        drop(unsafe { Box::from_raw(t) });
    }
}

extern "sysv64" fn thr_body(t: usize) {
    let t = t as *mut Thr;
    unsafe { (*t).ret = ((*t).f)((*t).arg) };
    thread_exit();
}

fn thr_entry(t: *mut Thr) {
    let s = Stack::new(unsafe { (*t).stack });
    let (lo, hi) = (s.lo, s.hi);
    let st = ts();
    unsafe {
        (*st).stack = Some(s);
        (*st).on_guest_stack = true;
    }
    run_on_stack(thr_body, t as usize, hi, lo);
    ts_free();
    release(t);
}

struct SendPtr(*mut Thr);
// SAFETY: the thread owns one reference to the record.
unsafe impl Send for SendPtr {}

extern "sysv64" fn s_pthread_create(
    out: *mut *mut Thr,
    a: *const Attr,
    f: extern "sysv64" fn(usize) -> usize,
    arg: usize,
) -> i32 {
    let size = if a.is_null() {
        1 << 20
    } else {
        unsafe { (*a).stack_size }
    };
    let t = Box::into_raw(Box::new(Thr {
        f,
        arg,
        ret: 0,
        stack: size + 0x10000,
        handle: None,
        refs: AtomicI32::new(2),
    }));
    let p = SendPtr(t);
    let r = std::thread::Builder::new()
        .stack_size(0x10000)
        .spawn(move || {
            let p = p;
            thr_entry(p.0);
        });
    let code = if r.is_ok() { 0 } else { EAGAIN };
    glog!(
        "[thread] create start={} arg={arg:#x} stack={:#x} -> {code}\n",
        where_(f as usize),
        size + 0x10000
    );
    let Ok(h) = r else {
        drop(unsafe { Box::from_raw(t) });
        return EAGAIN;
    };
    unsafe {
        (*t).handle = Some(h);
        *out = t;
    }
    0
}

extern "sysv64" fn s_pthread_detach(t: *mut Thr) -> i32 {
    drop(unsafe { (*t).handle.take() });
    release(t);
    0
}

extern "sysv64" fn s_pthread_join(t: *mut Thr, ret: *mut usize) -> i32 {
    let Some(h) = (unsafe { (*t).handle.take() }) else {
        return EINVAL;
    };
    let _ = h.join();
    if !ret.is_null() {
        unsafe { *ret = (*t).ret };
    }
    release(t);
    0
}

// ---------- semaphores ----------

/// bionic's `sem_t`: the count in the first word.
extern "sysv64" fn s_sem_init(s: *mut u32, _pshared: i32, v: u32) -> i32 {
    unsafe { *s = v };
    0
}

extern "sysv64" fn s_sem_destroy(_s: *mut u32) -> i32 {
    0
}

extern "sysv64" fn s_sem_post(s: *const AtomicU32) -> i32 {
    unsafe { (*s).fetch_add(1, Ordering::Release) };
    futex_wake(s, 1);
    0
}

extern "sysv64" fn s_sem_wait(s: *const AtomicU32) -> i32 {
    let sem = unsafe { &*s };
    loop {
        let c = sem.load(Ordering::Acquire);
        if c > 0 {
            if sem
                .compare_exchange(c, c - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return 0;
            }
        } else {
            futex_wait(s, 0, None);
        }
    }
}

// ---------- the scheduler ----------

extern "sysv64" fn s_sched_yield() -> i32 {
    unsafe { SwitchToThread() };
    0
}

pub(super) fn cpus() -> u32 {
    unsafe { GetActiveProcessorCount(0xffff) }
}

extern "sysv64" fn s_sched_getaffinity(_pid: i32, n: usize, mask: *mut u8) -> i32 {
    unsafe {
        mask.write_bytes(0, n);
        for i in 0..cpus() as usize {
            if i / 8 < n {
                *mask.add(i / 8) |= 1 << (i % 8);
            }
        }
    }
    0
}

extern "sysv64" fn s_sched_getscheduler(_pid: i32) -> i32 {
    0
}

extern "sysv64" fn s_sched_getparam(_pid: i32, p: *mut i32) -> i32 {
    unsafe { *p = 0 };
    0
}

extern "sysv64" fn s_sched_setscheduler(_pid: i32, _pol: i32, _p: *const i32) -> i32 {
    set_errno(EPERM);
    -1
}

extern "sysv64" fn s_getpriority(_w: i32, _who: i32) -> i32 {
    0
}

extern "sysv64" fn s_setpriority(_w: i32, _who: i32, _p: i32) -> i32 {
    0
}

macro_rules! s {
    ($($name:ident),* $(,)?) => {
        vec![$((&stringify!($name)[2..], $name as *const () as usize, false)),*]
    };
}

pub(super) fn table() -> Vec<(&'static str, usize, bool)> {
    let mut t = s![
        s_pthread_mutex_lock,
        s_pthread_mutex_unlock,
        s_pthread_mutex_destroy,
        s_pthread_cond_broadcast,
        s_pthread_cond_wait,
        s_pthread_rwlock_init,
        s_pthread_rwlock_destroy,
        s_pthread_rwlock_rdlock,
        s_pthread_rwlock_wrlock,
        s_pthread_rwlock_unlock,
        s_pthread_once,
        s_pthread_key_create,
        s_pthread_key_delete,
        s_pthread_getspecific,
        s_pthread_setspecific,
        s_pthread_attr_init,
        s_pthread_attr_destroy,
        s_pthread_attr_setstacksize,
        s_pthread_create,
        s_pthread_detach,
        s_pthread_join,
        s_sem_init,
        s_sem_destroy,
        s_sem_post,
        s_sem_wait,
        s_sched_yield,
        s_sched_getaffinity,
        s_sched_getscheduler,
        s_sched_getparam,
        s_sched_setscheduler,
        s_getpriority,
        s_setpriority,
    ];
    t.push((
        "__cxa_thread_atexit_impl",
        s_cxa_thread_atexit_impl as *const () as usize,
        false,
    ));
    t
}
