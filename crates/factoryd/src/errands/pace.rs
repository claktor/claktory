//! An errand's agent drives the browser at a person's pace (#530,
//! agent-browser-r11.md R11.4): a mouse that travels, hovers and holds, keys
//! typed one at a time at a typist's rate, a wheel turned in bouts, and time
//! to read what's new before the next change. The numbers are R11.4's
//! engineering defaults, not a measured person: synthetic, the same for
//! every errand's model, never learnt from anyone's input. A person's rates
//! are drawn once per errand, each act varies around them.

use std::time::Duration;

/// A small, fast random source (SplitMix64): seeded from the system for an
/// errand, or fixed for a test.
#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn seeded(seed: u64) -> Rng {
        Rng(seed)
    }

    pub(crate) fn fresh() -> Rng {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the system's random source");
        Rng(u64::from_le_bytes(bytes))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1).
    pub(crate) fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    pub(crate) fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    /// A whole number from `low` to `high`, both included.
    pub(crate) fn whole(&mut self, low: u32, high: u32) -> u32 {
        low + (self.next() % u64::from(high - low + 1)) as u32
    }

    /// Standard normal (Box-Muller).
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.unit(), self.unit());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    /// TN(mean, sd; low, high): drawn again when out of bounds, clamped
    /// after a few tries.
    pub(crate) fn truncated(&mut self, mean: f64, sd: f64, low: f64, high: f64) -> f64 {
        for _ in 0..16 {
            let x = mean + sd * self.normal();
            if (low..=high).contains(&x) {
                return x;
            }
        }
        mean.clamp(low, high)
    }

    /// LNmed(median, s): a log-normal with this median and log-space sd.
    pub(crate) fn log_normal(&mut self, median: f64, s: f64) -> f64 {
        median * (s * self.normal()).exp()
    }
}

fn ms(value: f64) -> Duration {
    Duration::from_micros((value.max(0.0) * 1000.0) as u64)
}

/// The person an errand's agent drives as: rates drawn once per errand.
#[derive(Clone, Debug)]
pub(crate) struct Person {
    /// Typing, words a minute (five characters a word).
    pub typing_wpm: f64,
    /// Reading, English-equivalent words a minute.
    pub reading_wpm: f64,
}

impl Person {
    pub(crate) fn drawn(rng: &mut Rng) -> Person {
        Person { typing_wpm: rng.truncated(50.0, 10.0, 30.0, 75.0), reading_wpm: rng.truncated(238.0, 35.0, 175.0, 300.0) }
    }
}

/// A point in the page's viewport, CSS pixels.
pub(crate) type Point = (f64, f64);

/// How long the mouse takes to reach a target `width` wide `distance`
/// away (Fitts's shape, R11.4's coefficients).
pub(crate) fn travel(rng: &mut Rng, distance: f64, width: f64) -> Duration {
    let width = width.max(8.0);
    let t = (160.0 + 120.0 * (1.0 + distance / width).log2()) * rng.log_normal(1.0, 0.18);
    ms(t.clamp(220.0, 1400.0))
}

/// The whole CSS pixel a mouse at a device pixel ratio of 1 reports for
/// `point`, inside the `viewport`.
pub(crate) fn pixel(point: Point, viewport: Point) -> Point {
    (point.0.round().clamp(0.0, (viewport.0 - 1.0).floor().max(0.0)), point.1.round().clamp(0.0, (viewport.1 - 1.0).floor().max(0.0)))
}

/// The share of a sub-movement's way covered when it falls short, as a
/// primary sub-movement does (Meyer et al.'s stochastic optimized
/// sub-movements).
const PRIMARY_COVERS: (f64, f64) = (0.92, 0.98);
/// How far past the target an overshoot carries, as a share of the way.
const OVERSHOOT_COVERS: (f64, f64) = (1.03, 1.08);
/// Overshoots happen only on ways longer than this, in CSS pixels.
const OVERSHOOT_PAST: f64 = 500.0;
/// How often an overshoot happens on such a way.
const OVERSHOOT_CHANCE: (f64, f64) = (0.15, 0.25);
/// The scatter where a primary sub-movement ends: its standard deviation
/// as a share of the way, at most half the target's width.
const SCATTER: f64 = 0.05;
/// An end farther than this share of the target's width from the point
/// aimed at missed it, and a correction follows.
const ON_TARGET: f64 = 0.3;
/// A sub-movement's log-normal speed: the log-time spread σ, whose range
/// puts the speed's peak 30–40% into it (Plamondon's kinematic theory).
const SIGMA: (f64, f64) = (0.12, 0.24);
/// The sub-movement spans σ times this either side of its log-time middle.
const SPAN: f64 = 2.5;
/// The share of the time a primary sub-movement takes when corrections
/// follow it.
const PRIMARY_TAKES: (f64, f64) = (0.62, 0.78);
/// The hand's tremor while it moves 100 px or more, CSS pixels; less on
/// a shorter way.
const TREMOR: (f64, f64) = (0.3, 0.6);

/// One sub-movement of the mouse: from `from` to `end`, `took` ms from
/// `begins` into the way, its speed a log-normal bump (a fast rise and a
/// long tail), bent `bend` px to one side at most.
#[derive(Clone, Debug, PartialEq)]
struct Stroke {
    from: Point,
    end: Point,
    begins: f64,
    took: f64,
    sigma: f64,
    bend: f64,
}

impl Stroke {
    /// How far along it is, 0 to 1, `t` ms into it.
    fn along(&self, t: f64) -> f64 {
        let r = (t / self.took).clamp(0.0, 1.0);
        let (low, high) = ((-SPAN * self.sigma).exp(), (SPAN * self.sigma).exp());
        let z = (low + r * (high - low)).ln() / self.sigma;
        ((normal_below(z) - normal_below(-SPAN)) / (normal_below(SPAN) - normal_below(-SPAN))).clamp(0.0, 1.0)
    }

    /// Where it is `t` ms into it, and how far along.
    fn at(&self, t: f64) -> (Point, f64) {
        let p = self.along(t);
        let (dx, dy) = (self.end.0 - self.from.0, self.end.1 - self.from.1);
        let length = dx.hypot(dy).max(1e-9);
        let side = self.bend * 6.75 * p * (1.0 - p) * (1.0 - p);
        ((self.from.0 + dx * p - dy / length * side, self.from.1 + dy * p + dx / length * side), p)
    }
}

/// The standard normal's cumulative distribution (Abramowitz and Stegun
/// 7.1.26, within 1.5e-7).
fn normal_below(z: f64) -> f64 {
    let x = z.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-x * x).exp();
    0.5 * (1.0 + if z < 0.0 { -erf } else { erf })
}

/// The sub-movements that take the mouse from `from` to `to`, a target
/// `width` wide, in `took` ms: a primary one that falls short (or, on a
/// long way, now and then overshoots) with some scatter; when that misses
/// the target, one or two short corrections, the last ending at `to`.
fn strokes(rng: &mut Rng, from: Point, to: Point, width: f64, took: f64) -> Vec<Stroke> {
    let width = width.max(8.0);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let distance = dx.hypot(dy);
    let bound = (0.12 * distance).min(30.0);
    let bend = rng.truncated(0.0, 0.04 * distance, -bound, bound);
    let sigma = |rng: &mut Rng| rng.between(SIGMA.0, SIGMA.1);
    let overshoots = distance > OVERSHOOT_PAST && rng.unit() < rng.between(OVERSHOOT_CHANCE.0, OVERSHOOT_CHANCE.1);
    let covers = if overshoots { rng.between(OVERSHOOT_COVERS.0, OVERSHOOT_COVERS.1) } else { rng.between(PRIMARY_COVERS.0, PRIMARY_COVERS.1) };
    let scatter = (SCATTER * distance).min(width / 2.0);
    let end = (from.0 + dx * covers + scatter * rng.normal(), from.1 + dy * covers + scatter * rng.normal());
    let missed = (end.0 - to.0).hypot(end.1 - to.1);
    if missed <= ON_TARGET * width {
        return vec![Stroke { from, end: to, begins: 0.0, took, sigma: sigma(rng), bend }];
    }
    let primary = took * rng.between(PRIMARY_TAKES.0, PRIMARY_TAKES.1);
    let mut strokes = vec![Stroke { from, end, begins: 0.0, took: primary, sigma: sigma(rng), bend }];
    let mut ends = Vec::new();
    if missed > width && rng.unit() < 0.5 {
        let off = 0.15 * missed;
        ends.push((to.0 + off * rng.normal(), to.1 + off * rng.normal()));
    }
    ends.push(to);
    let left = took - primary;
    let each = left / ends.len() as f64;
    for end in ends {
        let last = strokes.last().expect("the primary");
        let rest = (each * rng.between(0.2, 0.35)).min(110.0);
        let begins = last.begins + last.took + rest;
        let takes = (each - rest).min(took - begins).max(1.0);
        strokes.push(Stroke { from: last.end, end, begins, took: takes, sigma: sigma(rng), bend: 0.0 });
    }
    strokes
}

/// The mouse's way from `from` to `to`, a target `width` wide, taking
/// `took`: the whole pixels it reports, each with the wait before it, about
/// every 8 ms as a USB mouse polled at 125 Hz, none repeated, inside the
/// `viewport` (width, height); the last is `to`. Its sub-movements (see
/// `strokes`) each speed up fast and slow down long, bent a little, with a
/// hand's tremor. None when the mouse is there already.
pub(crate) fn path(rng: &mut Rng, from: Point, to: Point, width: f64, viewport: Point, took: Duration) -> Vec<(Point, Duration)> {
    let (from, to) = (pixel(from, viewport), pixel(to, viewport));
    if (to.0 - from.0).hypot(to.1 - from.1) < 1.0 {
        return Vec::new();
    }
    let total = took.as_secs_f64() * 1000.0;
    let strokes = strokes(rng, from, to, width, total);
    let tremor = rng.between(TREMOR.0, TREMOR.1) * ((to.0 - from.0).hypot(to.1 - from.1) / 100.0).min(1.0);
    let mut shake = (0.0, 0.0);
    let mut points: Vec<(Point, Duration)> = Vec::new();
    let (mut elapsed, mut waited, mut at) = (0.0, 0.0, from);
    while elapsed < total {
        let gap = rng.truncated(8.0, 1.0, 6.0, 10.0).min(total - elapsed).max(1.0);
        elapsed += gap;
        waited += gap;
        shake = (0.85 * shake.0 + 0.53 * tremor * rng.normal(), 0.85 * shake.1 + 0.53 * tremor * rng.normal());
        let point = if elapsed >= total {
            to
        } else {
            let stroke = strokes.iter().rev().find(|stroke| stroke.begins <= elapsed).expect("the primary begins at 0");
            let (place, p) = stroke.at(elapsed - stroke.begins);
            let moving = (std::f64::consts::PI * p).sin();
            pixel((place.0 + shake.0 * moving, place.1 + shake.1 * moving), viewport)
        };
        if point != at {
            points.push((point, ms(waited)));
            (waited, at) = (0.0, point);
        }
    }
    match points.last_mut() {
        Some(last) if last.0 == to => last.1 += ms(waited),
        _ => points.push((to, ms(waited))),
    }
    points
}

/// A small move of the mouse while a person reads: `after` into the
/// pause, `by` (x, y) CSS pixels away, taking `took`.
#[derive(Clone, Debug)]
pub(crate) struct Drift {
    pub after: Duration,
    pub by: Point,
    pub took: Duration,
}

/// Whether, in a reading pause with `left` still to go, the hand drifts the
/// mouse once: in 30–50% of pauses longer than 4 s, 20–120 px, slowly.
pub(crate) fn drift(rng: &mut Rng, left: Duration) -> Option<Drift> {
    if left <= Duration::from_secs(4) || rng.unit() >= rng.between(0.3, 0.5) {
        return None;
    }
    let took = rng.between(400.0, 900.0);
    let after = rng.between(0.15, 0.6) * (left.as_secs_f64() * 1000.0 - took);
    let (reach, angle) = (rng.between(20.0, 120.0), rng.between(0.0, std::f64::consts::TAU));
    Some(Drift { after: ms(after), by: (reach * angle.cos(), reach * angle.sin()), took: ms(took) })
}

/// Where in a target (left, top, width, height) the click lands: a whole
/// pixel near its middle, within its central 60% and 2 px from its edges,
/// of the part inside the `viewport`.
pub(crate) fn aim(rng: &mut Rng, rect: (f64, f64, f64, f64), viewport: Point) -> Point {
    let (left, top) = (rect.0.max(0.0), rect.1.max(0.0));
    let (right, bottom) = ((rect.0 + rect.2).min(viewport.0), (rect.1 + rect.3).min(viewport.1));
    let (width, height) = ((right - left).max(0.0), (bottom - top).max(0.0));
    let (cx, cy) = (left + width / 2.0, top + height / 2.0);
    let one = |rng: &mut Rng, middle: f64, size: f64| {
        let reach = (0.3 * size).min((size / 2.0 - 2.0).max(0.0));
        let at = rng.truncated(middle, 0.12 * size, middle - reach, middle + reach).round();
        let (low, high) = ((middle - reach).ceil(), (middle + reach).floor());
        if low <= high { at.clamp(low, high) } else { at }
    };
    (one(rng, cx, width), one(rng, cy, height))
}

/// Resting over the target before pressing.
pub(crate) fn hover(rng: &mut Rng) -> Duration {
    ms(rng.log_normal(180.0, 0.4).clamp(80.0, 600.0))
}

/// The button held down.
pub(crate) fn hold(rng: &mut Rng) -> Duration {
    ms(rng.truncated(85.0, 25.0, 40.0, 180.0))
}

/// After a click, before the next thing.
pub(crate) fn settle(rng: &mut Rng) -> Duration {
    ms(rng.log_normal(650.0, 0.4).clamp(300.0, 1800.0))
}

/// A key held down.
pub(crate) fn key_hold(rng: &mut Rng) -> Duration {
    ms(rng.truncated(70.0, 18.0, 35.0, 120.0))
}

/// The typist of one fill: when each key goes down after the one before.
pub(crate) struct Typist {
    mean: f64,
    until_pause: u32,
}

impl Typist {
    pub(crate) fn new(person: &Person, rng: &mut Rng) -> Typist {
        Typist { mean: 60_000.0 / (5.0 * person.typing_wpm), until_pause: rng.whole(6, 15) }
    }

    /// From one key's going down to the next's, after typing `typed`, whose
    /// key was held `held`: never sooner than 8 ms after its release, with
    /// a pause to plan now and then, and a longer one after punctuation.
    pub(crate) fn gap(&mut self, rng: &mut Rng, typed: &str, held: Duration) -> Duration {
        let sigma: f64 = 0.35;
        let mu = self.mean.ln() - sigma * sigma / 2.0;
        let mut gap = (mu + sigma * rng.normal()).exp().clamp(80.0, 650.0);
        self.until_pause = self.until_pause.saturating_sub(1);
        if self.until_pause == 0 {
            gap += rng.truncated(250.0, 90.0, 100.0, 600.0);
            self.until_pause = rng.whole(6, 15);
        }
        if typed.chars().any(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?')) {
            gap += rng.truncated(450.0, 140.0, 180.0, 900.0);
        }
        ms(gap).max(held + Duration::from_millis(8))
    }
}

/// About how long typing `text` takes `person`: what a fill is planned
/// to take, before it starts.
pub(crate) fn typing_time(person: &Person, text: &str) -> Duration {
    let count = text.chars().count() as f64;
    let punctuation = text.chars().filter(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?')).count() as f64;
    ms(count * 60_000.0 / (5.0 * person.typing_wpm) + count / 10.5 * 250.0 + punctuation * 450.0)
}

/// The CSS pixels one notch of a mouse's wheel scrolls in Chrome on Linux
/// (`MouseWheelEvent::kWheelDelta` there), as each wheel event sent says.
pub(crate) const NOTCH: f64 = 120.0;

/// The bouts that turn the wheel `pages` screens of `height` px: the wait
/// before each notch, a bout 2–8 notches 25–90 ms apart, as many notches
/// as come nearest to the screens asked for.
pub(crate) fn bouts(rng: &mut Rng, height: f64, pages: u8) -> Vec<Vec<Duration>> {
    let mut left = ((height * f64::from(pages) / NOTCH).round() as u32).clamp(1, 256);
    let mut bouts = Vec::new();
    while left > 0 {
        let count = rng.whole(2, 8).min(left);
        left -= count;
        bouts.push((0..count).map(|i| if i == 0 { Duration::ZERO } else { ms(rng.log_normal(45.0, 0.35).clamp(25.0, 90.0)) }).collect());
    }
    bouts
}

/// After a bout, before the next.
pub(crate) fn between_bouts(rng: &mut Rng) -> Duration {
    ms(rng.between(250.0, 700.0))
}

/// The most words of a page's new text read before its next change: an
/// agent reads all of what it's given, a person about to click glances at
/// the few labels around what they click, which Claktory can't tell apart.
/// So a page's change waits at most about 12 s after a read (25 words at the
/// slowest reading rate, and the longest look), never the minute a whole
/// page would take.
pub(crate) const READ_AT_MOST: usize = 25;

/// The time a person takes to look over `words` new words of a page.
pub(crate) fn reading(rng: &mut Rng, person: &Person, words: usize) -> Duration {
    let orientation = rng.truncated(1.5, 0.4, 0.8, 3.0);
    Duration::from_secs_f64(orientation + 60.0 * words.min(READ_AT_MOST) as f64 / person.reading_wpm)
}

/// The words in `text`.
pub(crate) fn words(text: &str) -> usize {
    text.split_whitespace().filter(|word| word.chars().any(char::is_alphanumeric)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_draw_stays_in_its_bounds() {
        let mut rng = Rng::seeded(7);
        for _ in 0..2000 {
            let person = Person::drawn(&mut rng);
            assert!((30.0..=75.0).contains(&person.typing_wpm) && (175.0..=300.0).contains(&person.reading_wpm));
            let d = rng.between(0.0, 2000.0);
            let w = rng.between(1.0, 400.0);
            let t = travel(&mut rng, d, w);
            assert!((Duration::from_millis(220)..=Duration::from_millis(1400)).contains(&t), "{t:?}");
            assert!((Duration::from_millis(80)..=Duration::from_millis(600)).contains(&hover(&mut rng)));
            assert!((Duration::from_millis(40)..=Duration::from_millis(180)).contains(&hold(&mut rng)));
            assert!((Duration::from_millis(300)..=Duration::from_millis(1800)).contains(&settle(&mut rng)));
            let (x, y) = aim(&mut rng, (100.0, 50.0, 80.0, 30.0), (1280.0, 720.0));
            assert!((100.0 + 80.0 * 0.2..=100.0 + 80.0 * 0.8).contains(&x) && (52.0..=78.0).contains(&y), "{x},{y}");
        }
    }

    #[test]
    fn a_farther_smaller_target_takes_longer_on_average() {
        let mut rng = Rng::seeded(1);
        let mean = |rng: &mut Rng, d: f64, w: f64| (0..500).map(|_| travel(rng, d, w).as_secs_f64()).sum::<f64>() / 500.0;
        assert!(mean(&mut rng, 900.0, 20.0) > mean(&mut rng, 100.0, 200.0));
    }

    #[test]
    fn the_mouse_arrives_inside_the_viewport_over_its_time_in_whole_pixels_at_a_usb_mouses_rate() {
        let mut rng = Rng::seeded(3);
        let took = Duration::from_millis(600);
        let points = path(&mut rng, (10.0, 700.0), (1200.0, 20.0), 40.0, (1280.0, 720.0), took);
        assert_eq!(points.last().unwrap().0, (1200.0, 20.0));
        assert!(points.len() >= 50, "{}", points.len());
        let total: Duration = points.iter().map(|(_, gap)| *gap).sum();
        assert!(total.abs_diff(took) < Duration::from_millis(1), "{total:?}");
        assert!(points.iter().all(|((x, y), _)| (0.0..1280.0).contains(x) && (0.0..720.0).contains(y)));
        assert!(points.iter().all(|((x, y), _)| x.fract() == 0.0 && y.fract() == 0.0), "whole pixels");
        assert!(points.windows(2).all(|pair| pair[0].0 != pair[1].0), "no report without a move");
        let fast = points.iter().filter(|(_, gap)| *gap <= Duration::from_millis(10)).count();
        assert!(fast * 10 >= points.len() * 7, "mostly 125 Hz: {fast} of {}", points.len());
        assert!(path(&mut rng, (5.0, 5.0), (5.2, 5.0), 40.0, (100.0, 100.0), took).is_empty(), "no trip in place");
        let short = path(&mut rng, (5.0, 5.0), (8.4, 5.0), 40.0, (100.0, 100.0), took);
        assert_eq!(short.last().unwrap().0, (8.0, 5.0));
        assert!(short.len() <= 3, "{short:?}");
        let again = |seed| path(&mut Rng::seeded(seed), (10.0, 700.0), (1200.0, 20.0), 12.0, (1280.0, 720.0), took);
        assert_eq!(again(4), again(4), "the same seed, the same way");
    }

    #[test]
    fn a_movement_speeds_up_fast_and_slows_down_long() {
        let mut rng = Rng::seeded(8);
        let mut peaks = Vec::new();
        for _ in 0..300 {
            let took = Duration::from_millis(700);
            let points = path(&mut rng, (100.0, 400.0), (550.0, 380.0), 400.0, (1280.0, 720.0), took);
            let (mut t, mut at, mut speeds) = (0.0, (100.0, 400.0), Vec::new());
            for ((x, y), gap) in &points {
                let gap = gap.as_secs_f64() * 1000.0;
                t += gap;
                speeds.push((t, (x - at.0).hypot(y - at.1) / gap));
                at = (*x, *y);
            }
            let smooth: Vec<(f64, f64)> =
                speeds.windows(5).map(|five| (five[2].0, five.iter().map(|(_, speed)| speed).sum::<f64>() / 5.0)).collect();
            let peak = smooth.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
            peaks.push(peak / 700.0);
        }
        let mean = peaks.iter().sum::<f64>() / peaks.len() as f64;
        assert!((0.28..=0.45).contains(&mean), "the speed peaks {mean} of the way through");
        assert!(peaks.iter().filter(|peak| **peak < 0.5).count() >= 270, "{peaks:?}");
    }

    #[test]
    fn corrections_follow_only_a_miss_and_end_on_the_target() {
        let mut rng = Rng::seeded(12);
        let (from, to) = ((50.0, 50.0), (850.0, 450.0));
        let (mut corrected, mut overshot) = (0, 0);
        for _ in 0..2000 {
            let wide = strokes(&mut rng, from, (350.0, 250.0), 400.0, 600.0);
            assert_eq!(wide.len(), 1, "a wide, near target is never missed: {wide:?}");
            let small = strokes(&mut rng, from, to, 10.0, 800.0);
            assert_eq!(small.last().unwrap().end, to);
            let last = small.last().unwrap();
            assert!((last.begins + last.took - 800.0).abs() < 1e-6, "{small:?}");
            if small.len() > 1 {
                corrected += 1;
                let end = small[0].end;
                assert!((end.0 - to.0).hypot(end.1 - to.1) > ON_TARGET * 10.0, "{small:?}");
                assert!(small.len() <= 3);
                assert!(small.windows(2).all(|pair| pair[1].begins >= pair[0].begins + pair[0].took && pair[1].from == pair[0].end));
                if (end.0 - from.0).hypot(end.1 - from.1) > (to.0 - from.0).hypot(to.1 - from.1) {
                    overshot += 1;
                }
            }
        }
        assert!(corrected > 1500, "a small far target is mostly missed first: {corrected}");
        assert!((200..=500).contains(&overshot), "now and then past it: {overshot}");
    }

    #[test]
    fn the_mouse_drifts_in_some_long_reading_pauses_only() {
        let mut rng = Rng::seeded(21);
        assert!((0..500).all(|_| drift(&mut rng, Duration::from_secs(4)).is_none()));
        let left = Duration::from_secs(12);
        let drifts: Vec<Drift> = (0..2000).filter_map(|_| drift(&mut rng, left)).collect();
        assert!((500..=1000).contains(&drifts.len()), "{}", drifts.len());
        for drift in &drifts {
            assert!(drift.after + drift.took < left, "{drift:?}");
            assert!((20.0..=120.0).contains(&drift.by.0.hypot(drift.by.1)), "{drift:?}");
            assert!((Duration::from_millis(400)..=Duration::from_millis(900)).contains(&drift.took), "{drift:?}");
        }
    }

    #[test]
    fn typing_runs_at_the_typists_rate_with_pauses() {
        let mut rng = Rng::seeded(11);
        let person = Person { typing_wpm: 50.0, reading_wpm: 238.0 };
        let mut typist = Typist::new(&person, &mut rng);
        let text = "Copium Labs makes tools for people who build things, and ships them weekly";
        let mut total = Duration::ZERO;
        for c in text.chars() {
            let held = key_hold(&mut rng);
            let gap = typist.gap(&mut rng, &c.to_string(), held);
            assert!(gap >= held + Duration::from_millis(8));
            total += gap;
        }
        // 75 characters at 50 wpm is 18 s before pauses; with them, a little more.
        assert!((Duration::from_secs(14)..Duration::from_secs(40)).contains(&total), "{total:?}");
    }

    #[test]
    fn the_wheel_turns_in_whole_notches_near_the_screens_asked_for() {
        let mut rng = Rng::seeded(5);
        for pages in 1..=3u8 {
            let bouts = bouts(&mut rng, 1000.0, pages);
            let notches = bouts.iter().flatten().count();
            assert_eq!(notches, [8, 17, 25][usize::from(pages) - 1]);
            for bout in &bouts {
                assert!((1..=8).contains(&bout.len()), "{bout:?}");
                assert_eq!(bout[0], Duration::ZERO);
                assert!(bout[1..].iter().all(|gap| (Duration::from_millis(25)..=Duration::from_millis(90)).contains(gap)), "{bout:?}");
            }
            assert!(bouts[..bouts.len() - 1].iter().all(|bout| bout.len() >= 2), "only the last bout is cut short");
        }
        assert_eq!(bouts(&mut rng, 30.0, 1).concat().len(), 1, "a short screen still turns one notch");
    }

    #[test]
    fn reading_takes_a_persons_time_up_to_its_cap() {
        let mut rng = Rng::seeded(9);
        let person = Person { typing_wpm: 50.0, reading_wpm: 238.0 };
        let some = reading(&mut rng, &person, 20);
        assert!((Duration::from_millis(5_040 + 800)..Duration::from_millis(5_050 + 3000)).contains(&some), "{some:?}");
        let slowest = Person { typing_wpm: 50.0, reading_wpm: 175.0 };
        for words in [25, 150, 5000] {
            let capped = reading(&mut rng, &slowest, words);
            assert!(capped <= Duration::from_millis(8_572 + 3000), "a page's worth waits about 12 s at most: {capped:?}");
        }
        assert_eq!(words("Sign up — it's free! 123"), 5);
    }
}
