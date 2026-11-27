//! `replay`: the person's input on Claktory's browser, replayed from a
//! script through the live view, as the app sends it. One connection
//! watches the browser (WatchAgentBrowser, every frame acknowledged), takes
//! it over when an errand drives it (TakeOverAgentBrowser; a no-op in the
//! person's turn), and sends each event of the script as its own
//! AgentBrowserInput, on time. A frame can be saved as a PNG at the end.
//!
//! A script is JSON lines, one step each (blank lines skipped):
//!
//! ```text
//! {"move": [530, 270]}                 the pointer to (x, y)
//! {"down": [590, 282]}                 a button down there ("button": 1 main, 2 middle, 3 secondary)
//! {"up": [590, 282]}                   and up
//! {"wheel": [300, 400], "dy": 50}      the wheel there, by (dx, dy) frame pixels: 50 is one notch
//! {"wait_ms": 16}                      nothing for that long
//! ```
//!
//! Points are the page's CSS pixels from its top left, at a device pixel
//! ratio of 1: the live view's fractions count the browser's whole screen,
//! so `--toolbar` (Chrome's toolbar above the page, in pixels) is added to
//! every y.

use std::path::{Path, PathBuf};
use std::time::Duration;

use factory_protocol::v1::server_message::Body;
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::Failure;
use crate::driver::Connection;
use crate::sandbox::Sandbox;

/// One step of a script.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Move { x: f64, y: f64 },
    Down { x: f64, y: f64, button: u32 },
    Up { x: f64, y: f64, button: u32 },
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    Wait(Duration),
}

/// The most steps a script may have.
const MOST_STEPS: usize = 10_000;

/// A script's steps, or what's wrong with it, by line.
pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let wrong = |why: &str| format!("Line {}: {why}: {line}", index + 1);
        let value: Value = serde_json::from_str(line).map_err(|error| wrong(&format!("not JSON ({error})")))?;
        let point = |key: &str| -> Result<Option<(f64, f64)>, String> {
            let Some(found) = value.get(key) else { return Ok(None) };
            match found.as_array().map(Vec::as_slice) {
                Some([x, y]) => match (x.as_f64(), y.as_f64()) {
                    (Some(x), Some(y)) if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 => Ok(Some((x, y))),
                    _ => Err(wrong(&format!("{key} takes [x, y], two numbers of 0 or more"))),
                },
                _ => Err(wrong(&format!("{key} takes [x, y]"))),
            }
        };
        let button = match value.get("button") {
            None => 1,
            Some(button) => match button.as_u64() {
                Some(button @ 1..=3) => button as u32,
                _ => return Err(wrong("button is 1, 2 or 3")),
            },
        };
        let delta = |key: &str| value.get(key).map_or(Some(0.0), Value::as_f64).filter(|delta| delta.is_finite());
        let step = if let Some((x, y)) = point("move")? {
            Step::Move { x, y }
        } else if let Some((x, y)) = point("down")? {
            Step::Down { x, y, button }
        } else if let Some((x, y)) = point("up")? {
            Step::Up { x, y, button }
        } else if let Some((x, y)) = point("wheel")? {
            let (Some(dx), Some(dy)) = (delta("dx"), delta("dy")) else { return Err(wrong("dx and dy are numbers")) };
            if dx == 0.0 && dy == 0.0 {
                return Err(wrong("a wheel turns: give dy (or dx)"));
            }
            Step::Wheel { x, y, dx, dy }
        } else if let Some(wait) = value.get("wait_ms") {
            let ms = wait.as_u64().filter(|ms| *ms <= 600_000).ok_or_else(|| wrong("wait_ms is whole milliseconds, 10 minutes at most"))?;
            Step::Wait(Duration::from_millis(ms))
        } else {
            return Err(wrong("a step is move, down, up, wheel or wait_ms"));
        };
        steps.push(step);
        if steps.len() > MOST_STEPS {
            return Err(format!("At most {MOST_STEPS} steps."));
        }
    }
    if !steps.iter().any(|step| !matches!(step, Step::Wait(_))) {
        return Err("The script has no input: give at least one move, down, up or wheel.".to_owned());
    }
    Ok(steps)
}

/// The AgentBrowserEvent a step sends, as JSON, its point a fraction of a
/// `screen` (width, height) with `toolbar` pixels above the page, on frame
/// `frame_seq`; None for a wait.
pub fn event(step: &Step, screen: (u32, u32), toolbar: f64, frame_seq: u64) -> Option<Value> {
    let fraction = |x: f64, y: f64| {
        let (width, height) = (f64::from(screen.0.max(1)), f64::from(screen.1.max(1)));
        ((x / width).clamp(0.0, 1.0), ((y + toolbar) / height).clamp(0.0, 1.0))
    };
    let pointer = |kind: &str, x: f64, y: f64, button: u32| {
        let (x, y) = fraction(x, y);
        json!({"pointer": {"kind": kind, "x": x, "y": y, "button": button, "frame_seq": frame_seq}})
    };
    Some(match *step {
        Step::Move { x, y } => pointer("AGENT_BROWSER_POINTER_KIND_MOVE", x, y, 0),
        Step::Down { x, y, button } => pointer("AGENT_BROWSER_POINTER_KIND_DOWN", x, y, button),
        Step::Up { x, y, button } => pointer("AGENT_BROWSER_POINTER_KIND_UP", x, y, button),
        Step::Wheel { x, y, dx, dy } => {
            let (x, y) = fraction(x, y);
            json!({"wheel": {"x": x, "y": y, "dx": dx, "dy": dy, "frame_seq": frame_seq}})
        }
        Step::Wait(_) => return None,
    })
}

/// How long to wait for the browser's first frame.
const FIRST_FRAME: Duration = Duration::from_secs(20);
/// After the last event, before the frame saved is taken.
const SETTLE: Duration = Duration::from_secs(1);
/// After that, how long a frame still coming is waited for.
const LAST_FRAME: Duration = Duration::from_secs(3);

/// The newest frame the connection was sent.
struct Frame {
    seq: u64,
    jpeg: Vec<u8>,
    screen: (u32, u32),
}

/// Takes what came, acknowledging every frame so the next can come.
async fn take(connection: &mut Connection, message: factory_protocol::v1::ServerMessage, newest: &mut Option<Frame>) -> Result<bool, Failure> {
    match message.body {
        Some(Body::AgentBrowserFrame(frame)) => {
            connection.request("AgentBrowserFrameAck", &json!({ "seq": frame.seq })).await?;
            let screen = if frame.screen_width > 0 { (frame.screen_width, frame.screen_height) } else { (frame.width, frame.height) };
            *newest = Some(Frame { seq: frame.seq, jpeg: frame.jpeg, screen });
            Ok(true)
        }
        Some(Body::AgentBrowserViewMoved(moved)) => {
            Err(Failure::failed(format!("The live view moved to {} midway: nothing more was sent.", moved.device)))
        }
        _ => Ok(false),
    }
}

/// Takes what comes until `until`, acknowledging frames; whether a frame came.
async fn until(connection: &mut Connection, until: Instant, newest: &mut Option<Frame>) -> Result<bool, Failure> {
    let mut framed = false;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(framed);
        }
        match connection.receive(left).await? {
            Some(message) => framed |= take(connection, message, newest).await?,
            None => return Ok(framed),
        }
    }
}

/// Replays `steps` on `sandbox`'s browser, `toolbar` pixels of Chrome above
/// the page; with `png`, the frame shown about a second after the last
/// event is saved there.
pub async fn run(sandbox: &Sandbox, steps: &[Step], toolbar: f64, png: Option<&Path>) -> Result<Value, Failure> {
    let mut connection = Connection::open(sandbox).await?;
    connection.request("WatchAgentBrowser", &json!({ "on": true, "max_width": 0 })).await?;
    connection.request("TakeOverAgentBrowser", &json!({})).await?;
    let mut newest = None;
    let first = Instant::now() + FIRST_FRAME;
    while newest.is_none() {
        let left = first.saturating_duration_since(Instant::now());
        let Some(message) = (if left.is_zero() { None } else { connection.receive(left).await? }) else {
            return Err(Failure::failed(format!("The browser sent no frame in {} s.", FIRST_FRAME.as_secs())));
        };
        take(&mut connection, message, &mut newest).await?;
    }
    let started = Instant::now();
    let (mut next, mut sent) = (started, 0);
    for step in steps {
        if let Step::Wait(wait) = step {
            next += *wait;
            until(&mut connection, next, &mut newest).await?;
            continue;
        }
        let frame = newest.as_ref().expect("a frame came first");
        let event = event(step, frame.screen, toolbar, frame.seq).expect("an event");
        connection.request("AgentBrowserInput", &json!({ "events": [event] })).await?;
        sent += 1;
        next = next.max(Instant::now());
    }
    let took = started.elapsed();
    let mut saved = Value::Null;
    if let Some(png) = png {
        until(&mut connection, Instant::now() + SETTLE, &mut newest).await?;
        until(&mut connection, Instant::now() + LAST_FRAME, &mut newest).await?;
        let frame = newest.as_ref().expect("a frame came first");
        save(&frame.jpeg, png)?;
        saved = json!({ "png": png, "frame_seq": frame.seq });
    }
    let screen = newest.as_ref().map(|frame| frame.screen).unwrap_or_default();
    Ok(json!({ "events": sent, "took_ms": took.as_millis() as u64, "screen": [screen.0, screen.1], "saved": saved }))
}

/// A frame's JPEG, as a PNG at `path`.
fn save(jpeg: &[u8], path: &Path) -> Result<(), Failure> {
    let image = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map_err(|error| Failure::failed(format!("The frame isn't a JPEG this can read: {error}")))?;
    image.save_with_format(path, image::ImageFormat::Png).map_err(|error| Failure::failed(format!("Can't write {}: {error}", path.display())))
}

/// `replay NAME SCRIPT [--toolbar PX] [--png FILE]`'s script, read.
pub fn read(script: &str) -> Result<Vec<Step>, Failure> {
    let text = std::fs::read_to_string(script).map_err(|error| Failure::usage(format!("Can't read {script}: {error}")))?;
    parse(&text).map_err(Failure::usage)
}

/// Where `--png` writes, checked before anything is sent.
pub fn png_path(path: Option<&str>) -> Result<Option<PathBuf>, Failure> {
    let Some(path) = path else { return Ok(None) };
    let path = PathBuf::from(path);
    if path.extension().and_then(|extension| extension.to_str()).is_none_or(|extension| !extension.eq_ignore_ascii_case("png")) {
        return Err(Failure::usage("--png names a .png file"));
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_reads_as_its_steps_and_refuses_what_it_cant_send() {
        let script = r#"
            {"move": [530, 270]}
            {"wait_ms": 16}
            {"down": [590, 282]}
            {"up": [590, 282], "button": 3}
            {"wheel": [300, 400], "dy": 50}
        "#;
        assert_eq!(
            parse(script).unwrap(),
            [
                Step::Move { x: 530.0, y: 270.0 },
                Step::Wait(Duration::from_millis(16)),
                Step::Down { x: 590.0, y: 282.0, button: 1 },
                Step::Up { x: 590.0, y: 282.0, button: 3 },
                Step::Wheel { x: 300.0, y: 400.0, dx: 0.0, dy: 50.0 },
            ]
        );
        for wrong in [
            r#"{"move": [1]}"#,
            r#"{"move": [-1, 2]}"#,
            r#"{"down": [1, 2], "button": 4}"#,
            r#"{"wheel": [1, 2]}"#,
            r#"{"wait_ms": -5}"#,
            r#"{"jump": [1, 2]}"#,
            r#"{"wait_ms": 10}"#,
            "move 1 2",
        ] {
            assert!(parse(wrong).is_err(), "{wrong}");
        }
        assert!(parse("{\"move\": [1, 2]}\n{\"oops\": 1}").unwrap_err().starts_with("Line 2:"));
    }

    #[test]
    fn a_point_on_the_page_is_its_fraction_of_the_whole_screen_below_the_toolbar() {
        let event = |step: &Step| event(step, (1280, 800), 80.0, 7).unwrap();
        let moved = event(&Step::Move { x: 640.0, y: 320.0 });
        assert_eq!(moved["pointer"]["kind"], "AGENT_BROWSER_POINTER_KIND_MOVE");
        assert_eq!((moved["pointer"]["x"].as_f64(), moved["pointer"]["y"].as_f64()), (Some(0.5), Some(0.5)));
        assert_eq!((moved["pointer"]["button"].as_u64(), moved["pointer"]["frame_seq"].as_u64()), (Some(0), Some(7)));
        let pressed = event(&Step::Down { x: 0.0, y: 0.0, button: 1 });
        assert_eq!((pressed["pointer"]["y"].as_f64(), pressed["pointer"]["button"].as_u64()), (Some(0.1), Some(1)));
        let wheel = event(&Step::Wheel { x: 1280.0, y: 2000.0, dx: 0.0, dy: -50.0 });
        assert_eq!((wheel["wheel"]["x"].as_f64(), wheel["wheel"]["y"].as_f64(), wheel["wheel"]["dy"].as_f64()), (Some(1.0), Some(1.0), Some(-50.0)));
        assert!(super::event(&Step::Wait(Duration::from_millis(5)), (1280, 800), 80.0, 7).is_none());
    }

    #[test]
    fn the_cursor_probes_script_presses_go_twice_and_turns_single_notches() {
        let steps = parse(include_str!("../replays/cursor-probe.jsonl")).unwrap();
        let wheels: Vec<f64> = steps.iter().filter_map(|step| if let Step::Wheel { dy, .. } = step { Some(*dy) } else { None }).collect();
        assert_eq!(wheels, [50.0; 6]);
        let go = |x: f64, y: f64| (520.0..660.0).contains(&x) && (260.0..304.0).contains(&y);
        assert_eq!(steps.iter().filter(|step| matches!(step, Step::Down { x, y, .. } if go(*x, *y))).count(), 2);
        assert!(!steps.iter().any(|step| matches!(step, Step::Down { x, .. } | Step::Up { x, .. } if *x >= 700.0)), "never Reset");
    }

    #[test]
    fn each_event_is_one_the_protocol_takes() {
        let steps = [Step::Move { x: 1.0, y: 2.0 }, Step::Up { x: 1.0, y: 2.0, button: 2 }, Step::Wheel { x: 1.0, y: 2.0, dx: 3.0, dy: 50.0 }];
        for step in &steps {
            let body = json!({ "events": [event(step, (1280, 800), 0.0, 3).unwrap()] });
            assert!(crate::driver::encode("AgentBrowserInput", &body, 1).is_ok(), "{body}");
        }
    }
}
