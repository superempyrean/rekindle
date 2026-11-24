//! The library's native motion engine, through its `ldh_motion_*` exports.
//!
//! On Android the platform creates one engine with
//! `MotionNative.nativeCreateNativeFirst(4, null)` and feeds it the guest's
//! motion cells and the views' inputs. Each frame it calls `nativeTick` and
//! applies the channel values the engine writes to its views. The engine,
//! the worklets the guest names and their springs are the library's own
//! code, so a platform that calls these exports animates exactly as
//! Android does.

use anyhow::{Result, bail};

use crate::loader::Module;
use crate::sys;

/// The engine's exports.
#[derive(Clone, Copy)]
pub(crate) struct Syms {
    new_native_first: usize,
    free: usize,
    apply: usize,
    view_motion: usize,
    declare: usize,
    layout_in: usize,
    size: usize,
    system: usize,
    set_reduced: usize,
    wants_frame: usize,
    tick: usize,
    drop_view: usize,
    layout_views: usize,
    exit_begin: usize,
    exits_done: usize,
    trips: usize,
}

impl Syms {
    pub(crate) fn find(m: &Module) -> Option<Syms> {
        let s = |n: &str| m.sym(n);
        Some(Syms {
            new_native_first: s("ldh_motion_new_native_first")?,
            free: s("ldh_motion_free")?,
            apply: s("ldh_motion_apply")?,
            view_motion: s("ldh_motion_view_motion")?,
            declare: s("ldh_motion_declare")?,
            layout_in: s("ldh_motion_layout_in")?,
            size: s("ldh_motion_size")?,
            system: s("ldh_motion_system")?,
            set_reduced: s("ldh_motion_set_reduced")?,
            wants_frame: s("ldh_motion_wants_frame")?,
            tick: s("ldh_motion_tick")?,
            drop_view: s("ldh_motion_drop_view")?,
            layout_views: s("ldh_motion_layout_views")?,
            exit_begin: s("ldh_motion_exit_begin")?,
            exits_done: s("ldh_motion_exits_done")?,
            trips: s("ldh_motion_trips")?,
        })
    }
}

/// One engine. Calls into it are made from one thread at a time, the
/// thread that created it.
pub struct Motion {
    h: u64,
    s: Syms,
}

/// What one [`Motion::tick`] returned, split as the Android platform reads
/// it (`overlays.b` case 4 and `mh.u.p`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tick {
    /// The scroll views a running gesture locks.
    pub locks: Vec<u32>,
    /// Views whose exit animation finished, which the platform may now
    /// remove.
    pub exits: Vec<u32>,
    /// List rows whose animation finished.
    pub rows: Vec<u32>,
    /// The engine wants another frame.
    pub wants_frame: bool,
    /// The guard logged trips, which [`Motion::trips`] drains.
    pub has_trips: bool,
    /// Channel values for views: `(view, channel, value bits)`.
    pub writes: Vec<(u32, u32, u32)>,
    /// The word pairs the platform hands the guest as UI event op 8.
    pub events: Vec<(u32, u32)>,
    /// The frame stream's own `wants_frame`.
    pub frame_wants: bool,
}

fn words<'a>(p: u64, n: usize) -> &'a [u32] {
    if p == 0 || n == 0 {
        &[]
    } else {
        // SAFETY: the engine returns `n` words that stay valid until the
        // next output call on this engine, and the caller copies them
        // before making one.
        unsafe { std::slice::from_raw_parts(p as *const u32, n) }
    }
}

fn ptr<T>(p: *const T) -> u64 {
    p as u64
}

impl Motion {
    /// `ldh_motion_new_native_first(4, NULL)`, as Android creates it.
    pub(crate) fn new(s: Syms) -> Result<Motion> {
        let h = sys::guest_call(s.new_native_first, [4, 0, 0, 0]);
        if h == 0 {
            bail!("ldh_motion_new_native_first(4, NULL) returned NULL");
        }
        Ok(Motion { h, s })
    }

    fn call(&self, f: usize, a: [u64; 5], x: [f32; 8]) -> u64 {
        sys::guest_call_f(f, [self.h, a[0], a[1], a[2], a[3], a[4]], x)
    }

    /// `ldh_motion_apply`: an op stream (a turn's Motion op, or a
    /// `worklet` cell as op 3).
    pub fn apply(&self, ops: &[u32]) {
        self.call(
            self.s.apply,
            [ptr(ops.as_ptr()), ops.len() as u64, 0, 0, 0],
            [0.0; 8],
        );
    }

    /// `ldh_motion_view_motion`: a view's `viewMotion` cell, or `None` when
    /// the cell is removed.
    pub fn view_motion(&self, view: u32, spec: Option<&[u32]>) {
        let (p, n) = spec.map_or((0, 0), |w| (ptr(w.as_ptr()), w.len() as u64));
        self.call(self.s.view_motion, [u64::from(view), p, n, 0, 0], [0.0; 8]);
    }

    /// `ldh_motion_declare`: a channel's resting value, NaN for none.
    pub fn declare(&self, view: u32, channel: u32, value: f32) {
        let mut x = [0.0; 8];
        x[0] = value;
        self.call(
            self.s.declare,
            [u64::from(view), u64::from(channel), 0, 0, 0],
            x,
        );
    }

    /// `ldh_motion_layout_in`: the view's rect against the window root,
    /// then its left and top in its parent and the parent's size, in dp.
    pub fn layout(&self, view: u32, rect: [f32; 8]) {
        self.call(self.s.layout_in, [u64::from(view), 0, 0, 0, 0], rect);
    }

    /// `ldh_motion_size`, in dp.
    pub fn size(&self, view: u32, w: f32, h: f32) {
        let mut x = [0.0; 8];
        x[0] = w;
        x[1] = h;
        self.call(self.s.size, [u64::from(view), 0, 0, 0, 0], x);
    }

    /// `ldh_motion_system(kind, key, value, ms)`: an input.
    pub fn system(&self, kind: u32, key: u32, value: f32, ms: f32) {
        let mut x = [0.0; 8];
        x[0] = value;
        x[1] = ms;
        self.call(
            self.s.system,
            [u64::from(kind), u64::from(key), 0, 0, 0],
            x,
        );
    }

    pub fn set_reduced(&self, on: bool) {
        self.call(self.s.set_reduced, [u64::from(on), 0, 0, 0, 0], [0.0; 8]);
    }

    pub fn wants_frame(&self) -> bool {
        self.call(self.s.wants_frame, [0; 5], [0.0; 8]) as u8 != 0
    }

    pub fn drop_view(&self, view: u32) {
        self.call(self.s.drop_view, [u64::from(view), 0, 0, 0, 0], [0.0; 8]);
    }

    /// `ldh_motion_exit_begin`: true if the view's exit animates, so the
    /// platform keeps it until [`Tick::exits`] lists it.
    pub fn exit_begin(&self, view: u32, inner: &[u32]) -> bool {
        self.call(
            self.s.exit_begin,
            [u64::from(view), ptr(inner.as_ptr()), inner.len() as u64, 0, 0],
            [0.0; 8],
        ) as u8
            != 0
    }

    /// `ldh_motion_exits_done`.
    pub fn exits_done(&self) -> Vec<u32> {
        let mut n = 0usize;
        let p = self.call(self.s.exits_done, [ptr(&raw mut n), 0, 0, 0, 0], [0.0; 8]);
        words(p, n).to_vec()
    }

    /// `ldh_motion_layout_views`: the views that want their layout
    /// reported, and a generation number.
    pub fn layout_views(&self) -> (Vec<u32>, u32) {
        let mut n = 0usize;
        let mut extra = 0u32;
        let p = self.call(
            self.s.layout_views,
            [ptr(&raw mut n), ptr(&raw mut extra), 0, 0, 0],
            [0.0; 8],
        );
        (words(p, n).to_vec(), extra)
    }

    /// `ldh_motion_trips`: the guard's lines since the last call.
    pub fn trips(&self) -> String {
        let mut n = 0usize;
        let p = self.call(self.s.trips, [ptr(&raw mut n), 0, 0, 0, 0], [0.0; 8]);
        if p == 0 || n == 0 {
            return String::new();
        }
        // SAFETY: `n` bytes of UTF-8 the engine keeps until the next call.
        let b = unsafe { std::slice::from_raw_parts(p as *const u8, n) };
        String::from_utf8_lossy(b).into_owned()
    }

    /// `ldh_motion_tick(dt)`: one frame, `dt` in milliseconds.
    pub fn tick(&self, dt: f32) -> Tick {
        let mut n = 0usize;
        let mut x = [0.0; 8];
        x[0] = dt;
        let p = self.call(self.s.tick, [ptr(&raw mut n), 0, 0, 0, 0], x);
        Tick::read(words(p, n)).unwrap_or_default()
    }
}

impl Drop for Motion {
    fn drop(&mut self) {
        sys::guest_call(self.s.free, [self.h, 0, 0, 0]);
    }
}

struct Reader<'a> {
    w: &'a [u32],
    pos: usize,
}

impl Reader<'_> {
    fn word(&mut self) -> Option<u32> {
        let w = *self.w.get(self.pos)?;
        self.pos += 1;
        Some(w)
    }

    fn list(&mut self) -> Option<Vec<u32>> {
        let n = self.word()? as usize;
        let s = self.w.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s.to_vec())
    }
}

impl Tick {
    /// `[k, nLocks, locks, nExits, exits, nRows, rows, wants_frame,
    /// has_trips, frame...]`, with the frame at `k + 1`. The frame starts
    /// with the writes, three words each, then the event pairs and the
    /// frame's `wants_frame`. The rest of it (portal placements, haptics,
    /// offloaded keyframes) isn't read here.
    pub fn read(w: &[u32]) -> Option<Tick> {
        let mut r = Reader { w, pos: 0 };
        let k = r.word()? as usize;
        let mut t = Tick {
            locks: r.list()?,
            exits: r.list()?,
            rows: r.list()?,
            wants_frame: r.word()? != 0,
            has_trips: r.word()? != 0,
            ..Tick::default()
        };
        r.pos = k + 1;
        for _ in 0..r.word()? {
            t.writes.push((r.word()?, r.word()?, r.word()?));
        }
        for _ in 0..r.word()? {
            t.events.push((r.word()?, r.word()?));
        }
        t.frame_wants = r.word()? != 0;
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_reads_as_android_splits_it() {
        // k = 7: one lock, no exits, one row, wants, no trips; then the
        // frame: one write, one event pair, wants.
        let w = [
            7, 1, 77, 0, 1, 5, 1, 0, //
            1, 12, 7, 0x3f80_0000, 1, 0x0010_0001, 5, 1, 0, 0,
        ];
        let t = Tick::read(&w).unwrap();
        assert_eq!(t.locks, vec![77]);
        assert!(t.exits.is_empty());
        assert_eq!(t.rows, vec![5]);
        assert!(t.wants_frame && !t.has_trips);
        assert_eq!(t.writes, vec![(12, 7, 0x3f80_0000)]);
        assert_eq!(t.events, vec![(0x0010_0001, 5)]);
        assert!(t.frame_wants);
    }

    /// The engine of the real library runs `press_look` as the guest's
    /// buttons program it: value 9 follows the pressed input of the view,
    /// and a press moves `Background` to the pressed colour and `Scale`
    /// below 1. `PHOENIX_TEST_SO` points at the `x86_64` host library.
    #[test]
    #[ignore = "needs PHOENIX_TEST_SO=<path to liblibdiscord_wasm_host.so>"]
    fn press_look_on_the_real_engine() {
        use rekindle_wire::motion::worklet::{Look, PressLook, Transition};
        let Some(so) = std::env::var_os("PHOENIX_TEST_SO") else {
            return;
        };
        let mut o = crate::Options::new(so);
        o.trace = 0;
        let lib = crate::load(&o).unwrap();
        let m = lib.motion().unwrap();
        let spring = Transition {
            kind: 1,
            mass: 1.0,
            stiffness: 300.0,
            damping: 27.0,
            clamp: 1,
            rest_displacement: 0.0,
            rest_speed: 0.0,
            ms: 0.0,
            easing: 0,
            bezier: [0.0; 4],
        };
        let set = Transition { kind: 2, ..spring };
        let look = PressLook {
            press: 9,
            scale: 10,
            look: 11,
            portal_slot: 0xffff_ffff,
            shrink: 8.0,
            pressed_scale: 0.96,
            scale_transition: spring,
            looks: vec![Look {
                channel: 8,
                rest: 0xffff_ffff,
                pressed: 0xffc2_c2c7,
                only_pressed: 0,
            }],
            opacity: [f32::NAN, f32::NAN],
            press_transition: set,
            release_transition: Transition { kind: 0, ..spring },
            tail: vec![],
        };
        let w = look.to_worklet();
        let mut op = vec![3, 5, 0, w.id, w.args.len() as u32];
        op.extend(&w.args);
        m.apply(&op);
        m.view_motion(5, Some(&[21, 9, 5, 0xffff_ffff, 0]));
        m.size(5, 100.0, 40.0);
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.extend(m.tick(16.0).writes);
        }
        m.system(5, 5, 1.0, 0.0);
        for _ in 0..30 {
            seen.extend(m.tick(16.0).writes);
        }
        eprintln!("{seen:x?}");
        assert!(seen.contains(&(5, 8, 0xffc2_c2c7)), "{seen:x?}");
        let scale = seen
            .iter()
            .filter(|w| w.0 == 5 && w.1 == 3)
            .map(|w| f32::from_bits(w.2))
            .fold(1.0f32, f32::min);
        assert!(scale < 0.99, "{scale}");
    }
}
