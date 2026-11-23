//! Notifications (#273; the plan is notifications.md in Research): what's
//! worth telling the person when they aren't looking, decided here, the same
//! for every device (tenet 7).
//!
//! Three kinds notify: a new Needs-you item (a seq added to an agent's,
//! pipe's or chat's waiting seqs), an agent's alert (`notify_person`, in
//! `tool`) and an agent's reply to a message the person sent from a phone or
//! tablet (from a computer too, when they ask for it). Needs-you items and
//! replies are found in one place, `observe`, which sees every Workspace as
//! it's broadcast: anything that waits for the person shows there, so a new
//! kind of Needs-you item notifies without hooking where it's made.
//!
//! What passes the person's settings (kinds, the agent's "Notify me", quiet
//! hours) and the hourly cap goes to every connected app as a `Notify`, and
//! (`push`) sealed to each device registered for pushes that isn't
//! connected. Nothing notifies while the person is using Claktory on any
//! device (SetActive), but an alert, which shows nowhere else; nothing is
//! pushed for two minutes after.
//!
//! An agent that has read a web page (or is quarantined for a pipe) never puts its
//! own words on the person's lock screen (threat model, Fugo's change B):
//! its notifications say only "<agent> has something for you". A pipe's
//! held messages never carry what the other side wrote.

pub(crate) mod push;
pub(crate) mod tool;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use factory_protocol::v1::{self, NotifyKind as Kind, entry::Kind as EntryKind, server_message::Body};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Invalid};
use crate::factory::{Factory, Inner, RequestError};
use crate::transcript::now_ms;
use rusqlite::OptionalExtension;

/// How many notifications one agent (or pipe, or chat) sends in an hour.
/// Past it they're counted, and one more says how many.
pub(crate) const PER_HOUR: usize = 6;
const HOUR_MS: i64 = 3_600_000;
/// For this long after the person was last using Claktory on a device,
/// nothing is pushed to the others: they're probably still at it.
pub(crate) const ACTIVE_GRACE_MS: i64 = 120_000;
/// The longest a notification's body gets, in characters.
pub(crate) const MAX_BODY: usize = 300;
/// What a notification says when it may say nothing more.
pub(crate) const GENERIC: &str = "Claktory has something for you";
/// How soon pushes the relay couldn't take are tried again.
const RETRY: Duration = Duration::from_secs(30);

// ---- Settings ----

/// When Claktory notifies the person (see v1::NotificationSettings), kept
/// in factory.db (`SCHEMA`), not config.json: a backend rolled back to
/// refuses a config.json with a key it doesn't know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NotificationsConfig {
    pub needs_you: bool,
    pub alerts: bool,
    /// Replies to messages sent from a phone or tablet.
    pub replies: bool,
    /// Replies to messages sent from a computer.
    pub desktop_replies: bool,
    pub preview: Preview,
    pub quiet_hours: bool,
    /// Minutes after midnight, local time.
    pub quiet_start: u32,
    pub quiet_end: u32,
    pub urgent_breaks_quiet: bool,
    /// The push relay's address; empty for Claktory's own (`push::RELAY`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub relay: String,
    /// Each agent's "Notify me" (v1::Agent.notify), by agent id, when it
    /// isn't Default.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, Mode>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        NotificationsConfig {
            needs_you: true,
            alerts: true,
            replies: true,
            desktop_replies: false,
            preview: Preview::Full,
            quiet_hours: false,
            quiet_start: 22 * 60,
            quiet_end: 7 * 60,
            urgent_breaks_quiet: false,
            relay: String::new(),
            agents: BTreeMap::new(),
        }
    }
}

const DAY_MINUTES: u32 = 24 * 60;

pub(crate) const SCHEMA: &str = "
-- When Claktory notifies the person (crate::notify): the row 'settings'
-- holds them as JSON, each agent's Notify me included.
CREATE TABLE notify_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

impl NotificationsConfig {
    pub fn is_default(&self) -> bool {
        *self == NotificationsConfig::default()
    }

    pub fn check(&self) -> Result<(), Invalid> {
        if self.quiet_start >= DAY_MINUTES || self.quiet_end >= DAY_MINUTES {
            return Err(Invalid::BadNotifications("quiet_start and quiet_end are minutes after midnight, 0 to 1439".to_owned()));
        }
        if !self.relay.is_empty() && crate::account::service_url(&self.relay).is_none() {
            return Err(Invalid::BadNotifications(format!(
                "relay \"{}\" must be an https:// address (http:// only on this computer)",
                self.relay
            )));
        }
        Ok(())
    }

    pub(crate) fn to_proto(&self) -> v1::NotificationSettings {
        v1::NotificationSettings {
            needs_you: self.needs_you,
            alerts: self.alerts,
            replies: self.replies,
            desktop_replies: self.desktop_replies,
            preview: self.preview.to_proto().into(),
            quiet_hours: self.quiet_hours,
            quiet_start: self.quiet_start,
            quiet_end: self.quiet_end,
            urgent_breaks_quiet: self.urgent_breaks_quiet,
            relay: self.relay.clone(),
            time_zone: jiff::tz::TimeZone::system().iana_name().unwrap_or_default().to_owned(),
            utc_offset_minutes: jiff::ZonedDateTime::now().offset().seconds() / 60,
        }
    }

    /// `agent`'s "Notify me".
    pub(crate) fn mode(&self, agent: &str) -> Mode {
        self.agents.get(agent).copied().unwrap_or_default()
    }

    /// Sets `agent`'s "Notify me".
    pub(crate) fn set_mode(&mut self, agent: &str, mode: Mode) {
        if mode == Mode::Default {
            self.agents.remove(agent);
        } else {
            self.agents.insert(agent.to_owned(), mode);
        }
    }

    /// UpdateSettings.notifications: the settings as the app sends them,
    /// agents' "Notify me" kept from `self`.
    pub(crate) fn updated(&self, settings: &v1::NotificationSettings) -> NotificationsConfig {
        NotificationsConfig {
            agents: self.agents.clone(),
            needs_you: settings.needs_you,
            alerts: settings.alerts,
            replies: settings.replies,
            desktop_replies: settings.desktop_replies,
            preview: Preview::from_proto(settings.preview()),
            quiet_hours: settings.quiet_hours,
            quiet_start: settings.quiet_start,
            quiet_end: settings.quiet_end,
            urgent_breaks_quiet: settings.urgent_breaks_quiet,
            relay: settings.relay.trim().to_owned(),
        }
    }

    /// UpdateSettings.notification_change: `self` with the fields that are
    /// set changed.
    pub(crate) fn changed(&self, change: &v1::NotificationSettingsChange) -> NotificationsConfig {
        let mut settings = self.clone();
        let flags = [
            (change.needs_you, &mut settings.needs_you),
            (change.alerts, &mut settings.alerts),
            (change.replies, &mut settings.replies),
            (change.desktop_replies, &mut settings.desktop_replies),
            (change.quiet_hours, &mut settings.quiet_hours),
            (change.urgent_breaks_quiet, &mut settings.urgent_breaks_quiet),
        ];
        for (value, setting) in flags {
            if let Some(value) = value {
                *setting = value;
            }
        }
        if change.preview.is_some() {
            settings.preview = Preview::from_proto(change.preview());
        }
        if let Some(start) = change.quiet_start {
            settings.quiet_start = start;
        }
        if let Some(end) = change.quiet_end {
            settings.quiet_end = end;
        }
        if let Some(relay) = &change.relay {
            relay.trim().clone_into(&mut settings.relay);
        }
        settings
    }

    /// Quiet hours hold at `minute` (after midnight).
    fn quiet_at(&self, minute: u32) -> bool {
        let (start, end) = (self.quiet_start, self.quiet_end);
        self.quiet_hours && start != end && if start < end { (start..end).contains(&minute) } else { minute >= start || minute < end }
    }
}

/// What notifications say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preview {
    /// Who, and what it says.
    #[default]
    Full,
    /// Only who.
    Who,
    /// Just "Claktory has something for you".
    None,
}

impl Preview {
    fn to_proto(self) -> v1::NotifyPreview {
        match self {
            Preview::Full => v1::NotifyPreview::Full,
            Preview::Who => v1::NotifyPreview::Who,
            Preview::None => v1::NotifyPreview::None,
        }
    }

    fn from_proto(preview: v1::NotifyPreview) -> Preview {
        match preview {
            v1::NotifyPreview::Unspecified | v1::NotifyPreview::Full => Preview::Full,
            v1::NotifyPreview::Who => Preview::Who,
            v1::NotifyPreview::None => Preview::None,
        }
    }
}

/// Whether an agent notifies the person (config.json's agents' `notify`;
/// see v1::NotifyMode). Only written when not the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Default,
    Always,
    Never,
}

impl Mode {
    pub fn is_default(&self) -> bool {
        *self == Mode::Default
    }

    pub(crate) fn to_proto(self) -> v1::NotifyMode {
        match self {
            Mode::Default => v1::NotifyMode::Default,
            Mode::Always => v1::NotifyMode::Always,
            Mode::Never => v1::NotifyMode::Never,
        }
    }

    pub(crate) fn from_proto(mode: v1::NotifyMode) -> Mode {
        match mode {
            v1::NotifyMode::Default => Mode::Default,
            v1::NotifyMode::Always => Mode::Always,
            v1::NotifyMode::Never => Mode::Never,
        }
    }
}

// ---- State ----

/// What notifications keep while Claktory runs: who's using it where, what
/// was already seen waiting, and how many each source sent this hour.
#[derive(Default)]
pub(crate) struct State {
    data: Arc<Mutex<Data>>,
    /// Wakes `monitor`: a summary may be due sooner, or a push waits.
    wake: Arc<tokio::sync::Notify>,
    /// Devices registered for pushes, and pushes waiting.
    pub(crate) push: push::State,
    /// The settings, once read from factory.db.
    settings: Mutex<Option<NotificationsConfig>>,
    /// Held while the settings are read, changed and saved, so two changes
    /// at once both stay.
    editing: Mutex<()>,
}

#[derive(Default)]
struct Data {
    /// Each connection's device id, and whether its app is in front, by
    /// number.
    connections: HashMap<u64, (String, bool)>,
    next_connection: u64,
    /// When each device last stopped being active, in ms since 1970.
    last_active: HashMap<String, i64>,
    /// Agents whose conversations are loaded, as last seen.
    agents: HashMap<String, Seen>,
    /// Pipes' waiting seqs, as last seen.
    pipes: HashMap<String, BTreeSet<u64>>,
    /// Chats whose conversations are loaded, as last seen.
    chats: HashMap<String, BTreeSet<u64>>,
    /// Agents' direct messages waiting for review, by proposal id, as last
    /// seen.
    dm_sends: BTreeSet<String>,
    /// The first Workspace was seen: what waits then isn't new.
    started: bool,
    /// How many items wait for the person, as last seen: a push's badge.
    badge: u32,
    /// What each source sent this hour, by `Note::source`.
    sent: HashMap<String, Sent>,
}

/// An agent as last seen.
struct Seen {
    waiting: BTreeSet<u64>,
    working: bool,
    /// Entries before this were looked at for replies.
    looked: u64,
}

/// What a source (an agent, a pipe, a chat) sent this hour.
#[derive(Default)]
struct Sent {
    /// When, in ms since 1970, oldest first.
    times: VecDeque<i64>,
    /// How many it held back since, past the cap.
    held: u32,
    /// The last it held back, for the summary.
    last: Option<Note>,
}

fn lock(data: &Mutex<Data>) -> MutexGuard<'_, Data> {
    data.lock().unwrap_or_else(PoisonError::into_inner)
}

impl State {
    fn data(&self) -> MutexGuard<'_, Data> {
        lock(&self.data)
    }

    /// The presence of a connection of `device` (its id; "local" for this
    /// computer's app), inactive until it says otherwise (SetActive); it
    /// goes when the connection closes.
    pub(crate) fn connect(&self, device: &str) -> Presence {
        let mut data = self.data();
        data.next_connection += 1;
        let id = data.next_connection;
        data.connections.insert(id, (device.to_owned(), false));
        Presence { data: self.data.clone(), id }
    }

    /// The person is using Claktory on some device right now.
    fn active_now(data: &Data) -> bool {
        data.connections.values().any(|(_, active)| *active)
    }

    /// Whether a push to `device` waits: while it's in use itself (an
    /// active connection), or while the person is using another device,
    /// still connected, or did in the last two minutes, since that one
    /// shows it. A phone that just went to the background still gets it:
    /// iOS suspends its app, whose tunnel may stay open a while after, and
    /// it can't show a Notify any more.
    pub(crate) fn push_waits(&self, device: &str, now: i64) -> bool {
        let data = self.data();
        data.connections.values().any(|(other, active)| {
            *active || (other != device && data.last_active.get(other).is_some_and(|at| now - at < ACTIVE_GRACE_MS))
        })
    }

    /// How many items wait for the person.
    pub(crate) fn badge(&self) -> u32 {
        self.data().badge
    }

    fn wake(&self) {
        self.wake.notify_one();
    }

}

/// One connection's say in whether the person is using Claktory.
pub(crate) struct Presence {
    data: Arc<Mutex<Data>>,
    id: u64,
}

impl Presence {
    /// SetActive.
    pub(crate) fn set_active(&self, active: bool) {
        let mut data = lock(&self.data);
        let Some((device, was)) = data.connections.get_mut(&self.id) else { return };
        let was = std::mem::replace(was, active);
        if was && !active {
            let device = device.clone();
            data.last_active.insert(device, now_ms());
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let mut data = lock(&self.data);
        if let Some((device, true)) = data.connections.remove(&self.id) {
            data.last_active.insert(device, now_ms());
        }
    }
}

// ---- What notifies ----

/// Something to tell the person, before the settings have their say.
#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub kind: Kind,
    /// Whom the hourly cap counts it against: "agent:<id>", "pipe:<id>" or
    /// "chat:<id>".
    pub source: String,
    /// Who it's from, as the person knows them: an agent's name, a pipe's,
    /// a chat's title.
    pub who: String,
    /// What it says, in full; empty for nothing more than who.
    pub what: String,
    /// It's from an agent that has read a web page: it says only who.
    pub tainted: bool,
    /// An alert its agent marked urgent.
    pub urgent: bool,
    /// The agent's "Notify me"; Default for pipes and chats.
    pub mode: Mode,
    /// A reply to a message sent from a phone or tablet (else a computer).
    pub from_phone: bool,
    pub agent_id: String,
    pub pipe_id: String,
    pub chat_id: String,
    pub seq: u64,
}

impl Note {
    fn new(kind: Kind, source: String, who: String) -> Note {
        Note {
            kind,
            source,
            who,
            what: String::new(),
            tainted: false,
            urgent: false,
            mode: Mode::Default,
            from_phone: false,
            agent_id: String::new(),
            pipe_id: String::new(),
            chat_id: String::new(),
            seq: 0,
        }
    }
}

/// Why a note didn't notify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Skipped {
    /// Its kind is off, or its agent's "Notify me" is Never.
    Off,
    /// Quiet hours.
    Quiet,
    /// The person is using Claktory right now: they see it there.
    Active,
    /// Its source sent its hourly share: a summary says how many more.
    Held,
}

/// Whether the person's settings let `note` notify at `minute` (local,
/// after midnight); the hourly cap and presence aside.
fn allowed(settings: &NotificationsConfig, note: &Note, minute: u32) -> Result<(), Skipped> {
    let on = match note.kind {
        Kind::NeedsYou => settings.needs_you,
        Kind::Alert => settings.alerts,
        Kind::Reply if note.from_phone => settings.replies,
        Kind::Reply => settings.desktop_replies,
        Kind::Test => return Ok(()),
        Kind::Unspecified => false,
    };
    match note.mode {
        Mode::Never => return Err(Skipped::Off),
        Mode::Default if !on => return Err(Skipped::Off),
        Mode::Always | Mode::Default => {}
    }
    if settings.quiet_at(minute) && !(note.urgent && settings.urgent_breaks_quiet) {
        return Err(Skipped::Quiet);
    }
    Ok(())
}

/// What a notification says: its title and body.
pub(crate) fn text(note: &Note, preview: Preview) -> (String, String) {
    if preview == Preview::None || note.who.is_empty() {
        return ("Claktory".to_owned(), GENERIC.to_owned());
    }
    let who = note.who.clone();
    if note.tainted {
        return (who.clone(), format!("{who} has something for you"));
    }
    if preview == Preview::Who || note.what.is_empty() {
        let body = match note.kind {
            Kind::NeedsYou => format!("{who} needs you"),
            Kind::Alert => format!("{who} sent an alert"),
            Kind::Reply => format!("{who} replied"),
            Kind::Test | Kind::Unspecified => format!("{who} has something for you"),
        };
        return (who, body);
    }
    (who, shorten(&note.what))
}

/// `text` on lines of single spaces, cut to `MAX_BODY` characters.
fn shorten(text: &str) -> String {
    let lines: Vec<String> =
        text.lines().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|line| !line.is_empty()).collect();
    let joined = lines.join("\n");
    if joined.chars().count() <= MAX_BODY {
        return joined;
    }
    let cut: String = joined.chars().take(MAX_BODY - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The local time of day, in minutes after midnight.
fn local_minute() -> u32 {
    let time = jiff::ZonedDateTime::now().time();
    u32::try_from(time.hour()).unwrap_or(0) * 60 + u32::try_from(time.minute()).unwrap_or(0)
}

/// The settings as they are now.
pub(crate) fn settings(factory: &Factory) -> NotificationsConfig {
    let mut cached = factory.notify.settings.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(settings) = cached.as_ref() {
        return settings.clone();
    }
    let read = factory
        .store
        .db()
        .query_row("SELECT value FROM notify_settings WHERE key = 'settings'", [], |row| row.get::<_, String>(0))
        .optional();
    let settings = match read {
        Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_else(|error| {
            tracing::error!(%error, "the notification settings can't be read; the defaults apply");
            NotificationsConfig::default()
        }),
        Ok(None) => NotificationsConfig::default(),
        Err(error) => {
            tracing::error!(%error, "could not read the notification settings; the defaults apply");
            return NotificationsConfig::default();
        }
    };
    *cached = Some(settings.clone());
    settings
}

fn save(factory: &Factory, settings: NotificationsConfig) -> Result<(), RequestError> {
    let json = serde_json::to_string(&settings)?;
    factory.store.db().execute(
        "INSERT INTO notify_settings (key, value) VALUES ('settings', ?1) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [json],
    )?;
    *factory.notify.settings.lock().unwrap_or_else(PoisonError::into_inner) = Some(settings);
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// UpdateSettings.notifications.
pub(crate) fn update(factory: &Factory, request: &v1::NotificationSettings) -> Result<(), RequestError> {
    let _editing = factory.notify.editing.lock().unwrap_or_else(PoisonError::into_inner);
    let settings = settings(factory).updated(request);
    settings.check()?;
    save(factory, settings)
}

/// UpdateSettings.notification_change: only the settings it names.
pub(crate) fn change(factory: &Factory, change: &v1::NotificationSettingsChange) -> Result<(), RequestError> {
    let _editing = factory.notify.editing.lock().unwrap_or_else(PoisonError::into_inner);
    let old = settings(factory);
    let settings = old.changed(change);
    settings.check()?;
    if settings == old {
        return Ok(());
    }
    save(factory, settings)
}

/// UpdateAgent.notify: `agent`'s "Notify me".
pub(crate) fn set_mode(factory: &Factory, agent: &str, mode: Mode) -> Result<(), RequestError> {
    let _editing = factory.notify.editing.lock().unwrap_or_else(PoisonError::into_inner);
    let mut settings = settings(factory);
    if settings.mode(agent) == mode {
        return Ok(());
    }
    settings.set_mode(agent, mode);
    save(factory, settings)
}

/// Sees a Workspace about to be broadcast: what newly waits for the person,
/// and agents that just answered their message, notify.
pub(crate) fn observe(factory: &Factory, inner: &Inner, workspace: &v1::Workspace) {
    let settings = settings(factory);
    let notes = {
        let mut data = factory.notify.data();
        let notes = changes(&mut data, factory, inner, &settings, workspace);
        data.started = true;
        let waiting = workspace.agents.iter().map(|agent| agent.waiting_seqs.len()).sum::<usize>()
            + workspace.pipes.iter().map(|pipe| pipe.waiting_seqs.len()).sum::<usize>()
            + workspace.chats.iter().map(|chat| chat.waiting_seqs.len()).sum::<usize>()
            + workspace.dm_sends.len();
        data.badge = u32::try_from(waiting).unwrap_or(u32::MAX);
        notes
    };
    for note in notes {
        let _ = deliver(factory, &inner.config, &settings, note);
    }
}

/// What changed since the last Workspace that notifies.
fn changes(data: &mut Data, factory: &Factory, inner: &Inner, settings: &NotificationsConfig, workspace: &v1::Workspace) -> Vec<Note> {
    let mut notes = Vec::new();
    let mut agents = HashMap::new();
    for agent in &workspace.agents {
        let Some(transcript) = inner.agents.get(&agent.id).and_then(|runtime| runtime.transcript.as_ref()) else { continue };
        let waiting: BTreeSet<u64> = agent.waiting_seqs.iter().copied().collect();
        let working = agent.status() == v1::AgentStatus::Working;
        let entries = transcript.entries();
        let mut seen = match data.agents.remove(&agent.id) {
            // Just loaded: what waits in it isn't new.
            None => Seen { waiting: waiting.clone(), working, looked: entries.len() as u64 },
            Some(seen) => seen,
        };
        let mode = settings.mode(&agent.id);
        let tainted = !agent.tainted_by_pipes.is_empty() || agent.quarantined;
        let note = |kind| {
            let mut note = Note::new(kind, format!("agent:{}", agent.id), agent.name.clone());
            note.agent_id = agent.id.clone();
            note.mode = mode;
            note.tainted = tainted;
            note
        };
        for &seq in waiting.difference(&seen.waiting) {
            let Some(entry) = entries.get(seq as usize) else { continue };
            let mut note = note(Kind::NeedsYou);
            note.seq = seq;
            (note.what, note.tainted) = waiting_text(entry.kind.as_ref(), tainted);
            notes.push(note);
        }
        if seen.working
            && !working
            && let Some((seq, what, from_phone)) = reply(entries, seen.looked)
        {
            let mut note = note(Kind::Reply);
            (note.seq, note.what, note.from_phone) = (seq, what, from_phone);
            notes.push(note);
        }
        if !working {
            seen.looked = entries.len() as u64;
        }
        (seen.waiting, seen.working) = (waiting, working);
        agents.insert(agent.id.clone(), seen);
    }
    data.agents = agents;

    let mut pipes = HashMap::new();
    for pipe in &workspace.pipes {
        let waiting: BTreeSet<u64> = pipe.waiting_seqs.iter().copied().collect();
        if data.started {
            let seen = data.pipes.get(&pipe.id);
            for &seq in waiting.iter().filter(|seq| seen.is_none_or(|seen| !seen.contains(seq))) {
                let (who, what) = held_text(factory, pipe);
                let mut note = Note::new(Kind::NeedsYou, format!("pipe:{}", pipe.id), who);
                (note.pipe_id, note.seq, note.what) = (pipe.id.clone(), seq, what);
                notes.push(note);
            }
        }
        pipes.insert(pipe.id.clone(), waiting);
    }
    data.pipes = pipes;

    let mut chats = HashMap::new();
    for chat in &workspace.chats {
        let Some(transcript) = inner.agents.get(&chat.id).and_then(|runtime| runtime.transcript.as_ref()) else { continue };
        let waiting: BTreeSet<u64> = chat.waiting_seqs.iter().copied().collect();
        if let Some(seen) = data.chats.get(&chat.id) {
            for &seq in waiting.difference(seen) {
                let Some(entry) = transcript.entries().get(seq as usize) else { continue };
                let mut note = Note::new(Kind::NeedsYou, format!("chat:{}", chat.id), chat.title.clone());
                (note.chat_id, note.seq) = (chat.id.clone(), seq);
                note.what = waiting_text(entry.kind.as_ref(), false).0;
                notes.push(note);
            }
        }
        chats.insert(chat.id.clone(), waiting);
    }
    data.chats = chats;

    // An agent's direct message waiting for review: in Claktory's own words,
    // never the message or a name the friend chose.
    let dm_sends: BTreeSet<String> = workspace.dm_sends.iter().map(|waiting| waiting.proposal_id.clone()).collect();
    if data.started {
        for waiting in workspace.dm_sends.iter().filter(|waiting| !data.dm_sends.contains(&waiting.proposal_id)) {
            let Some(agent) = workspace.agents.iter().find(|agent| agent.id == waiting.agent_id) else { continue };
            let mut note = Note::new(Kind::NeedsYou, format!("agent:{}", agent.id), agent.name.clone());
            note.agent_id = agent.id.clone();
            note.mode = settings.mode(&agent.id);
            note.what = "Wants to send a message to a friend".to_owned();
            notes.push(note);
        }
    }
    data.dm_sends = dm_sends;
    notes
}

/// What a Needs-you item says, and whether it carries a taint of its own
/// (asked while its agent had read a pipe, before #346, or a web page).
fn waiting_text(kind: Option<&EntryKind>, tainted: bool) -> (String, bool) {
    match kind {
        Some(EntryKind::AskPerson(ask)) => (ask.title.clone(), tainted || !ask.taint.is_empty()),
        Some(EntryKind::SecretRequest(request)) => {
            let name = if request.secret_name.is_empty() {
                request.new_secret.as_ref().map(|secret| secret.name.clone()).unwrap_or_default()
            } else {
                request.secret_name.clone()
            };
            let what = if name.is_empty() { "Asks for a secret".to_owned() } else { format!("Asks for a secret: {name}") };
            (what, tainted)
        }
        Some(EntryKind::SecretApproval(approval)) => {
            (format!("Wants to use {} in a command", approval.secret_names.join(", ")), tainted || !approval.taint.is_empty())
        }
        Some(EntryKind::Proposal(proposal)) => {
            let first = proposal.text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
            (format!("Proposes to Rivet: {first}"), tainted || !proposal.taint.is_empty())
        }
        Some(EntryKind::ConnectProposal(proposal)) => {
            let what = if proposal.purpose.is_empty() { proposal.connector_id.clone() } else { proposal.purpose.clone() };
            (format!("Asks to connect {what}"), tainted)
        }
        Some(EntryKind::DevicePairing(pairing)) => (format!("Is adding a device: {}", pairing.name), tainted),
        _ => (String::new(), tainted),
    }
}

/// Who and what a pipe's held message says: never what was written, nor
/// any name the other side chose (the pipe's, when it named it, or its
/// Claktory's): the pipe's name only when the person gave it.
fn held_text(factory: &Factory, pipe: &v1::Pipe) -> (String, String) {
    let named_here = factory.pipes.get(&pipe.id).is_some_and(|record| record.named_here.unwrap_or(record.peer.is_none()));
    if named_here && !pipe.name.trim().is_empty() {
        (pipe.name.clone(), format!("A message in {} waits for your review", pipe.name))
    } else {
        ("A pipe".to_owned(), "A message in a pipe waits for your review".to_owned())
    }
}

/// The agent's reply among the entries from `looked` on: its last text, if
/// any text there answers the person's message, and whether they sent it
/// from a phone or tablet. With the seq of the last text.
fn reply(entries: &[v1::Entry], looked: u64) -> Option<(u64, String, bool)> {
    let new = entries.get(looked as usize..)?;
    let answered = new.iter().filter_map(|entry| entry.replied_to).filter_map(|seq| entries.get(seq as usize)).next_back()?;
    let Some(EntryKind::User(message)) = &answered.kind else { return None };
    let kind = message.sent_from.as_ref().map_or(v1::DeviceKind::Unspecified, v1::SentFrom::kind);
    let from_phone = matches!(kind, v1::DeviceKind::Phone | v1::DeviceKind::Tablet);
    let (seq, text) = new.iter().rev().find_map(|entry| match &entry.kind {
        Some(EntryKind::Assistant(text)) if !text.text.trim().is_empty() => Some((entry.seq, text.text.clone())),
        _ => None,
    })?;
    Some((seq, text, from_phone))
}

// ---- Delivering ----

/// What became of a note.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivered {
    /// Sent: to this many connected apps, and pushed to this many devices.
    Sent {
        apps: usize,
        pushed: usize,
    },
    Skipped(Skipped),
}

/// Tells the person `note`, if their settings, their presence and its
/// source's hourly share let it.
pub(crate) fn deliver(factory: &Factory, config: &Config, settings: &NotificationsConfig, note: Note) -> Delivered {
    if let Err(skipped) = allowed(settings, &note, local_minute()) {
        return Delivered::Skipped(skipped);
    }
    let now = now_ms();
    {
        let mut data = factory.notify.data();
        // An alert shows nowhere else, so the app in front gets it too.
        if State::active_now(&data) && !matches!(note.kind, Kind::Alert | Kind::Test) {
            return Delivered::Skipped(Skipped::Active);
        }
        if note.kind != Kind::Test {
            let sent = data.sent.entry(note.source.clone()).or_default();
            while sent.times.front().is_some_and(|time| now - time >= HOUR_MS) {
                sent.times.pop_front();
            }
            if sent.times.len() >= PER_HOUR {
                sent.held += 1;
                sent.last = Some(note);
                factory.notify.wake();
                return Delivered::Skipped(Skipped::Held);
            }
            sent.times.push_back(now);
        }
    }
    send(factory, config, settings, &note, now)
}

/// Sends `note` to the connected apps and pushes it to the devices that
/// aren't connected.
fn send(factory: &Factory, config: &Config, settings: &NotificationsConfig, note: &Note, now: i64) -> Delivered {
    let (title, body) = text(note, settings.preview);
    let collapse = match note.kind {
        Kind::Alert => format!("{}:alert", note.source),
        Kind::Reply => format!("{}:reply", note.source),
        Kind::Test => "test".to_owned(),
        Kind::NeedsYou | Kind::Unspecified => String::new(),
    };
    let notify = v1::Notify {
        id: format!("{now}-{}", rand_suffix()),
        kind: note.kind.into(),
        title,
        body,
        agent_id: note.agent_id.clone(),
        pipe_id: note.pipe_id.clone(),
        chat_id: note.chat_id.clone(),
        seq: note.seq,
        urgent: note.urgent && settings.urgent_breaks_quiet,
        collapse,
        thread: note.source.clone(),
        sent_ms: now,
    };
    let apps = factory.clients();
    factory.send(Body::Notify(notify.clone()));
    let pushed = push::queue(factory, config, settings, &notify, now);
    tracing::info!(kind = ?note.kind, source = %note.source, apps, pushed, "notified the person");
    Delivered::Sent { apps, pushed }
}

/// A few random hex digits, so two notifications in one millisecond differ.
fn rand_suffix() -> String {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SendTestNotification: to every connected app and every registered
/// device, whatever the settings and presence.
pub(crate) fn send_test(factory: &Factory) -> Delivered {
    let config = factory.lock().config.clone();
    let mut note = Note::new(Kind::Test, "test".to_owned(), config.claktory_name());
    note.what = "This is a test notification. Notifications work.".to_owned();
    send(factory, &config, &settings(factory), &note, now_ms())
}

/// Sends what waits: sources' summaries once their hour allows another,
/// and pushes. Runs while Claktory does.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let wake = strong.notify.wake.clone();
        let next = summarize(&strong);
        let retry = push::flush(&strong).await;
        drop(strong);
        let mut wait =
            next.map_or(Duration::from_secs(3600), |due| Duration::from_millis(u64::try_from(due - now_ms()).unwrap_or(0).max(1000)));
        if retry {
            wait = wait.min(RETRY);
        }
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// Sends the summaries that are due; when the next is, if any.
fn summarize(factory: &Factory) -> Option<i64> {
    let now = now_ms();
    let config = factory.lock().config.clone();
    let settings = settings(factory);
    let mut due = Vec::new();
    let mut next = None::<i64>;
    {
        let mut data = factory.notify.data();
        let active = State::active_now(&data);
        data.sent.retain(|_, sent| {
            while sent.times.front().is_some_and(|time| now - time >= HOUR_MS) {
                sent.times.pop_front();
            }
            if sent.held == 0 {
                return !sent.times.is_empty();
            }
            if sent.times.len() >= PER_HOUR {
                let at = sent.times.front().map_or(now, |time| time + HOUR_MS);
                next = Some(next.map_or(at, |next| next.min(at)));
                return true;
            }
            if let Some(last) = sent.last.take() {
                // The person is looking: they see it in the app.
                if !active {
                    due.push((last, sent.held));
                    sent.times.push_back(now);
                }
            }
            sent.held = 0;
            true
        });
    }
    for (last, held) in due {
        let mut note = last;
        note.what = if held == 1 { format!("{} sent 1 more", note.who) } else { format!("{} sent {held} more", note.who) };
        note.tainted = false;
        note.urgent = false;
        if allowed(&settings, &note, local_minute()).is_ok() {
            send(factory, &config, &settings, &note, now);
        }
    }
    next
}

/// Tests: the pipe as if the other side had named it (a join without a
/// name of the person's own).
#[doc(hidden)]
pub fn pipe_named_by_peer_for_test(factory: &Factory, pipe: &str) {
    crate::pipes::change(factory, pipe, |record| record.named_here = Some(false));
}

/// Tests: how many connections have a say in presence now.
#[doc(hidden)]
pub fn connections_for_test(factory: &Factory) -> usize {
    factory.notify.data().connections.len()
}

/// Tests: whether `device` gets pushes.
#[doc(hidden)]
pub fn registered_for_test(factory: &Factory, device: &str) -> bool {
    push::registered(factory, device)
}

#[cfg(test)]
mod tests;
