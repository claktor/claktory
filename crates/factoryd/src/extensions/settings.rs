//! The Settings extension: an admin agent such as Rivet reads and
//! changes the settings Claktory keeps in factory.db rather than in the
//! settings file, each through the same path as the app's request for it:
//! advanced diagnostics (SetDiagnostics), keeping this Mac awake
//! (SetKeepAwake), notifications (UpdateSettings.notification_change) and
//! each agent's Notify me (UpdateAgent.notify), the checklist's intervals
//! (UpdateSettings.checklist), how many panes `show` keeps open
//! (UpdateSettings.max_open_panes), the voice model (InstallSpeechModel,
//! RemoveSpeechModel) and the app dock's pins (PinApp). Everything else in
//! Settings lives in the settings file, which admin agents edit.
//!
//! Only admin agents get the tool, and it checks again when it runs. Where
//! the push relay sends notifications stays the person's (Settings alone).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptSection, async_tool};
use crate::config::AgentConfig;
use crate::factory::{AgentChange, Factory, Inner};
use crate::transcript::now_ms;

pub(crate) const ID: &str = "settings";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Settings",
    icon: "gauge",
    description: "Lets an admin agent read and change the settings Claktory keeps outside the settings file, as you would in Settings: advanced diagnostics, keeping this Mac awake, notifications, the checklist's intervals, panes, the voice model and the app dock.",
    default_enabled: false,
    options: &[],
};

const SETTINGS: &str = "Read or change the settings Claktory keeps outside the settings file, as the person would in Settings; everything else in Settings is in the settings file. With nothing given, says what each is now. Change a setting only when the person asks for it; each change goes through the same path as theirs in the app, and the answer says what everything is afterwards.
- diagnostics: advanced diagnostics. true turns them on (the person's devices and Claktory record what happens around connections, kept in factory.db for 14 days or 4 MB per device, to read with db_query on diagnostics). false turns them off and deletes everything kept, at once and for good: only when the person explicitly asks for that, never to tidy up.
- keep_awake: keep this Mac awake while agents work and it's plugged in (Mac only).
- notifications: which kinds notify (needs_you, alerts, replies: replies to messages sent from a phone or tablet, desktop_replies: from a computer), preview (full, who or none: what a notification shows), quiet_hours with quiet_start and quiet_end (\"22:00\", local time) and urgent_breaks_quiet. Only what you give changes.
- agent_notify: an agent's Notify me, by id or name: default (as notifications say), always or never.
- checklist: Settings → Checklist's intervals, in minutes (15 to 10080): agent_blocked (when a block on an agent is asked about), person_blocked (a wait on the person), external_retry (an outside wait checked again), reask (a dismissed question asked again), overdue (a block told to its owner's lead).
- max_open_panes: how many panes show keeps open beside a conversation, 1 to 11; 0 for the default.
- voice_model: install (downloads it: it's large, so ask the person first; the answer says its size) or remove (deletes it; voice messages waiting for it fail).
- pin_app: the app dock: an installed app's id, pinned true (at position, 0 first; left out, last) or false.
Once you've read a web page or a pipe, you can read these but not change them.";

impl Extension for Settings {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        if !crate::proposals::is_admin(agent) {
            return Vec::new();
        }
        let person = inner.config.person();
        vec![PromptSection::static_text(format!(
            "## Settings\n\nSome of Settings lives in factory.db, not the settings file: advanced diagnostics, keeping this Mac awake, notifications and each agent's Notify me, the checklist's intervals, the pane limit, the voice model and the app dock. Read and change those with the settings tool, never by editing the settings file or the database; change one only when {person} asks. Turning advanced diagnostics off deletes everything they kept: only when {person} explicitly asks.\n"
        ))]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let admin = factory.lock().config.agent(agent).is_some_and(crate::proposals::is_admin);
        if !admin {
            return Vec::new();
        }
        let flag = json!({"type": "boolean"});
        let minutes = json!({"type": "integer", "minimum": 15, "maximum": 7 * 24 * 60});
        let time = json!({"type": "string", "pattern": "^[0-2]?[0-9]:[0-5][0-9]$"});
        let parameters = json!({
            "type": "object",
            "properties": {
                "diagnostics": flag,
                "keep_awake": flag,
                "notifications": {
                    "type": "object",
                    "properties": {
                        "needs_you": flag,
                        "alerts": flag,
                        "replies": flag,
                        "desktop_replies": flag,
                        "preview": {"enum": ["full", "who", "none"]},
                        "quiet_hours": flag,
                        "quiet_start": time,
                        "quiet_end": time,
                        "urgent_breaks_quiet": flag,
                    },
                    "additionalProperties": false,
                },
                "agent_notify": {
                    "type": "object",
                    "description": "By agent id or name.",
                    "additionalProperties": {"enum": ["default", "always", "never"]},
                },
                "checklist": {
                    "type": "object",
                    "properties": {
                        "agent_blocked": minutes,
                        "person_blocked": minutes,
                        "external_retry": minutes,
                        "reask": minutes,
                        "overdue": minutes,
                    },
                    "additionalProperties": false,
                },
                "max_open_panes": {"type": "integer", "minimum": 0, "maximum": crate::config::MAX_OPEN_PANES},
                "voice_model": {"enum": ["install", "remove"]},
                "pin_app": {
                    "type": "object",
                    "properties": {"app": {"type": "string", "minLength": 1}, "pinned": flag, "position": {"type": "integer"}},
                    "required": ["app", "pinned"],
                    "additionalProperties": false,
                },
            },
            "additionalProperties": false,
        });
        vec![async_tool(factory, agent, "settings", "Settings", SETTINGS, parameters, settings)]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

pub(crate) struct Settings;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Notifications {
    needs_you: Option<bool>,
    alerts: Option<bool>,
    replies: Option<bool>,
    desktop_replies: Option<bool>,
    preview: Option<String>,
    quiet_hours: Option<bool>,
    quiet_start: Option<String>,
    quiet_end: Option<String>,
    urgent_breaks_quiet: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Checklist {
    agent_blocked: Option<u32>,
    person_blocked: Option<u32>,
    external_retry: Option<u32>,
    reask: Option<u32>,
    overdue: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    app: String,
    pinned: bool,
    #[serde(default)]
    position: Option<i32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SettingsArgs {
    diagnostics: Option<bool>,
    keep_awake: Option<bool>,
    notifications: Option<Notifications>,
    agent_notify: Option<BTreeMap<String, String>>,
    checklist: Option<Checklist>,
    max_open_panes: Option<u32>,
    voice_model: Option<String>,
    pin_app: Option<Pin>,
}

/// "22:00" as minutes after midnight.
fn minute_of_day(text: &str) -> Result<u32, String> {
    let wrong = || format!("\"{text}\" isn't a time of day such as 22:00.");
    let (hours, minutes) = text.trim().split_once(':').ok_or_else(wrong)?;
    let (hours, minutes): (u32, u32) = (hours.parse().map_err(|_| wrong())?, minutes.parse().map_err(|_| wrong())?);
    if hours > 23 || minutes > 59 {
        return Err(wrong());
    }
    Ok(hours * 60 + minutes)
}

fn clock(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn events(count: u32) -> String {
    if count == 1 { "1 event".to_owned() } else { format!("{count} events") }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// What the agent asked to change, as requests to apply in order, each
/// with what it says once it's done. Nothing is applied before every one
/// reads right.
enum Change {
    Diagnostics(bool),
    KeepAwake(bool),
    Settings(Box<v1::UpdateSettings>, String),
    /// The agent's id, how it's called (`called`) and the mode.
    Notify(String, String, v1::NotifyMode),
    Voice(bool),
    Pin(v1::PinApp),
}

fn changes(factory: &Factory, agent: &str, args: SettingsArgs) -> Result<Vec<Change>, String> {
    let mut changes = Vec::new();
    if let Some(on) = args.diagnostics {
        changes.push(Change::Diagnostics(on));
    }
    if let Some(on) = args.keep_awake {
        changes.push(Change::KeepAwake(on));
    }
    if let Some(given) = args.notifications {
        let preview = match given.preview.as_deref() {
            None => None,
            Some("full") => Some(v1::NotifyPreview::Full),
            Some("who") => Some(v1::NotifyPreview::Who),
            Some("none") => Some(v1::NotifyPreview::None),
            Some(other) => return Err(format!("preview is full, who or none, not \"{other}\".")),
        };
        let change = v1::NotificationSettingsChange {
            needs_you: given.needs_you,
            alerts: given.alerts,
            replies: given.replies,
            desktop_replies: given.desktop_replies,
            preview: preview.map(i32::from),
            quiet_hours: given.quiet_hours,
            quiet_start: given.quiet_start.as_deref().map(minute_of_day).transpose()?,
            quiet_end: given.quiet_end.as_deref().map(minute_of_day).transpose()?,
            urgent_breaks_quiet: given.urgent_breaks_quiet,
            // Both the person's alone.
            relay: None,
            urgent_from: None,
        };
        let request = v1::UpdateSettings { notification_change: Some(change), ..Default::default() };
        changes.push(Change::Settings(Box::new(request), "notifications".to_owned()));
    }
    if let Some(modes) = args.agent_notify {
        let config = factory.lock().config.clone();
        let from = config.agent(agent).ok_or("You're no longer in the settings.")?;
        let candidates: Vec<&AgentConfig> = config.agents.iter().collect();
        for (name, mode) in modes {
            let found = super::agents::find(&config, from, &candidates, &name)?.ok_or_else(|| format!("No agent is called \"{name}\"."))?;
            let mode = match mode.as_str() {
                "default" => v1::NotifyMode::Default,
                "always" => v1::NotifyMode::Always,
                "never" => v1::NotifyMode::Never,
                other => return Err(format!("{name}'s Notify me is default, always or never, not \"{other}\".")),
            };
            changes.push(Change::Notify(found.id.clone(), called(found), mode));
        }
    }
    if let Some(given) = args.checklist {
        // 0 leaves an interval as it is.
        let times = v1::ChecklistSettings {
            agent_blocked_minutes: given.agent_blocked.unwrap_or(0),
            person_blocked_minutes: given.person_blocked.unwrap_or(0),
            external_retry_minutes: given.external_retry.unwrap_or(0),
            reask_minutes: given.reask.unwrap_or(0),
            overdue_minutes: given.overdue.unwrap_or(0),
        };
        if [given.agent_blocked, given.person_blocked, given.external_retry, given.reask, given.overdue].contains(&Some(0)) {
            return Err("Each checklist interval is from 15 minutes to 7 days.".to_owned());
        }
        let request = v1::UpdateSettings { checklist: Some(times), ..Default::default() };
        changes.push(Change::Settings(Box::new(request), "the checklist's intervals".to_owned()));
    }
    if let Some(panes) = args.max_open_panes {
        let request = v1::UpdateSettings { max_open_panes: Some(panes), ..Default::default() };
        changes.push(Change::Settings(Box::new(request), "the pane limit".to_owned()));
    }
    match args.voice_model.as_deref() {
        None => {}
        Some("install") => changes.push(Change::Voice(true)),
        Some("remove") => changes.push(Change::Voice(false)),
        Some(other) => return Err(format!("voice_model is install or remove, not \"{other}\".")),
    }
    if let Some(pin) = args.pin_app {
        let position = pin.position.unwrap_or(i32::MAX);
        changes.push(Change::Pin(v1::PinApp { app_id: pin.app.trim().to_owned(), pinned: pin.pinned, position }));
    }
    Ok(changes)
}

/// Applies one change through the app's own path; what it changed.
async fn apply(factory: &Arc<Factory>, change: Change) -> Result<String, String> {
    let error = |error: crate::factory::RequestError| error.to_string();
    match change {
        Change::Diagnostics(on) => {
            let kept: u32 = crate::diagnostics::view(factory).sources.iter().map(|source| source.events).sum();
            crate::diagnostics::set(factory, &v1::SetDiagnostics { share: Some(on), ..Default::default() }).map_err(error)?;
            Ok(if on {
                "advanced diagnostics on".to_owned()
            } else {
                format!("advanced diagnostics off, and everything kept deleted ({})", events(kept))
            })
        }
        Change::KeepAwake(on) => {
            crate::keep_awake::set(factory, on).map_err(error)?;
            Ok(format!("keep this Mac awake {}", on_off(on)))
        }
        Change::Settings(request, what) => {
            factory.update_settings(*request).await.map_err(error)?;
            Ok(what)
        }
        Change::Notify(id, called, mode) => {
            factory.update_agent(&id, AgentChange { notify: Some(mode), ..Default::default() }).await.map_err(error)?;
            Ok(format!("{called}'s Notify me"))
        }
        Change::Voice(true) => {
            crate::speech::model::install(factory).map_err(error)?;
            Ok("the voice model's download started".to_owned())
        }
        Change::Voice(false) => {
            crate::speech::model::remove(factory).map_err(error)?;
            Ok("the voice model removed".to_owned())
        }
        Change::Pin(pin) => {
            if !factory.apps.available() {
                return Err("Apps aren't available in this Claktory yet.".to_owned());
            }
            factory.pin_app(&pin).map_err(error)?;
            Ok(format!("{} {}", pin.app_id, if pin.pinned { "pinned" } else { "unpinned" }))
        }
    }
}

/// The settings tool.
async fn settings(factory: Arc<Factory>, agent: String, args: SettingsArgs) -> Result<ToolResult, String> {
    let admin = factory.lock().config.agent(&agent).is_some_and(crate::proposals::is_admin);
    if !admin {
        return Err("Only an admin agent can change these settings.".to_owned());
    }
    let changes = changes(&factory, &agent, args)?;
    if !changes.is_empty() {
        // A page or a pipe it read mustn't change the person's settings
        // (as for agents_stop); reading them is fine.
        super::untainted(&factory, &agent, "change these settings", "change them in Settings. Nothing was changed.")?;
    }
    let mut done = Vec::new();
    for change in changes {
        match apply(&factory, change).await {
            Ok(what) => done.push(what),
            Err(why) if done.is_empty() => return Err(why),
            Err(why) => return Err(format!("{why} Already changed: {}.", done.join("; "))),
        }
    }
    if !done.is_empty() {
        tracing::info!(%agent, changed = done.join("; "), "an agent changed settings");
    }
    let mut text = String::new();
    if !done.is_empty() {
        let _ = writeln!(text, "Changed: {}.\n", done.join("; "));
    }
    text.push_str(&now(&factory));
    let diagnostics = crate::diagnostics::view(&factory);
    Ok(ToolResult::text(text.trim_end().to_owned(), json!({"changed": done, "diagnostics": diagnostics.share})))
}

/// Every setting the tool covers, as it is now.
/// An agent as the person knows it: its name, and its id too when that
/// reads differently ("Frontend (app-frontend)").
fn called(agent: &AgentConfig) -> String {
    match agent.name.as_str() {
        "" => agent.id.clone(),
        name if name.eq_ignore_ascii_case(&agent.id) => name.to_owned(),
        name => format!("{name} ({})", agent.id),
    }
}

fn now(factory: &Factory) -> String {
    let mut text = String::new();

    let diagnostics = crate::diagnostics::view(factory);
    let _ = write!(text, "Advanced diagnostics: {}", on_off(diagnostics.share));
    if diagnostics.share {
        if diagnostics.sources.is_empty() {
            text.push_str(", nothing kept yet");
        } else {
            text.push_str(", kept (read it with db_query on diagnostics):");
            let at = now_ms();
            for source in &diagnostics.sources {
                let name = if source.name.is_empty() { source.id.as_str() } else { source.name.as_str() };
                let areas: Vec<&str> = source.areas.iter().map(|area| area.area.as_str()).collect();
                let _ = write!(
                    text,
                    "\n  - {name} ({}): {}, {} KB, {}; last sent {} ago",
                    source.id,
                    events(source.events),
                    (source.bytes + 1023) / 1024,
                    areas.join(", "),
                    super::agents::ago(source.received_ms, at)
                );
            }
        }
    }
    text.push('\n');

    let machine = factory.lock().config.claktory_name();
    let awake = factory.keep_awake.view(&machine);
    if awake.available {
        let _ = write!(text, "Keep this Mac awake while agents work: {}", on_off(awake.on));
        if awake.laptop && !awake.answered {
            text.push_str(" (not answered yet; a laptop asks)");
        }
        text.push('\n');
    } else {
        text.push_str("Keep this Mac awake: not offered (Claktory doesn't run on a Mac)\n");
    }

    let notify = crate::notify::settings(factory);
    let preview = match notify.preview {
        crate::notify::Preview::Full => "full",
        crate::notify::Preview::Who => "who",
        crate::notify::Preview::None => "none",
    };
    let _ = writeln!(
        text,
        "Notifications: needs_you {}, alerts {}, replies {}, desktop_replies {}; preview {preview}; quiet_hours {} ({} to {}, urgent_breaks_quiet {})",
        on_off(notify.needs_you),
        on_off(notify.alerts),
        on_off(notify.replies),
        on_off(notify.desktop_replies),
        on_off(notify.quiet_hours),
        clock(notify.quiet_start),
        clock(notify.quiet_end),
        on_off(notify.urgent_breaks_quiet),
    );
    let modes: Vec<String> = notify
        .agents
        .iter()
        .filter(|(_, mode)| !mode.is_default())
        .map(|(id, mode)| {
            let called = factory.lock().config.agent(id).map_or_else(|| id.clone(), called);
            format!("{called} {}", if *mode == crate::notify::Mode::Always { "always" } else { "never" })
        })
        .collect();
    let _ = writeln!(text, "Agents' Notify me: {}", if modes.is_empty() { "all default".to_owned() } else { format!("{}; the rest default", modes.join(", ")) });

    let times = crate::extensions::checklist::Times::read(&factory.store).proto();
    let _ = writeln!(
        text,
        "Checklist (minutes): agent_blocked {}, person_blocked {}, external_retry {}, reask {}, overdue {}",
        times.agent_blocked_minutes, times.person_blocked_minutes, times.external_retry_minutes, times.reask_minutes, times.overdue_minutes
    );

    let (chosen, apps) = {
        let inner = factory.lock();
        let installed: Vec<String> = inner.config.apps.iter().map(|app| app.id.clone()).collect();
        let pinned = crate::apps::pins::view(factory, &inner.config);
        (inner.state.max_open_panes, (installed, pinned))
    };
    let _ = writeln!(
        text,
        "Panes beside a conversation: {}{}",
        crate::config::open_panes(chosen),
        if chosen == 0 { " (the default)" } else { "" }
    );

    let voice = factory.speech.view();
    let mb = |bytes: u64| bytes.div_ceil(1_000_000);
    let voice = match voice.state() {
        v1::SpeechModelState::Ready => "installed".to_owned(),
        v1::SpeechModelState::Absent | v1::SpeechModelState::Unspecified => format!("not installed ({} MB)", mb(voice.total)),
        v1::SpeechModelState::Downloading => format!("downloading, {} of {} MB", mb(voice.bytes), mb(voice.total)),
        v1::SpeechModelState::Verifying => "downloaded, being checked".to_owned(),
        v1::SpeechModelState::Failed => format!("its download failed: {}", voice.error),
        v1::SpeechModelState::Unsupported => format!("not available here: {}", voice.error),
    };
    let _ = writeln!(text, "Voice model: {voice}");

    let (installed, pinned) = apps;
    if !factory.apps.available() {
        text.push_str("App dock: apps aren't available in this Claktory yet\n");
    } else {
        let unpinned: Vec<&String> = installed.iter().filter(|app| !pinned.contains(app)).collect();
        let _ = writeln!(
            text,
            "App dock: {}{}",
            if pinned.is_empty() { "nothing pinned".to_owned() } else { format!("pinned {}", pinned.join(", ")) },
            if unpinned.is_empty() {
                String::new()
            } else {
                format!("; not pinned {}", unpinned.iter().map(|app| app.as_str()).collect::<Vec<_>>().join(", "))
            }
        );
    }
    text
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_time_of_day_reads_as_minutes_after_midnight() {
        assert_eq!(super::minute_of_day("22:00"), Ok(22 * 60));
        assert_eq!(super::minute_of_day("7:05"), Ok(7 * 60 + 5));
        assert_eq!(super::minute_of_day(" 00:00 "), Ok(0));
        assert!(super::minute_of_day("24:00").is_err());
        assert!(super::minute_of_day("7").is_err());
        assert!(super::minute_of_day("7:60").is_err());
        assert_eq!(super::clock(7 * 60 + 5), "07:05");
    }
}
