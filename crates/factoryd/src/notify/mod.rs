//! Notifications (#273; the plan is notifications.md in Research): what's
//! worth telling the person when they aren't looking, decided here, the same
//! for every device (tenet 7).
//!
//! Three kinds notify: a new Needs-you item (a seq added to an agent's,
//! pipe's or chat's waiting seqs, or agents starting to wait on a
//! provider's usage limit or credit), an agent's alert (`notify_person`, in
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
const DAY_MS: i64 = 24 * HOUR_MS;
/// For this long after the person was last using Claktory on a device,
/// nothing is pushed to the others: they're probably still at it.
pub(crate) const ACTIVE_GRACE_MS: i64 = 120_000;
/// The longest a notification's body gets, in characters.
pub(crate) const MAX_BODY: usize = 300;
/// The source of the one notification when quiet hours end (O3).
const QUIET: &str = "quiet";
/// What a notification says when it may say nothing more.
pub(crate) const GENERIC: &str = "Claktory has something for you";
/// How soon pushes the relay couldn't take are tried again.
const RETRY: Duration = Duration::from_secs(30);
/// How long a provider wait (Workspace.provider_waits) must have been over
/// before it notifies again when it comes back: Try now, a message or a
/// usage read wake the waiters for a moment, and they wait again.
const WAIT_AGAIN_MS: i64 = 10 * 60_000;
/// How many devices waiting to be allowed notify in an hour, all of them
/// together (#352 P4, Fugo): past it they show only in Needs you.
pub(crate) const DEVICES_PER_HOUR: usize = 3;

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
    /// The connections whose apps show notifications (`shows_notifications`).
    showing: BTreeSet<u64>,
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
    /// Provider waits as last noted, by provider id; read from factory.db
    /// at the first look (None until then).
    provider_waits: Option<BTreeMap<String, Noted>>,
    /// Devices waiting to be allowed, as noted; read from factory.db at the
    /// first look (None until then).
    devices: Option<DevicesNoted>,
    /// Needs-you items that came during quiet hours, by `waiting_keys`'
    /// names; read from factory.db at the first look (None until then).
    quiet: Option<BTreeSet<String>>,
    /// What waits for the person now, by `waiting_keys`' names.
    waiting: BTreeSet<String>,
    /// The first Workspace was seen: what waits then isn't new.
    started: bool,
    /// The Mac couldn't sleep with its lid closed (#371's STUCK), as last
    /// seen: noted once when it starts, at a start too.
    lid_stuck: bool,
    /// How many items wait for the person, as last seen: a push's badge.
    badge: u32,
    /// What each source sent this hour, by `Note::source`.
    sent: HashMap<String, Sent>,
}

/// Devices waiting to be allowed, as noted, kept in factory.db
/// (notify_settings' row 'devices') so a restart notifies none of them
/// again, nor more than the hour's `DEVICES_PER_HOUR`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct DevicesNoted {
    /// Each request seen, "<id>:<requested_ms>", by its requested_ms: kept a
    /// day, past any code's life, so one missing from a Workspace for a
    /// moment (at a start) isn't new when it's back.
    seen: BTreeMap<String, i64>,
    /// When devices waiting to be allowed notified, this hour.
    times: VecDeque<i64>,
}

/// A provider wait as last noted (Workspace.provider_waits), kept in
/// factory.db (notify_settings' row 'provider_waits') so a restart, which
/// ends every wait for a moment, doesn't notify it again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Noted {
    credit: bool,
    until_ms: i64,
    /// When it was over, in ms since 1970; 0 while it lasts.
    gone_ms: i64,
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

    /// Entry `seq` of `agent`, which already waits for the person, notifies
    /// at the next Workspace as if it newly waited: its agent answered them
    /// on a card that still waits (#430, ask_reply on a procedure).
    pub(crate) fn waits_again(&self, agent: &str, seq: u64) {
        if let Some(seen) = self.data().agents.get_mut(agent) {
            seen.waiting.remove(&seq);
        }
    }

    /// The presence of a connection of `device` (its id; "local" for this
    /// computer's app), inactive until it says otherwise (SetActive); it
    /// goes when the connection closes.
    pub(crate) fn connect(&self, device: &str, platform: &str) -> Presence {
        let mut data = self.data();
        data.next_connection += 1;
        let id = data.next_connection;
        data.connections.insert(id, (device.to_owned(), false));
        if shows_notifications(platform) {
            data.showing.insert(id);
        }
        Presence { data: self.data.clone(), id }
    }

    /// How many connected apps show a Notify as a notification.
    fn showing(&self) -> usize {
        self.data().showing.len()
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
        data.connections
            .values()
            .any(|(other, active)| *active || (other != device && data.last_active.get(other).is_some_and(|at| now - at < ACTIVE_GRACE_MS)))
    }

    /// How many items wait for the person.
    pub(crate) fn badge(&self) -> u32 {
        self.data().badge
    }

    fn wake(&self) {
        self.wake.notify_one();
    }
}

/// Whether an app on `platform` (Hello.platform) shows a Notify as a
/// notification: the Mac's, the iPhone's and the iPad's do; Android's
/// can't yet (#366 A2), nor Linux's or Windows'. Apps from before
/// platform was sent were the Mac's and the iPhone's.
fn shows_notifications(platform: &str) -> bool {
    matches!(platform, "" | "macos" | "ios" | "ipados")
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
        data.showing.remove(&self.id);
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
    /// A device waiting to be allowed, whose Needs you item a tap opens.
    pub device_id: String,
    /// The Needs-you item it's about, as `waiting_keys` names it: held
    /// through quiet hours while it waits (O3).
    pub waits: String,
    /// A website another Claktory started sharing over `pipe_id` (W2),
    /// whose share a tap opens.
    pub share_id: String,
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
            device_id: String::new(),
            waits: String::new(),
            share_id: String::new(),
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
    // Claktory's own words when quiet hours end, which say nothing private.
    if note.source == QUIET {
        return (who, note.what.clone());
    }
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
    // Quiet hours may have ended (O3).
    factory.notify.wake();
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
            + workspace.dm_sends.len()
            + workspace.provider_waits.len()
            + usize::from(lid_stuck(workspace))
            + workspace.remote.as_ref().map_or(0, |remote| remote.pending_devices.len());
        data.badge = u32::try_from(waiting).unwrap_or(u32::MAX);
        data.waiting = waiting_keys(workspace);
        notes
    };
    for note in notes {
        let _ = deliver(factory, &inner.config, &settings, note);
    }
}

/// What waits for the person in `workspace`, each named as its note's
/// `waits`.
fn waiting_keys(workspace: &v1::Workspace) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for agent in &workspace.agents {
        keys.extend(agent.waiting_seqs.iter().map(|seq| format!("agent:{}:{seq}", agent.id)));
    }
    for pipe in &workspace.pipes {
        keys.extend(pipe.waiting_seqs.iter().map(|seq| format!("pipe:{}:{seq}", pipe.id)));
    }
    for chat in &workspace.chats {
        keys.extend(chat.waiting_seqs.iter().map(|seq| format!("chat:{}:{seq}", chat.id)));
    }
    keys.extend(workspace.dm_sends.iter().map(|waiting| format!("dm:{}", waiting.proposal_id)));
    keys.extend(workspace.provider_waits.iter().map(|wait| format!("provider:{}", wait.provider_id)));
    if lid_stuck(workspace) {
        keys.insert("keep_awake:lid".to_owned());
    }
    let pending = workspace.remote.as_ref().map_or(&[][..], |remote| &remote.pending_devices[..]);
    keys.extend(pending.iter().map(|device| format!("device:{}", device.device_id)));
    keys
}

/// What changed since the last Workspace that notifies.
fn changes(data: &mut Data, factory: &Factory, inner: &Inner, settings: &NotificationsConfig, workspace: &v1::Workspace) -> Vec<Note> {
    let mut notes = Vec::new();
    let mut agents = HashMap::new();
    for agent in &workspace.agents {
        let Some(transcript) = inner.agents.get(&agent.id).and_then(|runtime| runtime.transcript.as_ref()) else { continue };
        let waiting: BTreeSet<u64> = agent.waiting_seqs.iter().copied().collect();
        let working = agent.status() == v1::AgentStatus::Working;
        let mut seen = match data.agents.remove(&agent.id) {
            // Just loaded: what waits in it isn't new.
            None => Seen { waiting: waiting.clone(), working, looked: transcript.len() },
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
            let Some(entry) = transcript.entry(seq) else { continue };
            let mut note = note(Kind::NeedsYou);
            note.seq = seq;
            note.waits = format!("agent:{}:{seq}", agent.id);
            (note.what, note.tainted) = waiting_text(entry.kind.as_ref(), tainted);
            notes.push(note);
        }
        if seen.working
            && !working
            && let (base, held) = transcript.window()
            && let Some((seq, what, from_phone)) = reply(held, base, seen.looked)
        {
            let mut note = note(Kind::Reply);
            (note.seq, note.what, note.from_phone) = (seq, what, from_phone);
            notes.push(note);
        }
        if !working {
            seen.looked = transcript.len();
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
                note.waits = format!("pipe:{}:{seq}", pipe.id);
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
                let Some(entry) = transcript.entry(seq) else { continue };
                let mut note = Note::new(Kind::NeedsYou, format!("chat:{}", chat.id), chat.title.clone());
                (note.chat_id, note.seq) = (chat.id.clone(), seq);
                note.waits = format!("chat:{}:{seq}", chat.id);
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
            note.waits = format!("dm:{}", waiting.proposal_id);
            notes.push(note);
        }
    }
    data.dm_sends = dm_sends;

    notes.extend(provider_waits(data, factory, inner, settings, workspace));
    notes.extend(devices(data, factory, inner, workspace));
    notes.extend(lid(data, inner, workspace));
    notes
}

/// Whether the Mac can't sleep with its lid closed although Claktory isn't
/// keeping it awake (#371, LID_CLOSED_STUCK).
fn lid_stuck(workspace: &v1::Workspace) -> bool {
    workspace.keep_awake.as_ref().is_some_and(|keep| keep.lid_closed() == v1::LidClosedState::LidClosedStuck)
}

/// The Mac became unable to sleep with its lid closed: one note, in
/// Claktory's words with what fixes it, also when a start finds it so (the
/// only way the person hears in time, as a closed laptop drains or heats).
fn lid(data: &mut Data, inner: &Inner, workspace: &v1::Workspace) -> Vec<Note> {
    let stuck = lid_stuck(workspace);
    let new = stuck && !data.lid_stuck;
    data.lid_stuck = stuck;
    if !new {
        return Vec::new();
    }
    let machine = inner.config.claktory_name();
    let fix = workspace.keep_awake.as_ref().map(|keep| keep.lid_closed_fix.replace('`', "")).unwrap_or_default();
    let mut note = Note::new(Kind::NeedsYou, "keep_awake".to_owned(), machine.clone());
    note.what = format!("{machine} can't sleep with its lid closed right now. {fix}").trim_end().to_owned();
    note.waits = "keep_awake:lid".to_owned();
    vec![note]
}

/// Devices that started waiting to be allowed (#352 P4, Fugo's rules): in
/// Claktory's words only, never the name the device gave itself, which an
/// account thief chooses; one per device, never again, and at most
/// `DEVICES_PER_HOUR` an hour in all. A notification can't allow it: a tap
/// opens its Needs you item, where the person types the digits it shows.
fn devices(data: &mut Data, factory: &Factory, inner: &Inner, workspace: &v1::Workspace) -> Vec<Note> {
    let started = data.started;
    let noted = data.devices.get_or_insert_with(|| read_row(factory, "devices"));
    let before = noted.clone();
    let now = now_ms();
    noted.seen.retain(|_, requested| now - *requested < DAY_MS);
    while noted.times.front().is_some_and(|time| now - time >= HOUR_MS) {
        noted.times.pop_front();
    }
    let pending = workspace.remote.as_ref().map_or(&[][..], |remote| &remote.pending_devices[..]);
    let machine = inner.config.claktory_name();
    let mut notes = Vec::new();
    for device in pending {
        let key = format!("{}:{}", device.device_id, device.requested_ms);
        if noted.seen.insert(key, device.requested_ms).is_some() || !started {
            continue;
        }
        if noted.times.len() >= DEVICES_PER_HOUR {
            tracing::info!(device = %device.device_id, "a device waits to be allowed; past the hour's notifications, it shows only in Needs you");
            continue;
        }
        noted.times.push_back(now);
        let mut note = Note::new(Kind::NeedsYou, "devices".to_owned(), machine.clone());
        note.what = format!("A device is waiting to be allowed on {machine}.");
        note.device_id = device.device_id.clone();
        note.waits = format!("device:{}", device.device_id);
        notes.push(note);
    }
    if *noted != before {
        save_row(factory, "devices", noted);
    }
    notes
}

/// Providers agents and chats started waiting on, for a usage limit or
/// credit: one note each, from Claktory rather than an agent, in its own
/// words. Not again while the wait lasts, nor when it's back within
/// `WAIT_AGAIN_MS` of ending, unless it's for something else now (credit
/// instead of a limit, or a later reset once the earlier one passed).
fn provider_waits(
    data: &mut Data,
    factory: &Factory,
    inner: &Inner,
    settings: &NotificationsConfig,
    workspace: &v1::Workspace,
) -> Vec<Note> {
    let noted = data.provider_waits.get_or_insert_with(|| read_row(factory, "provider_waits"));
    let before = noted.clone();
    let now = now_ms();
    noted.retain(|_, old| old.gone_ms == 0 || now - old.gone_ms < WAIT_AGAIN_MS);
    let mut notes = Vec::new();
    for wait in &workspace.provider_waits {
        let new = noted.get(&wait.provider_id).is_none_or(|old| {
            old.credit != wait.credit || (!wait.credit && old.until_ms != 0 && old.until_ms <= now && wait.until_ms > old.until_ms)
        });
        noted.insert(wait.provider_id.clone(), Noted { credit: wait.credit, until_ms: wait.until_ms, gone_ms: 0 });
        if !new {
            continue;
        }
        let mut note = Note::new(Kind::NeedsYou, format!("provider:{}", wait.provider_id), inner.config.claktory_name());
        note.what = format!("{}. {}", wait.title, wait.text);
        note.waits = format!("provider:{}", wait.provider_id);
        // Opens the first of them, whose wait line offers the ways forward.
        match wait.agent_ids.first() {
            Some(agent) => note.agent_id = agent.clone(),
            None => note.chat_id = wait.chat_ids.first().cloned().unwrap_or_default(),
        }
        // Their "Notify me": Always if any says so, Never if all of them do.
        let modes: Vec<Mode> = wait.agent_ids.iter().map(|agent| settings.mode(agent)).collect();
        note.mode = if modes.contains(&Mode::Always) {
            Mode::Always
        } else if wait.chat_ids.is_empty() && !modes.is_empty() && modes.iter().all(|mode| *mode == Mode::Never) {
            Mode::Never
        } else {
            Mode::Default
        };
        notes.push(note);
    }
    for (provider, old) in noted.iter_mut() {
        if old.gone_ms == 0 && !workspace.provider_waits.iter().any(|wait| wait.provider_id == *provider) {
            old.gone_ms = now;
        }
    }
    if *noted != before {
        save_row(factory, "provider_waits", noted);
    }
    notes
}

/// What notifications noted, kept in factory.db (notify_settings' row
/// `key`: 'provider_waits', 'devices'); empty when there's none or it
/// can't be read.
fn read_row<T: serde::de::DeserializeOwned + Default>(factory: &Factory, key: &str) -> T {
    let read =
        factory.store.db().query_row("SELECT value FROM notify_settings WHERE key = ?1", [key], |row| row.get::<_, String>(0)).optional();
    match read {
        Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_else(|error| {
            tracing::warn!(%error, key, "what notifications noted can't be read; starting afresh");
            T::default()
        }),
        Ok(None) => T::default(),
        Err(error) => {
            tracing::warn!(%error, key, "could not read what notifications noted");
            T::default()
        }
    }
}

fn save_row<T: Serialize>(factory: &Factory, key: &str, noted: &T) {
    let saved = serde_json::to_string(noted).map_err(|error| error.to_string()).and_then(|json| {
        factory
            .store
            .db()
            .execute(
                "INSERT INTO notify_settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [key, json.as_str()],
            )
            .map_err(|error| error.to_string())
    });
    if let Err(error) = saved {
        tracing::warn!(%error, key, "could not keep what notifications noted");
    }
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

/// A Claktory on `pipe` started sharing a website with this one (W2, its
/// `share`): one notification, under Needs you, whose tap opens the pipe at
/// the share. As a held message's (`held_text`), it never says what the
/// other side chose (the website's name, the pipe's when they named it):
/// the pipe's name only when the person gave it.
pub(crate) fn shared(factory: &Factory, pipe: &str, share: &str) {
    let named = factory
        .pipes
        .get(pipe)
        .filter(|record| record.named_here.unwrap_or(record.peer.is_none()))
        .map(|record| record.name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let (who, what) = match named {
        Some(name) => (name.clone(), format!("{name} shares a website with you")),
        None => ("A pipe".to_owned(), "A website was shared with you in a pipe".to_owned()),
    };
    let mut note = Note::new(Kind::NeedsYou, format!("pipe:{pipe}"), who);
    (note.pipe_id, note.share_id, note.what) = (pipe.to_owned(), share.to_owned(), what);
    let config = factory.lock().config.clone();
    let delivered = deliver(factory, &config, &settings(factory), note);
    tracing::info!(pipe, ?delivered, "told the person a website was shared with them");
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
/// from a phone or tablet. With the seq of the last text. `entries` are the
/// ones held, from seq `base` on.
fn reply(entries: &[v1::Entry], base: u64, looked: u64) -> Option<(u64, String, bool)> {
    let at = |seq: u64| entries.get(usize::try_from(seq.checked_sub(base)?).ok()?);
    let new = entries.get(usize::try_from(looked.saturating_sub(base)).ok()?..)?;
    let answered = new.iter().filter_map(|entry| entry.replied_to).filter_map(at).next_back()?;
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
        // A Needs-you item that came during quiet hours waits for their end
        // (O3), counted in one notification then if it still waits.
        if skipped == Skipped::Quiet && note.kind == Kind::NeedsYou && !note.waits.is_empty() {
            let mut data = factory.notify.data();
            let held = data.quiet.get_or_insert_with(|| read_row(factory, "quiet"));
            if held.insert(note.waits.clone()) {
                save_row(factory, "quiet", held);
            }
            drop(data);
            factory.notify.wake();
        }
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
        device_id: note.device_id.clone(),
        share_id: note.share_id.clone(),
        urgent: note.urgent && settings.urgent_breaks_quiet,
        collapse,
        thread: note.source.clone(),
        sent_ms: now,
    };
    // Only apps that show it count as reaching the person (#366 A2).
    let apps = factory.notify.showing();
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
        let next = [summarize(&strong), morning(&strong)].into_iter().flatten().min();
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

/// When quiet hours have ended and Needs-you items that came during them
/// still wait, one notification says how many (O3; Trish's words): "2
/// things need you", "They came during your quiet hours.". A tap opens
/// Needs you. When the next look is due while they last.
fn morning(factory: &Factory) -> Option<i64> {
    let settings = settings(factory);
    let now = now_ms();
    let count = {
        let mut data = factory.notify.data();
        // What waits is known from the first Workspace on.
        let started = data.started;
        let held = data.quiet.get_or_insert_with(|| read_row(factory, "quiet"));
        if held.is_empty() {
            return None;
        }
        let time = jiff::ZonedDateTime::now().time();
        let minute = u32::try_from(time.hour()).unwrap_or(0) * 60 + u32::try_from(time.minute()).unwrap_or(0);
        if settings.quiet_at(minute) {
            let minutes = i64::from((settings.quiet_end + DAY_MINUTES - minute) % DAY_MINUTES);
            return Some(now + minutes * 60_000 - i64::from(time.second()) * 1000);
        }
        if !started {
            return Some(now + 1000);
        }
        let held = std::mem::take(held);
        save_row(factory, "quiet", &BTreeSet::<String>::new());
        held.intersection(&data.waiting).count()
    };
    if count > 0 {
        let config = factory.lock().config.clone();
        let title = if count == 1 { "1 thing needs you".to_owned() } else { format!("{count} things need you") };
        let mut note = Note::new(Kind::NeedsYou, QUIET.to_owned(), title);
        note.what = if count == 1 { "It came during your quiet hours." } else { "They came during your quiet hours." }.to_owned();
        let delivered = deliver(factory, &config, &settings, note);
        tracing::info!(count, ?delivered, "quiet hours ended with Needs-you items that came during them");
    }
    None
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

/// Tests: the registrations are read again from the file, as after a
/// restart.
#[doc(hidden)]
pub fn forget_registrations_for_test(factory: &Factory) {
    push::forget_read(factory);
}

#[cfg(test)]
mod tests;
