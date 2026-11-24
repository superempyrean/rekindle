//! A navigator's transition between two screens, as the Android platform's
//! `NavigatorLayout` runs it (`modules.ih`, decompiled from the 356207 base
//! APK). The motion engine has no part in it: Java animates a progress from
//! 0 to 1 and moves the two screens from it (`ih.b`).
//!
//! The kind comes from the screen on top (the new one on a push, the old
//! one on a pop): `modal` for `presentation: "modal"` or
//! `"transparentModal"`, `panels` for `panelBack`, `card` for
//! `presentation: "card"`, and otherwise the navigator's `transition`,
//! `"slide"` by default.

use std::time::Instant;

use rekindle_wire::NodeId;

/// How a screen is drawn this frame: its translation in dp and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub node: NodeId,
    pub tx: f32,
    pub ty: f32,
    pub alpha: f32,
}

#[derive(Clone, Debug)]
pub struct NavAnim {
    pub from: NodeId,
    pub to: NodeId,
    /// The new screen comes after the old one in the navigator's children.
    pub push: bool,
    pub kind: String,
    start: Instant,
}

impl NavAnim {
    pub fn new(from: NodeId, to: NodeId, push: bool, kind: String) -> NavAnim {
        NavAnim {
            from,
            to,
            push,
            kind,
            start: Instant::now(),
        }
    }

    /// The duration in ms and the interpolator of each kind (`ih.a`).
    fn timing(&self) -> (f32, fn(f32) -> f32) {
        match self.kind.as_str() {
            "slide" => (450.0, emphasized),
            "card" => (300.0, |t| cubic(0.0, 0.0, 0.2, 1.0, t)),
            "stack" | "stackFade" => (425.0, |t| cubic(0.20833, 0.82, 0.25, 1.0, t)),
            "modal" => (400.0, accelerate_decelerate),
            "fade" => (150.0, |t| t),
            "nativeFade" => (150.0, accelerate_decelerate),
            // Toward the pushed panel 250 ms, back 200, along (0.4, 0,
            // 0.2, 1).
            "panels" => (if self.push { 250.0 } else { 200.0 }, |t| {
                cubic(0.4, 0.0, 0.2, 1.0, t)
            }),
            _ => (0.0, |t| t),
        }
    }

    pub fn done(&self) -> bool {
        self.start.elapsed().as_secs_f32() * 1000.0 >= self.timing().0
    }

    /// The two screens in the navigator's order, given whether `from`
    /// comes first, in a box `w` by `h` dp (`ih.b`).
    pub fn places(&self, from_first: bool, w: f32, h: f32) -> Vec<Place> {
        let (ms, interp) = self.timing();
        let l = self.start.elapsed().as_secs_f32() * 1000.0;
        let g = if ms <= 0.0 {
            1.0
        } else {
            interp((l / ms).clamp(0.0, 1.0))
        };
        let mut from = Place {
            node: self.from,
            tx: 0.0,
            ty: 0.0,
            alpha: 1.0,
        };
        let mut to = Place {
            node: self.to,
            ..from
        };
        match (self.kind.as_str(), self.push) {
            ("panels", true) => to.tx = (1.0 - g) * w,
            ("panels", false) => from.tx = w * g,
            ("card" | "stack" | "stackFade", true) => {
                to.tx = (1.0 - g) * w;
                from.tx = -w * 0.3 * g;
            }
            ("card" | "stack" | "stackFade", false) => {
                to.tx = (1.0 - g) * -w * 0.3;
                from.tx = w * g;
            }
            ("modal", true) => to.ty = (1.0 - g) * h,
            ("modal", false) => from.ty = h * g,
            ("slide", true) => {
                to.tx = (1.0 - g) * w * 0.1;
                from.tx = -w * 0.1 * g;
                to.alpha = ((l - 50.0) / 83.0).clamp(0.0, 1.0);
            }
            ("slide", false) => {
                to.tx = (1.0 - g) * -w * 0.1;
                from.tx = 0.1 * w * g;
                from.alpha = 1.0 - ((l - 35.0) / 83.0).clamp(0.0, 1.0);
            }
            ("fade" | "nativeFade", _) => {
                to.alpha = g;
                from.alpha = 1.0 - g;
            }
            _ => {}
        }
        if from_first {
            vec![from, to]
        } else {
            vec![to, from]
        }
    }
}

/// `pg.f8489a`: a path of two cubics, (0, 0) to (0.166666, 0.4) through
/// (0.05, 0) and (0.133333, 0.06), then to (1, 1) through (0.208333, 0.82)
/// and (0.25, 1).
fn emphasized(t: f32) -> f32 {
    if t <= 0.166_666 {
        segment(
            [(0.0, 0.0), (0.05, 0.0), (0.133_333, 0.06), (0.166_666, 0.4)],
            t,
        )
    } else {
        segment(
            [(0.166_666, 0.4), (0.208_333, 0.82), (0.25, 1.0), (1.0, 1.0)],
            t,
        )
    }
}

/// `AccelerateDecelerateInterpolator`.
fn accelerate_decelerate(t: f32) -> f32 {
    ((t + 1.0) * std::f32::consts::PI).cos() / 2.0 + 0.5
}

/// A CSS cubic Bezier from (0, 0) to (1, 1).
fn cubic(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    segment([(0.0, 0.0), (x1, y1), (x2, y2), (1.0, 1.0)], t)
}

/// The `y` of a cubic segment whose `x` rises monotonically, where `x` is
/// `t`, by bisection on the parameter.
fn segment(p: [(f32, f32); 4], t: f32) -> f32 {
    let at = |s: f32, i: usize| {
        let u = 1.0 - s;
        let c = |k: usize| if i == 0 { p[k].0 } else { p[k].1 };
        u * u * u * c(0) + 3.0 * u * u * s * c(1) + 3.0 * u * s * s * c(2) + s * s * s * c(3)
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let mid = f32::midpoint(lo, hi);
        if at(mid, 0) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(f32::midpoint(lo, hi), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolators_run_from_zero_to_one() {
        for f in [emphasized, accelerate_decelerate, |t| {
            cubic(0.0, 0.0, 0.2, 1.0, t)
        }] {
            assert!(f(0.0).abs() < 1e-3);
            assert!((f(1.0) - 1.0).abs() < 1e-3);
            let mut last = -1.0;
            for i in 0..=20 {
                let v = f(i as f32 / 20.0);
                assert!(v >= last - 1e-4, "not monotonic");
                last = v;
            }
        }
        // The emphasized curve passes its joint.
        assert!((emphasized(0.166_666) - 0.4).abs() < 1e-3);
    }

    #[test]
    fn a_settled_push_leaves_the_new_screen_in_place() {
        let mut a = NavAnim::new(1, 2, true, "card".into());
        a.start -= std::time::Duration::from_millis(1000);
        assert!(a.done());
        let p = a.places(true, 400.0, 800.0);
        assert_eq!(
            p[1],
            Place {
                node: 2,
                tx: 0.0,
                ty: 0.0,
                alpha: 1.0
            }
        );
        assert!((p[0].tx + 120.0).abs() < 1e-3);
    }
}
