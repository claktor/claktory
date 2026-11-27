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

/// The mouse's way from `from` to `to` taking `took`: points and the wait
/// before each, on a cubic Bézier eased in and out, bent a little to one
/// side, inside the `viewport` (width, height); the last point is `to`.
pub(crate) fn path(rng: &mut Rng, from: Point, to: Point, viewport: Point, took: Duration) -> Vec<(Point, Duration)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let distance = dx.hypot(dy);
    if distance < 1.0 {
        return vec![(to, Duration::ZERO)];
    }
    let (nx, ny) = (-dy / distance, dx / distance);
    let bound = (0.12 * distance).min(30.0);
    let side = rng.truncated(0.0, 0.04 * distance, -bound, bound);
    let second = side * rng.between(0.4, 1.0);
    let p1 = (from.0 + dx / 3.0 + nx * side, from.1 + dy / 3.0 + ny * side);
    let p2 = (from.0 + 2.0 * dx / 3.0 + nx * second, from.1 + 2.0 * dy / 3.0 + ny * second);
    let total = took.as_secs_f64() * 1000.0;
    let mut points = Vec::new();
    let mut elapsed = 0.0;
    while elapsed < total {
        let gap = rng.truncated(18.0, 4.0, 10.0, 32.0).min(total - elapsed).max(1.0);
        elapsed += gap;
        let u = (elapsed / total).min(1.0);
        let s = u * u * u * (10.0 - 15.0 * u + 6.0 * u * u);
        let b = |a: f64, b: f64, c: f64, d: f64| {
            let r = 1.0 - s;
            r * r * r * a + 3.0 * r * r * s * b + 3.0 * r * s * s * c + s * s * s * d
        };
        let point = (b(from.0, p1.0, p2.0, to.0).clamp(0.0, viewport.0 - 1.0), b(from.1, p1.1, p2.1, to.1).clamp(0.0, viewport.1 - 1.0));
        points.push((point, ms(gap)));
    }
    if let Some(last) = points.last_mut() {
        last.0 = to;
    }
    points
}

/// Where in a target (left, top, width, height) the click lands: near its
/// middle, within its central 60% and 2 px from its edges, of the part
/// inside the `viewport`.
pub(crate) fn aim(rng: &mut Rng, rect: (f64, f64, f64, f64), viewport: Point) -> Point {
    let (left, top) = (rect.0.max(0.0), rect.1.max(0.0));
    let (right, bottom) = ((rect.0 + rect.2).min(viewport.0), (rect.1 + rect.3).min(viewport.1));
    let (width, height) = ((right - left).max(0.0), (bottom - top).max(0.0));
    let (cx, cy) = (left + width / 2.0, top + height / 2.0);
    let one = |rng: &mut Rng, middle: f64, size: f64| {
        let reach = (0.3 * size).min((size / 2.0 - 2.0).max(0.0));
        rng.truncated(middle, 0.12 * size, middle - reach, middle + reach)
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

/// One turn of the wheel: the pixels it scrolls and the wait before it.
pub(crate) type Pulse = (f64, Duration);

/// The bouts that scroll `pages` screens of `height` px: each 45–85% of a
/// screen, in 5–12 pulses over 450–1000 ms, eased in and out.
pub(crate) fn bouts(rng: &mut Rng, height: f64, pages: u8) -> Vec<Vec<Pulse>> {
    let mut left = height * f64::from(pages);
    let mut bouts = Vec::new();
    while left > 1.0 && bouts.len() < 16 {
        let size = (height * rng.between(0.45, 0.85)).min(left);
        left -= size;
        let count = rng.whole(5, 12) as usize;
        let span = rng.between(450.0, 1000.0);
        let weights: Vec<f64> = (0..count).map(|i| (std::f64::consts::PI * (i as f64 + 0.5) / count as f64).sin()).collect();
        let sum: f64 = weights.iter().sum();
        let pulses =
            weights.iter().enumerate().map(|(i, w)| (size * w / sum, if i == 0 { Duration::ZERO } else { ms(span / count as f64 * rng.between(0.8, 1.2)) })).collect();
        bouts.push(pulses);
    }
    bouts
}

/// After a bout, before the next.
pub(crate) fn between_bouts(rng: &mut Rng) -> Duration {
    ms(rng.between(250.0, 700.0))
}

/// The most words of a page's new text read before its next change: an
/// agent reads all of what it's given, a person scanning a page the
/// labels and snippets they need (R11.4 counts only those, typically
/// 40–120 words), which Claktory can't tell apart.
pub(crate) const READ_AT_MOST: usize = 150;

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
    fn the_mouse_arrives_inside_the_viewport_over_its_time() {
        let mut rng = Rng::seeded(3);
        let took = Duration::from_millis(600);
        let points = path(&mut rng, (10.0, 700.0), (1200.0, 20.0), (1280.0, 720.0), took);
        assert_eq!(points.last().unwrap().0, (1200.0, 20.0));
        assert!(points.len() >= 15, "{}", points.len());
        let total: Duration = points.iter().map(|(_, gap)| *gap).sum();
        assert!(total.abs_diff(took) < Duration::from_millis(5), "{total:?}");
        assert!(points.iter().all(|((x, y), _)| (0.0..1280.0).contains(x) && (0.0..720.0).contains(y)));
        assert_eq!(path(&mut rng, (5.0, 5.0), (5.2, 5.0), (100.0, 100.0), took).len(), 1, "no trip in place");
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
    fn bouts_scroll_the_screens_asked_for_in_eased_pulses() {
        let mut rng = Rng::seeded(5);
        for pages in 1..=3u8 {
            let bouts = bouts(&mut rng, 800.0, pages);
            let total: f64 = bouts.iter().flatten().map(|(delta, _)| delta).sum();
            assert!((total - 800.0 * f64::from(pages)).abs() < 1.0, "{total}");
            for bout in &bouts {
                assert!((5..=12).contains(&bout.len()));
                let size: f64 = bout.iter().map(|(delta, _)| delta).sum();
                assert!(size <= 0.85 * 800.0 + 0.1);
                let middle = bout[bout.len() / 2].0;
                assert!(middle >= bout[0].0 && middle >= bout[bout.len() - 1].0, "eased: {bout:?}");
            }
        }
    }

    #[test]
    fn reading_takes_a_persons_time_up_to_its_cap() {
        let mut rng = Rng::seeded(9);
        let person = Person { typing_wpm: 50.0, reading_wpm: 238.0 };
        let some = reading(&mut rng, &person, 100);
        assert!((Duration::from_millis(25_000 + 800)..Duration::from_millis(25_300 + 3000)).contains(&some), "{some:?}");
        let capped = reading(&mut rng, &person, 5000);
        assert!(capped < Duration::from_secs(41), "{capped:?}");
        assert_eq!(words("Sign up — it's free! 123"), 5);
    }
}
