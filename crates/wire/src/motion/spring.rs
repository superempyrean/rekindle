//! The engine's spring, spec tag 2 ([`super::op::Anim::Spring`]), as the
//! Android host runs it.
//!
//! Recovered from build 356207: the solver at `0x5543c44`, the start of a
//! spring at `0x55313d4` and its per-frame step at `0x55332ac`. The engine
//! keeps the displacement and velocity the spring started with and
//! evaluates the closed-form solution at the time since it started, so the
//! curve doesn't depend on the frame rate. Each frame's `dt` is cut to at
//! most 64 ms first. The arithmetic is `f32`, in the engine's order.
//! Emulating the engine's `tick` gives the same values to within 1e-4.

/// The most milliseconds one frame advances an animation. The engine cuts
/// a longer `dt` to this, and a negative one to 0.
pub const MAX_FRAME_MS: f32 = 64.0;

/// A settle by energy happens at this fraction of the starting energy.
const ENERGY_FRACTION: f32 = 6.0e-9;

/// A damped harmonic oscillator. The engine clamps `mass` and `stiffness`
/// to at least 1e-4 before solving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub mass: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl Spring {
    /// The displacement from the target and the velocity (units per
    /// second) `t` seconds after the spring started at displacement `x0`
    /// with velocity `v0`.
    ///
    /// With `ζ = damping / (2 sqrt(mass * stiffness))` and
    /// `ω = sqrt(stiffness / mass)`, a spring with `ζ < 1` oscillates:
    /// `x(t) = e^(-ζωt) (x0 cos(ω_d t) + B sin(ω_d t))`, with
    /// `ω_d = ω sqrt(1 - ζ²)` and `B = (v0 + ζω x0) / ω_d`. Any other spring
    /// takes the critically damped form `x(t) = e^(-ωt) (x0 + (v0 + ω x0) t)`.
    /// The engine has no overdamped case, so a spring with `ζ > 1` moves
    /// exactly as the critical one with the same mass and stiffness.
    pub fn solve(&self, x0: f32, v0: f32, t: f32) -> (f32, f32) {
        let m = self.mass.max(1e-4);
        let k = self.stiffness.max(1e-4);
        let w0 = (k / m).sqrt();
        let root = (m * k).sqrt();
        let zeta = self.damping / (root + root);
        if zeta < 1.0 {
            let a = w0 * zeta;
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let ax0 = x0 * a;
            let b = (v0 + ax0) / wd;
            let e = (-(a * t)).exp();
            let (sin, cos) = (t * wd).sin_cos();
            let x = e * (x0 * cos + b * sin);
            let v = e * (cos * (wd * b - ax0) + sin * (-(a * b) - x0 * wd));
            (x, v)
        } else {
            let e = (-(w0 * t)).exp();
            let b = v0 + x0 * w0;
            let p = x0 + t * b;
            (e * p, e * (b - w0 * p))
        }
    }
}

/// A running spring toward `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringRun {
    pub to: f32,
    spring: Spring,
    /// Displacement and velocity at the start.
    x0: f32,
    v0: f32,
    /// Milliseconds since the start, each frame's share cut to
    /// [`MAX_FRAME_MS`].
    elapsed: f32,
    /// The energy at the start, `m v0² / 2 + k x0² / 2`.
    energy: f32,
    clamp: bool,
    /// The rest thresholds, `(displacement, speed)`, when spec flag bit 2
    /// is set.
    rest: Option<(f32, f32)>,
    /// Settled on an earlier frame. The engine drops a settled
    /// animation, so later frames stay on `to`.
    done: bool,
}

/// One frame of a [`SpringRun`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub value: f32,
    /// Units per second.
    pub velocity: f32,
    /// The spring settled this frame. `value` is then `to` and `velocity` 0.
    pub settled: bool,
}

impl SpringRun {
    /// Start from `from` toward `to`. `flags` and the rest of the fields
    /// are spec tag 2's: bit 0 clamps overshoot, bit 1 takes `velocity`
    /// instead of `inherited`, and bit 2 keeps a velocity pointing away
    /// from `to` and settles by the rest thresholds. Without bit 2, a
    /// velocity pointing away from `to` is dropped, and the spring settles
    /// when its energy falls below a fixed fraction of the starting energy.
    #[expect(
        clippy::too_many_arguments,
        reason = "spec tag 2's fields, in its order"
    )]
    pub fn start(
        from: f32,
        to: f32,
        spring: Spring,
        flags: u32,
        rest_displacement: f32,
        rest_speed: f32,
        velocity: f32,
        inherited: f32,
    ) -> SpringRun {
        let keep = flags & 4 != 0;
        let x0 = from - to;
        let mut v0 = if flags & 2 != 0 { velocity } else { inherited };
        let toward = x0 * v0 <= 0.0;
        if !toward && !keep {
            v0 = 0.0;
        }
        let energy = v0 * ((spring.mass * 0.5) * v0) + x0 * (x0 * (spring.stiffness * 0.5));
        SpringRun {
            to,
            spring,
            x0,
            v0,
            elapsed: 0.0,
            energy,
            clamp: flags & 1 != 0,
            rest: keep.then_some((rest_displacement, rest_speed)),
            done: false,
        }
    }

    /// Advance by `dt` milliseconds.
    pub fn step(&mut self, dt: f32) -> Sample {
        if self.done {
            return Sample {
                value: self.to,
                velocity: 0.0,
                settled: true,
            };
        }
        self.elapsed += frame_ms(dt);
        let (x, v) = self.spring.solve(self.x0, self.v0, self.elapsed / 1000.0);
        let crossed = self.clamp && self.x0 != 0.0 && sign(x) != sign(self.x0);
        let s = self.spring;
        let settled = crossed
            || if let Some((rd, rs)) = self.rest {
                x.abs() < rd && v.abs() < rs
            } else {
                self.energy <= 0.0
                    || v * (v * (s.mass * 0.5)) + x * (x * (s.stiffness * 0.5))
                        <= self.energy * ENERGY_FRACTION
            };
        if settled {
            self.done = true;
            Sample {
                value: self.to,
                velocity: 0.0,
                settled,
            }
        } else {
            Sample {
                value: x + self.to,
                velocity: v,
                settled,
            }
        }
    }
}

/// `dt` as one frame advances an animation: at least 0 and at most
/// [`MAX_FRAME_MS`].
pub fn frame_ms(dt: f32) -> f32 {
    let dt = if dt < 0.0 { 0.0 } else { dt };
    if dt > MAX_FRAME_MS { MAX_FRAME_MS } else { dt }
}

/// 1 or -1 with `x`'s sign, NaN for NaN, so that NaN differs from either.
fn sign(x: f32) -> f32 {
    if x.is_nan() {
        f32::NAN
    } else {
        1f32.copysign(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPRING: Spring = Spring {
        mass: 1.0,
        stiffness: 100.0,
        damping: 10.0,
    };

    #[test]
    fn the_solution_starts_where_it_was_given() {
        for s in [
            SPRING,
            Spring {
                damping: 20.0,
                ..SPRING
            },
        ] {
            let (x, v) = s.solve(-1.0, 3.0, 0.0);
            assert!((x + 1.0).abs() < 1e-6 && (v - 3.0).abs() < 1e-5, "{s:?}");
        }
    }

    #[test]
    fn an_overdamped_spring_moves_as_the_critical_one() {
        let critical = Spring {
            damping: 20.0,
            ..SPRING
        };
        let over = Spring {
            damping: 40.0,
            ..SPRING
        };
        for t in [0.016, 0.1, 0.5] {
            assert_eq!(critical.solve(-1.0, 0.0, t), over.solve(-1.0, 0.0, t));
        }
    }

    #[test]
    fn a_frame_is_at_most_64_ms() {
        assert_eq!(frame_ms(100.0), 64.0);
        assert_eq!(frame_ms(-5.0), 0.0);
        assert_eq!(frame_ms(16.0), 16.0);
    }
}
