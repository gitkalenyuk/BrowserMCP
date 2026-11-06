//! Human-like input (opt-in per tab with `input.humanlike`).
//!
//! The default "fast path" of `page.act` teleports the mouse to the element and inserts text
//! in one go. With human-like input on, the same actions are sent the way a person produces
//! them:
//!
//! * **mouse** — a curved path (cubic Bézier with random control points) from the last mouse
//!   position, minimum-jerk easing (slow–fast–slow), a slight overshoot and correction on long
//!   moves, a short dwell before pressing and a realistic button hold time;
//! * **typing** — one key event per character with gaussian delays from the words-per-minute
//!   setting, a little longer after spaces;
//! * **scrolling** — mouse-wheel notches with pauses instead of one instant jump.
//!
//! Everything random comes from a seeded [`Rng`], so the paths are reproducible in tests.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tunables of human-like input.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanProfile {
    /// Average mouse speed in CSS pixels per second (default 1200; 600 = slow, 2500 = fast).
    #[serde(default = "default_speed")]
    pub mouse_speed: f64,
    /// Typing speed in words per minute (5 characters per word; default 55).
    #[serde(default = "default_wpm")]
    pub typing_wpm: f64,
    /// How irregular things are, 0 (robotic) to 1 (sloppy). Default 0.35.
    #[serde(default = "default_jitter")]
    pub jitter: f64,
}

fn default_speed() -> f64 {
    1200.0
}
fn default_wpm() -> f64 {
    55.0
}
fn default_jitter() -> f64 {
    0.35
}

impl Default for HumanProfile {
    fn default() -> Self {
        HumanProfile {
            mouse_speed: default_speed(),
            typing_wpm: default_wpm(),
            jitter: default_jitter(),
        }
    }
}

impl HumanProfile {
    /// Clamp to sane ranges (an agent may pass 0 or 1e9).
    pub fn sanitized(self) -> Self {
        let finite = |v: f64, d: f64| if v.is_finite() { v } else { d };
        HumanProfile {
            mouse_speed: finite(self.mouse_speed, default_speed()).clamp(100.0, 10_000.0),
            typing_wpm: finite(self.typing_wpm, default_wpm()).clamp(5.0, 250.0),
            jitter: finite(self.jitter, default_jitter()).clamp(0.0, 1.0),
        }
    }
}

/// Per-tab human-input state: the profile and its random generator.
#[derive(Debug, Clone)]
pub struct Human {
    pub profile: HumanProfile,
    pub rng: Rng,
}

impl Human {
    pub fn new(profile: HumanProfile, seed: u64) -> Self {
        Human {
            profile: profile.sanitized(),
            rng: Rng::new(seed),
        }
    }
}

// ---------------------------------------------------------------- random numbers

/// SplitMix64: tiny, seedable, good enough for timing noise (not cryptography).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn from_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Rng(nanos)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Normal distribution (Box–Muller).
    pub fn gauss(&mut self, mean: f64, sd: f64) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        mean + sd * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    /// A duration of about `mean_ms` (± `spread` as a fraction), never below a third of it.
    pub fn millis(&mut self, mean_ms: f64, spread: f64) -> Duration {
        let ms = self.gauss(mean_ms, mean_ms * spread).max(mean_ms / 3.0);
        Duration::from_micros((ms * 1000.0) as u64)
    }
}

// ---------------------------------------------------------------- mouse paths

/// One `mouseMoved` event and the pause before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseStep {
    pub x: f64,
    pub y: f64,
    pub delay: Duration,
}

/// Minimum-jerk easing: slow start, fast middle, slow end (how arm movements look).
fn ease(t: f64) -> f64 {
    t * t * t * (10.0 - 15.0 * t + 6.0 * t * t)
}

fn bezier(p: [(f64, f64); 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0,
        a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1,
    )
}

/// One curved segment from `from` to `to` taking about `duration`, as `steps` points (the
/// last one exactly `to`).
fn segment(
    from: (f64, f64),
    to: (f64, f64),
    duration: f64,
    profile: &HumanProfile,
    rng: &mut Rng,
) -> Vec<MouseStep> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let dist = (dx * dx + dy * dy).sqrt();
    // ~60 Hz like a real mouse, at least a few points.
    let steps = ((duration * 60.0).round() as usize).clamp(3, 120);
    // Control points pushed sideways (perpendicular to the line) for a natural arc.
    let (nx, ny) = if dist > 0.0 {
        (-dy / dist, dx / dist)
    } else {
        (0.0, 0.0)
    };
    let bend = dist * (0.08 + 0.2 * profile.jitter);
    let o1 = rng.gauss(0.0, bend);
    let o2 = rng.gauss(0.0, bend);
    let p = [
        from,
        (from.0 + dx * 0.3 + nx * o1, from.1 + dy * 0.3 + ny * o1),
        (from.0 + dx * 0.7 + nx * o2, from.1 + dy * 0.7 + ny * o2),
        to,
    ];
    let mean_delay = duration * 1000.0 / steps as f64;
    let wobble = 0.4 * profile.jitter;
    (1..=steps)
        .map(|i| {
            let t = ease(i as f64 / steps as f64);
            let (mut x, mut y) = bezier(p, t);
            if i < steps {
                // Hand tremor: a fraction of a pixel.
                x += rng.gauss(0.0, wobble);
                y += rng.gauss(0.0, wobble);
            }
            MouseStep {
                x,
                y,
                delay: rng.millis(mean_delay, 0.25 * (0.3 + profile.jitter)),
            }
        })
        .collect()
}

/// The whole movement from `from` to `to`: a curve, on longer moves a small overshoot past
/// the target and a short correction back. The last step is exactly `to`.
pub fn mouse_path(
    from: (f64, f64),
    to: (f64, f64),
    profile: &HumanProfile,
    rng: &mut Rng,
) -> Vec<MouseStep> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 2.0 {
        return vec![MouseStep {
            x: to.0,
            y: to.1,
            delay: rng.millis(16.0, 0.2),
        }];
    }
    // Fitts-like: a fixed reaction part plus distance / speed, with some randomness.
    let speed = profile.mouse_speed * rng.range(0.8, 1.2);
    let duration = 0.12 + dist / speed;

    let overshoot = dist > 120.0 && rng.unit() < 0.35 + 0.5 * profile.jitter;
    if !overshoot {
        return segment(from, to, duration, profile, rng);
    }
    // Overshoot 3-8% of the distance (max 30 px) along the movement, a bit to the side.
    let amount = (dist * rng.range(0.03, 0.08)).min(30.0);
    let (ux, uy) = (dx / dist, dy / dist);
    let side = rng.gauss(0.0, amount * 0.3);
    let past = (
        to.0 + ux * amount - uy * side,
        to.1 + uy * amount + ux * side,
    );
    let mut path = segment(from, past, duration * 0.9, profile, rng);
    path.extend(segment(past, to, 0.08 + duration * 0.15, profile, rng));
    path
}

/// Where the mouse comes from when we do not know its position yet: somewhere below and to
/// the side of the target, inside the viewport.
pub fn entry_point(to: (f64, f64), viewport: (f64, f64), rng: &mut Rng) -> (f64, f64) {
    let (w, h) = (viewport.0.max(1.0), viewport.1.max(1.0));
    let x = (to.0 + rng.range(-0.4, 0.4) * w).clamp(0.0, w - 1.0);
    let y = (to.1 + rng.range(0.1, 0.35) * h).clamp(0.0, h - 1.0);
    (x, y)
}

/// A point near `center` (people do not hit the exact middle), at most `max` px away.
pub fn aim(center: (f64, f64), max: f64, profile: &HumanProfile, rng: &mut Rng) -> (f64, f64) {
    let sd = max / 2.5 * (0.3 + profile.jitter);
    (
        center.0 + rng.gauss(0.0, sd).clamp(-max, max),
        center.1 + rng.gauss(0.0, sd).clamp(-max, max),
    )
}

// ---------------------------------------------------------------- typing and scrolling

/// Pause before typing the next character (after `prev`).
pub fn key_delay(prev: Option<char>, profile: &HumanProfile, rng: &mut Rng) -> Duration {
    // wpm words of 5 characters per minute.
    let mean = 60_000.0 / (profile.typing_wpm * 5.0);
    let mean = match prev {
        Some(' ') | Some('\n') => mean * 1.6,
        Some(c) if c.is_ascii_punctuation() => mean * 1.3,
        _ => mean,
    };
    rng.millis(mean, 0.2 + 0.4 * profile.jitter)
}

/// How long a key or mouse button is held down.
pub fn hold_time(rng: &mut Rng) -> Duration {
    rng.millis(70.0, 0.3)
}

/// Mouse-wheel notches for scrolling `total` pixels (sign = direction): ~100 px each like a
/// Windows wheel, with short pauses.
pub fn wheel_steps(total: f64, profile: &HumanProfile, rng: &mut Rng) -> Vec<(f64, Duration)> {
    let notch = 100.0;
    let count = (total.abs() / notch).ceil().max(1.0) as usize;
    let per = total / count as f64;
    (0..count)
        .map(|_| (per, rng.millis(55.0, 0.3 + 0.4 * profile.jitter)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn paths_are_deterministic_and_end_on_target() {
        let p = HumanProfile::default();
        let a = mouse_path((10.0, 500.0), (640.0, 120.0), &p, &mut Rng::new(7));
        let b = mouse_path((10.0, 500.0), (640.0, 120.0), &p, &mut Rng::new(7));
        assert_eq!(a, b);
        let c = mouse_path((10.0, 500.0), (640.0, 120.0), &p, &mut Rng::new(8));
        assert_ne!(a, c);
        for seed in 0..50 {
            let path = mouse_path((10.0, 500.0), (640.0, 120.0), &p, &mut Rng::new(seed));
            let last = path.last().unwrap();
            assert_eq!((last.x, last.y), (640.0, 120.0), "seed {seed}");
            assert!(path.len() >= 6, "seed {seed}: {} steps", path.len());
            // Steps are small (no teleporting) and the whole move takes a human time.
            let mut prev = (10.0, 500.0);
            for s in &path {
                assert!(dist(prev, (s.x, s.y)) < 120.0, "seed {seed}: jump");
                prev = (s.x, s.y);
            }
            let total: Duration = path.iter().map(|s| s.delay).sum();
            assert!(
                total > Duration::from_millis(250) && total < Duration::from_millis(2500),
                "{total:?}"
            );
        }
    }

    #[test]
    fn paths_curve_and_sometimes_overshoot() {
        let p = HumanProfile::default();
        let (from, to) = ((0.0, 0.0), (800.0, 0.0));
        let mut overshoots = 0;
        let mut curved = 0;
        for seed in 0..100 {
            let path = mouse_path(from, to, &p, &mut Rng::new(seed));
            if path.iter().any(|s| s.x > 803.0) {
                overshoots += 1;
            }
            if path.iter().map(|s| s.y.abs()).fold(0.0, f64::max) > 5.0 {
                curved += 1;
            }
        }
        assert!((20..=90).contains(&overshoots), "{overshoots} overshoots");
        assert!(curved > 80, "{curved} curved");
        // Short moves never overshoot.
        for seed in 0..50 {
            let path = mouse_path((0.0, 0.0), (60.0, 0.0), &p, &mut Rng::new(seed));
            assert!(path.iter().all(|s| s.x <= 60.5 + 2.0), "seed {seed}");
        }
        // Tiny moves are one step.
        assert_eq!(
            mouse_path((5.0, 5.0), (6.0, 5.0), &p, &mut Rng::new(1)).len(),
            1
        );
    }

    #[test]
    fn easing_is_slow_fast_slow() {
        let path = segment(
            (0.0, 0.0),
            (600.0, 0.0),
            0.6,
            &HumanProfile {
                jitter: 0.0,
                ..HumanProfile::default()
            },
            &mut Rng::new(3),
        );
        let speeds: Vec<f64> = path.windows(2).map(|w| (w[1].x - w[0].x).abs()).collect();
        let mid = speeds[speeds.len() / 2];
        assert!(
            mid > speeds[0] * 3.0 && mid > speeds[speeds.len() - 1] * 3.0,
            "{speeds:?}"
        );
    }

    #[test]
    fn typing_speed_follows_wpm() {
        let mut rng = Rng::new(11);
        let slow = HumanProfile {
            typing_wpm: 30.0,
            ..HumanProfile::default()
        };
        let fast = HumanProfile {
            typing_wpm: 120.0,
            ..HumanProfile::default()
        };
        let avg = |p: &HumanProfile, rng: &mut Rng| {
            (0..400)
                .map(|_| key_delay(Some('a'), p, rng).as_secs_f64())
                .sum::<f64>()
                / 400.0
        };
        let (s, f) = (avg(&slow, &mut rng), avg(&fast, &mut rng));
        // 30 wpm = 400 ms/char, 120 wpm = 100 ms/char (±15%).
        assert!((0.34..0.46).contains(&s), "{s}");
        assert!((0.085..0.115).contains(&f), "{f}");
    }

    #[test]
    fn wheel_steps_add_up() {
        let steps = wheel_steps(-576.0, &HumanProfile::default(), &mut Rng::new(1));
        assert_eq!(steps.len(), 6);
        let sum: f64 = steps.iter().map(|s| s.0).sum();
        assert!((sum + 576.0).abs() < 1e-9);
    }

    #[test]
    fn profiles_are_clamped_and_aim_stays_close() {
        let p = HumanProfile {
            mouse_speed: 0.0,
            typing_wpm: f64::NAN,
            jitter: 7.0,
        }
        .sanitized();
        assert_eq!((p.mouse_speed, p.typing_wpm, p.jitter), (100.0, 55.0, 1.0));
        let mut rng = Rng::new(2);
        for _ in 0..100 {
            let (x, y) = aim((100.0, 50.0), 3.0, &p, &mut rng);
            assert!((x - 100.0).abs() <= 3.0 && (y - 50.0).abs() <= 3.0);
            let (ex, ey) = entry_point((100.0, 50.0), (800.0, 600.0), &mut rng);
            assert!((0.0..800.0).contains(&ex) && (0.0..600.0).contains(&ey));
        }
    }
}
