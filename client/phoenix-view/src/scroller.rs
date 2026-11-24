//! Scroll physics as Android's: a fling that decelerates along
//! `SplineOverScroller`'s curve, and the stretch `EdgeEffect` shows when a
//! scroll reaches either end (Android 12 and later).
//!
//! A `RecyclerView` or `ScrollView` on Android flings with
//! `OverScroller.fling` when the finger lifts, at the velocity its
//! `VelocityTracker` measured, and stops a fling that reaches an end with
//! `EdgeEffect.onAbsorb`. A drag past an end pulls the edge
//! (`onPullDistance`), and the stretch springs back on release. The
//! constants here are those classes', from AOSP.
//!
//! Positions are in dp, the content's offset from its start, as
//! [`gpui::ScrollHandle`] keeps it negated. Times are milliseconds on one
//! clock, the view's.

use std::sync::OnceLock;

/// `ViewConfiguration.getScrollFriction()`.
const SCROLL_FRICTION: f64 = 0.015;
/// `SplineOverScroller.INFLEXION`.
const INFLEXION: f64 = 0.35;
const START_TENSION: f64 = 0.5;
const END_TENSION: f64 = 1.0;
const SAMPLES: usize = 100;
/// `ViewConfiguration`: below this a lift doesn't fling, above it a fling
/// is capped, in dp per second.
pub const MIN_FLING: f32 = 50.;
pub const MAX_FLING: f32 = 8000.;
/// Android's `getScaledTouchSlop`, in dp: a press that moves this far
/// becomes a drag.
pub const TOUCH_SLOP: f32 = 8.;
/// One wheel notch, `getScaledVerticalScrollFactor`, in dp.
pub const WHEEL_NOTCH: f32 = 64.;
/// How long the content takes to reach where the wheel has turned to, from
/// the last notch, in ms. Android moves a notch at once. A desktop list
/// eases it in over a few frames, as Chromium does.
const WHEEL_MS: f64 = 120.;
/// `smoothScrollBy`'s duration for a guest's `animated` scroll, in ms, as
/// the Android platform passes it with an `AccelerateDecelerateInterpolator`.
pub const ANIMATED_MS: f64 = 250.;

/// `EdgeEffect`'s spring.
const NATURAL_FREQUENCY: f64 = 24.657;
const DAMPING_RATIO: f64 = 0.98;
const ON_ABSORB_VELOCITY_ADJUSTMENT: f64 = 13.;
/// GPUI scales no box, so the stretch is drawn as the content moved away
/// from the edge: at most this share of the box, approached as the pull
/// grows to the box's length, about as far as Android's stretch shader
/// moves the far end of the content.
const STRETCH_SHOWN: f32 = 0.06;

fn deceleration_rate() -> f64 {
    0.78f64.ln() / 0.9f64.ln()
}

/// `SplineOverScroller.SPLINE_POSITION`: the share of a fling's distance
/// covered at each hundredth of its duration.
fn spline() -> &'static [f64; SAMPLES + 1] {
    static TABLE: OnceLock<[f64; SAMPLES + 1]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let p1 = START_TENSION * INFLEXION;
        let p2 = 1. - END_TENSION * (1. - INFLEXION);
        let mut t = [0.; SAMPLES + 1];
        let mut x_min = 0.;
        for (i, slot) in t.iter_mut().take(SAMPLES).enumerate() {
            let alpha = i as f64 / SAMPLES as f64;
            let mut x_max = 1.;
            let (mut x, mut coef);
            loop {
                x = x_min + (x_max - x_min) / 2.;
                coef = 3. * x * (1. - x);
                let tx = coef * ((1. - x) * p1 + x * p2) + x * x * x;
                if (tx - alpha).abs() < 1e-5 {
                    break;
                }
                if tx > alpha {
                    x_max = x;
                } else {
                    x_min = x;
                }
            }
            *slot = coef * ((1. - x) * START_TENSION + x) + x * x * x;
        }
        t[SAMPLES] = 1.;
        t
    })
}

/// A fling's curve at a density: Android computes it in pixels, with the
/// screen's pixels per inch in `PHYSICAL_COEF`.
#[derive(Clone, Copy)]
struct Curve {
    density: f64,
    /// `SCROLL_FRICTION * PHYSICAL_COEF`.
    fc: f64,
}

impl Curve {
    fn new(density: f32) -> Curve {
        let density = f64::from(density.max(0.5));
        let ppi = 160. * density;
        let physical = 9.80665 * 39.37 * ppi * 0.84;
        Curve {
            density,
            fc: SCROLL_FRICTION * physical,
        }
    }

    /// `getSplineDeceleration` for a speed in dp per second.
    fn l(&self, speed: f64) -> f64 {
        (INFLEXION * speed * self.density / self.fc).ln()
    }

    /// The distance in dp and the duration in ms of a fling at `speed`.
    fn fling(&self, speed: f64) -> (f64, f64) {
        if speed <= 0. {
            return (0., 0.);
        }
        let l = self.l(speed);
        let r = deceleration_rate();
        let duration = 1000. * (l / (r - 1.)).exp();
        let distance = self.fc * (r / (r - 1.) * l).exp() / self.density;
        (distance, duration)
    }

    /// The speed in dp per second whose fling covers `distance` dp.
    #[cfg(test)]
    fn speed_for(&self, distance: f64) -> f64 {
        if distance <= 0. {
            return 0.;
        }
        let r = deceleration_rate();
        let l = (distance * self.density / self.fc).ln() * (r - 1.) / r;
        self.fc * l.exp() / INFLEXION / self.density
    }
}

/// A running fling: where it started, how far it goes (signed) and how
/// long it takes.
#[derive(Clone, Copy, Debug)]
struct Fling {
    from: f64,
    distance: f64,
    duration: f64,
    start: f64,
}

impl Fling {
    /// The position and velocity (dp per second) at `now`, and whether it
    /// is over.
    fn at(&self, now: f64) -> (f64, f64, bool) {
        if self.duration <= 0. {
            return (self.from + self.distance, 0., true);
        }
        let t = ((now - self.start) / self.duration).clamp(0., 1.);
        if t >= 1. {
            return (self.from + self.distance, 0., true);
        }
        let table = spline();
        let i = ((SAMPLES as f64 * t) as usize).min(SAMPLES - 1);
        let (t_inf, t_sup) = (i as f64 / SAMPLES as f64, (i + 1) as f64 / SAMPLES as f64);
        let (d_inf, d_sup) = (table[i], table[i + 1]);
        let velocity_coef = (d_sup - d_inf) / (t_sup - t_inf);
        let coef = d_inf + (t - t_inf) * velocity_coef;
        (
            self.from + coef * self.distance,
            velocity_coef * self.distance / self.duration * 1000.,
            false,
        )
    }
}

/// A scroll eased from one offset to another: a wheel's (ease out) or a
/// guest's animated scroll (Android's accelerate-decelerate).
#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f64,
    to: f64,
    start: f64,
    duration: f64,
    ease_out: bool,
}

impl Tween {
    fn at(&self, now: f64) -> (f64, bool) {
        let t = ((now - self.start) / self.duration.max(1.)).clamp(0., 1.);
        let k = if self.ease_out {
            1. - (1. - t).powi(3)
        } else {
            (((t + 1.) * std::f64::consts::PI).cos() / 2.) + 0.5
        };
        (self.from + (self.to - self.from) * k, t >= 1.)
    }
}

/// The stretch at one end, as `EdgeEffect` in its stretch mode: a pull
/// while dragged past the end, a spring back after.
#[derive(Clone, Copy, Debug, Default)]
struct Edge {
    /// How far past the end, in dp, and the spring's velocity.
    pull: f64,
    velocity: f64,
    /// When the spring was last stepped, if it runs.
    sprung: Option<f64>,
}

impl Edge {
    fn idle(&self) -> bool {
        self.pull <= 0. && self.sprung.is_none()
    }

    fn pull_by(&mut self, d: f64, size: f64) {
        self.sprung = None;
        self.velocity = 0.;
        self.pull = (self.pull + d).clamp(0., size.max(1.));
    }

    fn release(&mut self, now: f64) {
        if self.pull > 0. && self.sprung.is_none() {
            self.sprung = Some(now);
            self.velocity = 0.;
        }
    }

    /// `onAbsorb`: a fling that reached this end, at `speed` dp per second.
    fn absorb(&mut self, speed: f64, now: f64) {
        self.velocity = speed.abs() * ON_ABSORB_VELOCITY_ADJUSTMENT;
        self.sprung = Some(now);
    }

    /// `updateSpring`: the underdamped spring from the last step to `now`.
    fn step(&mut self, now: f64, size: f64) {
        let Some(at) = self.sprung else { return };
        let dt = ((now - at) / 1000.).max(0.);
        self.sprung = Some(now);
        let damped = NATURAL_FREQUENCY * (1. - DAMPING_RATIO * DAMPING_RATIO).sqrt();
        let cos_c = self.pull;
        let sin_c = (DAMPING_RATIO * NATURAL_FREQUENCY * self.pull + self.velocity) / damped;
        let decay = (-DAMPING_RATIO * NATURAL_FREQUENCY * dt).exp();
        let (s, c) = (damped * dt).sin_cos();
        let pull = decay * (cos_c * c + sin_c * s);
        let velocity = pull * -NATURAL_FREQUENCY * DAMPING_RATIO
            + decay * (-damped * cos_c * s + damped * sin_c * c);
        self.pull = pull;
        self.velocity = velocity;
        if self.pull > size.max(1.) {
            self.pull = size.max(1.);
            self.velocity = 0.;
        }
        // `isAtEquilibrium`, in dp.
        if self.pull < 0. || (velocity.abs() < 1. && self.pull < 0.05) {
            *self = Edge::default();
        }
    }
}

/// A finger's (or the mouse's) position over time, for the velocity at
/// lift, as `VelocityTracker` measures it: a least-squares line through
/// the last 100 ms, and nothing if the pointer stood still for 40 ms.
#[derive(Clone, Debug, Default)]
struct Tracker {
    samples: Vec<(f64, f64)>,
}

impl Tracker {
    fn add(&mut self, t: f64, at: f64) {
        self.samples.retain(|(s, _)| t - s <= 100.);
        self.samples.push((t, at));
    }

    fn velocity(&self, now: f64) -> f64 {
        let Some(&(last, _)) = self.samples.last() else {
            return 0.;
        };
        if now - last > 40. {
            return 0.;
        }
        let s: Vec<_> = self.samples.iter().filter(|(t, _)| last - t <= 100.).collect();
        if s.len() < 2 {
            return 0.;
        }
        let n = s.len() as f64;
        let mt = s.iter().map(|(t, _)| t).sum::<f64>() / n;
        let mx = s.iter().map(|(_, x)| x).sum::<f64>() / n;
        let (mut num, mut den) = (0., 0.);
        for (t, x) in &s {
            num += (t - mt) * (x - mx);
            den += (t - mt) * (t - mt);
        }
        if den <= 0. { 0. } else { num / den * 1000. }
    }
}

/// One scroll view's motion.
#[derive(Clone, Debug, Default)]
pub struct Scroller {
    /// The offset this last set, in dp.
    pub pos: f32,
    fling: Option<Fling>,
    tween: Option<Tween>,
    /// The ends: start (top or left) and end.
    start: Edge,
    end: Edge,
    tracker: Tracker,
    /// The pointer's travel since the press, for the tracker.
    travel: f64,
    /// A touchpad's or touchscreen's precise scroll is under way, last
    /// moved at this time.
    precise: Option<f64>,
    /// The box's length along the axis, as last given.
    size: f32,
}

impl Scroller {
    /// Whether it still moves: a fling, a stretch springing back, or a
    /// precise scroll that hasn't been released.
    pub fn active(&self) -> bool {
        self.moving() || !self.start.idle() || !self.end.idle() || self.precise.is_some()
    }

    /// Whether a fling or an eased scroll moves the content.
    pub fn moving(&self) -> bool {
        self.fling.is_some() || self.tween.is_some()
    }

    /// The content before the anchor grew by `d` dp (or shrank, `d`
    /// negative): the offset and any motion under way move with it, so
    /// what is on screen stays put.
    pub fn shift(&mut self, d: f32) {
        let d64 = f64::from(d);
        self.pos += d;
        if let Some(f) = &mut self.fling {
            f.from += d64;
        }
        if let Some(t) = &mut self.tween {
            t.from += d64;
            t.to += d64;
        }
    }

    /// Stop any motion and stand at `pos`, as a guest's scroll does.
    pub fn jump(&mut self, pos: f32) {
        self.fling = None;
        self.tween = None;
        self.pos = pos;
    }

    /// Ease to `to` over `duration` ms on Android's accelerate-decelerate
    /// curve, as a guest's `animated` scroll.
    pub fn animate_to(&mut self, to: f32, duration: f64, now: f64) {
        self.fling = None;
        self.tween = Some(Tween {
            from: f64::from(self.pos),
            to: f64::from(to),
            start: now,
            duration,
            ease_out: false,
        });
    }

    #[cfg(test)]
    pub fn flinging(&self) -> bool {
        self.fling.is_some()
    }

    /// The content's shift for the stretch, in dp: positive away from the
    /// start, negative away from the end.
    pub fn stretch(&self) -> f32 {
        let size = self.size.max(1.);
        let shown = |pull: f64| STRETCH_SHOWN * size * (1. - (-3. * pull as f32 / size).exp());
        shown(self.start.pull) - shown(self.end.pull)
    }

    /// A press on the view: a fling stops, as a touch stops Android's.
    pub fn press(&mut self, pos: f32, now: f64) {
        self.pos = pos;
        self.fling = None;
        self.tween = None;
        self.tracker = Tracker::default();
        self.travel = 0.;
        self.tracker.add(now, 0.);
    }

    /// The pointer moved by `delta` dp along the axis while pressed: the
    /// content follows it, `delta` positive toward the content's start.
    /// Past either end it pulls that end instead.
    pub fn drag(&mut self, delta: f32, max: f32, size: f32, now: f64) {
        self.travel += f64::from(delta);
        self.tracker.add(now, self.travel);
        self.move_by(-delta, max, size);
    }

    /// The pointer lifted: a fling at the tracked velocity, and any
    /// stretch springs back.
    pub fn lift(&mut self, density: f32, now: f64) {
        let v = (-self.tracker.velocity(now)).clamp(-f64::from(MAX_FLING), f64::from(MAX_FLING));
        self.start.release(now);
        self.end.release(now);
        if v.abs() >= f64::from(MIN_FLING) {
            self.fling_at(v, density, now);
        }
    }

    /// A precise scroll (touchpad or touchscreen) of `delta` dp toward the
    /// content's end. The platform supplies its own momentum, so the
    /// content follows it, and past an end it pulls the edge.
    pub fn precise(&mut self, delta: f32, max: f32, size: f32, now: f64) {
        self.fling = None;
        self.tween = None;
        self.precise = Some(now);
        self.move_by(delta, max, size);
    }

    /// A precise scroll ended: any stretch springs back.
    pub fn precise_end(&mut self, now: f64) {
        self.precise = None;
        self.start.release(now);
        self.end.release(now);
    }

    /// A wheel's notches, `delta` dp toward the content's end: the content
    /// eases to where the wheel has turned to, and a run of notches adds
    /// up. At an end the wheel does nothing, as a wheel over an Android
    /// list or a desktop page shows no overscroll.
    pub fn wheel(&mut self, delta: f32, max: f32, size: f32, now: f64) {
        self.size = size;
        self.fling = None;
        let target = match self.tween {
            Some(t) if (t.to > t.from) == (delta > 0.) => t.to,
            _ => f64::from(self.pos),
        } + f64::from(delta);
        let to = target.clamp(0., f64::from(max.max(0.)));
        if (to - f64::from(self.pos)).abs() < 0.01 {
            self.tween = None;
            return;
        }
        self.tween = Some(Tween {
            from: f64::from(self.pos),
            to,
            start: now,
            duration: WHEEL_MS,
            ease_out: true,
        });
    }

    /// A fling at `v` dp per second toward the content's end.
    pub fn fling_at(&mut self, v: f64, density: f32, now: f64) {
        self.tween = None;
        let curve = Curve::new(density);
        let (distance, duration) = curve.fling(v.abs());
        self.fling = Some(Fling {
            from: f64::from(self.pos),
            distance: distance.copysign(v),
            duration,
            start: now,
        });
    }

    /// The motion at `now`: the new offset, a fling stopped at an end that
    /// stretches it, the stretch's spring, and a precise scroll that has
    /// gone quiet released.
    pub fn step(&mut self, max: f32, size: f32, now: f64) -> f32 {
        self.size = size;
        let (max64, size64) = (f64::from(max.max(0.)), f64::from(size));
        if let Some(f) = self.fling {
            let (at, v, done) = f.at(now);
            if at < 0. || at > max64 {
                self.pos = at.clamp(0., max64) as f32;
                self.fling = None;
                // A box with nothing to scroll doesn't stretch, as
                // Android's `OVER_SCROLL_IF_CONTENT_SCROLLS`.
                if max64 > 0. && v.abs() > 0. {
                    if at < 0. {
                        self.start.absorb(v, now);
                    } else {
                        self.end.absorb(v, now);
                    }
                }
            } else {
                self.pos = at as f32;
                if done {
                    self.fling = None;
                }
            }
        }
        if let Some(t) = self.tween {
            let (at, done) = t.at(now);
            self.pos = at.clamp(0., max64) as f32;
            if done || at <= 0. || at >= max64 {
                self.tween = None;
            }
        }
        if self.precise.is_some_and(|t| now - t > 80.) {
            self.precise_end(now);
        }
        self.start.step(now, size64);
        self.end.step(now, size64);
        self.pos = self.pos.clamp(0., max.max(0.));
        self.pos
    }

    fn move_by(&mut self, d: f32, max: f32, size: f32) {
        self.size = size;
        let size = f64::from(size);
        let mut d = f64::from(d);
        // A pull is taken back before the content moves again.
        for (edge, sign) in [(&mut self.start, -1.), (&mut self.end, 1.)] {
            if edge.pull > 0. && d * sign < 0. {
                let back = edge.pull.min(d.abs());
                edge.pull_by(-back, size);
                d += back * sign;
            }
        }
        let at = f64::from(self.pos) + d;
        let max = f64::from(max.max(0.));
        if at < 0. {
            if max > 0. {
                self.start.pull_by(-at, size);
            }
            self.pos = 0.;
        } else if at > max {
            if max > 0. {
                self.end.pull_by(at - max, size);
            }
            self.pos = max as f32;
        } else {
            self.pos = at as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spline_runs_from_zero_to_one() {
        let t = spline();
        assert!(t[0].abs() < 1e-4);
        assert!((t[SAMPLES] - 1.).abs() < 1e-9);
        assert!(t.windows(2).all(|w| w[1] >= w[0]));
        // Most of the distance is covered early: the curve decelerates.
        assert!(t[50] > 0.75, "{}", t[50]);
    }

    #[test]
    fn fling_matches_android_at_a_phones_density() {
        // 2000 dp/s at 2.625 (420 dpi): Android's `getSplineFlingDistance`
        // and `getSplineFlingDuration` in pixels, converted to dp.
        let c = Curve::new(2.625);
        let (d, ms) = c.fling(2000.);
        let fc: f64 = 0.015 * 9.80665 * 39.37 * 420. * 0.84;
        let l = (0.35 * 2000. * 2.625 / fc).ln();
        let r = deceleration_rate();
        assert!((ms - 1000. * (l / (r - 1.)).exp()).abs() < 1e-6);
        assert!((d - fc * (r / (r - 1.) * l).exp() / 2.625).abs() < 1e-6);
        assert!(d > 300. && d < 1500., "{d} dp");
        assert!(ms > 500. && ms < 3000., "{ms} ms");
        // The inverse gives the speed back.
        assert!((c.speed_for(d) - 2000.).abs() < 1e-6);
    }

    #[test]
    fn fling_decelerates_and_ends_where_it_said() {
        let mut s = Scroller::default();
        s.fling_at(3000., 1.25, 0.);
        let mut last = 0.;
        let mut steps = vec![];
        let mut t = 0.;
        while s.flinging() {
            t += 16.;
            let p = s.step(10_000., 800., t);
            steps.push(p - last);
            last = p;
        }
        assert!(steps.len() > 20);
        // Each frame covers less than the one before, past the first few.
        assert!(steps[2] > steps[steps.len() / 2]);
        assert!(steps[steps.len() / 2] > *steps.last().unwrap());
        let (d, _) = Curve::new(1.25).fling(3000.);
        assert!((f64::from(last) - d).abs() < 1., "{last} vs {d}");
    }

    #[test]
    fn a_fling_into_the_end_stretches_and_springs_back() {
        let mut s = Scroller {
            pos: 900.,
            ..Scroller::default()
        };
        s.fling_at(4000., 1.25, 0.);
        let mut t = 0.;
        let mut most: f32 = 0.;
        while s.active() && t < 3000. {
            t += 16.;
            s.step(1000., 800., t);
            most = most.min(s.stretch());
        }
        assert!((s.pos - 1000.).abs() < 1e-3);
        assert!(most < -5., "the content moved back from the end by {most} dp");
        assert!(!s.active(), "still moving after {t} ms");
        assert!(s.stretch().abs() < 0.01);
    }

    #[test]
    fn a_drag_past_the_start_pulls_and_lift_releases() {
        let mut s = Scroller::default();
        s.press(0., 0.);
        for i in 1..=10 {
            s.drag(10., 500., 800., f64::from(i) * 10.);
        }
        assert!(s.pos.abs() < 1e-3);
        assert!(s.stretch() > 0.);
        // Dragging back takes the pull back first.
        s.drag(-50., 500., 800., 110.);
        assert!(s.pos.abs() < 1e-3);
        s.drag(-80., 500., 800., 120.);
        assert!(s.pos > 0.);
        s.lift(1.25, 130.);
        let mut t = 130.;
        while s.active() && t < 5000. {
            t += 16.;
            s.step(500., 800., t);
        }
        assert!(!s.active());
        assert!(s.stretch().abs() < 0.01);
    }

    #[test]
    fn a_quick_drag_flings_and_a_held_one_does_not() {
        let mut s = Scroller::default();
        s.press(100., 0.);
        for i in 1..=5 {
            s.drag(-20., 5000., 800., f64::from(i) * 16.);
        }
        s.lift(1.25, 80.);
        assert!(s.flinging());
        let mut h = Scroller::default();
        h.press(100., 0.);
        h.drag(-100., 5000., 800., 16.);
        h.lift(1.25, 200.);
        assert!(!h.flinging());
    }

    #[test]
    fn wheel_notches_add_up_and_settle_quickly() {
        let mut s = Scroller::default();
        s.wheel(WHEEL_NOTCH, 5000., 800., 0.);
        s.step(5000., 800., 8.);
        // Most of a notch after the first frame or two: the content
        // follows the wheel, without a fling's long tail.
        let first = s.pos;
        s.wheel(WHEEL_NOTCH, 5000., 800., 10.);
        let mut t = 10.;
        let mut steps = vec![];
        while s.active() {
            t += 16.;
            s.step(5000., 800., t);
            steps.push(s.pos);
        }
        assert!(first > 0. && first < WHEEL_NOTCH);
        assert!((s.pos - 2. * WHEEL_NOTCH).abs() < 0.01, "{}", s.pos);
        assert!(t <= 10. + WHEEL_MS + 16., "settled after {t} ms");
        assert!(steps[0] > first + WHEEL_NOTCH * 0.25, "{steps:?}");
        assert!(steps.windows(2).all(|w| w[1] >= w[0]), "{steps:?}");
    }

    #[test]
    fn the_wheel_at_an_end_does_nothing() {
        let mut s = Scroller::default();
        s.wheel(-WHEEL_NOTCH, 5000., 800., 0.);
        assert!(!s.active());
        assert!(s.stretch().abs() < 1e-6);
    }

    #[test]
    fn a_shift_carries_a_fling_with_the_content() {
        let mut s = Scroller {
            pos: 500.,
            ..Scroller::default()
        };
        s.fling_at(-2000., 1.25, 0.);
        let a = s.step(10_000., 800., 50.);
        s.shift(300.);
        let b = s.step(10_000., 800., 66.);
        // The fling goes on from where the content moved to: one frame's
        // travel, not a jump back.
        assert!(b < a + 300. && b > a + 300. - 40., "{a} then {b}");
    }
}
