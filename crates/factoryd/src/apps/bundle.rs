//! A .clapp: an app's bundle. An uncompressed tar (ustar) of flat files:
//! `manifest.toml`, `app.wasm` (the WebAssembly component) and `signature`,
//! two lines of text:
//!
//! ```text
//! key ed25519:<base64 of the 32-byte public key>
//! sig <base64 of the 64-byte signature>
//! ```
//!
//! signed over `claktory-app-v1\n` and a `sha256sum` line for every other
//! file, in name order ("<hex>  app.wasm\n<hex>  manifest.toml\n"). Its
//! checksum is the SHA-256 of the whole file.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::encoding::hex;

/// The WIT world apps are built for, which every apps host runs.
pub(crate) const WORLD: &str = "claktory:app@1";
/// The world of apps with views (WIT 1.1).
pub(crate) const WORLD_VIEWS: &str = "claktory:app@1.1";
/// The most a bundle may weigh.
pub(crate) const MAX_BYTES: usize = 16 * 1024 * 1024;
/// What the signature is made over, before the files' lines.
const SIGNED_PREFIX: &str = "claktory-app-v1\n";
/// Claktory's own keys for apps it makes (tier Claktory): the Ed25519
/// public key, standard base64. Apps only: this isn't the key releases of
/// Claktory are signed with, and it signs nothing else. Its private half
/// lives in the vault (scripts/sign-clapp.sh --claktory signs with it and
/// refuses a key not listed here). An install pins the key it came with,
/// so adding or retiring a key here never lets one publisher's app replace
/// another's: it only changes the tier shown.
const CLAKTORY_KEYS: &[&str] = &[
    // The Claktory apps publisher key (2026-11), from the vault's
    // "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAII5l9hw1FpwDhaXLFH/NhQkOYKrjZSBr6fKotr5j8vaH".
    "jmX2HDUWnAOFpcsUf82FCQ5gquNlIGvp8qi2vmPy9oc=",
];
/// The most an app may ask for.
const MAX_MEMORY_MB: u32 = 1024;
const MAX_STORAGE_MB: u32 = 1024;

/// manifest.toml.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub world: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    /// A Lucide icon name.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default = "default_memory")]
    pub memory_mb: u32,
    #[serde(default = "default_storage")]
    pub storage_mb: u32,
    /// Its section of a granted agent's system prompt.
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub secrets: Vec<SecretSpec>,
    #[serde(default)]
    pub scopes: Vec<ScopeSpec>,
    #[serde(default)]
    pub events: Vec<EventSpec>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// The views the person can open; the first is its main one. Only
    /// for world 1.1.
    #[serde(default)]
    pub views: Vec<ViewSpec>,
    /// The data it asks to read, each granted by the person or not. Only
    /// for world 1.1.
    #[serde(default)]
    pub data: Vec<DataSpec>,
}

fn default_memory() -> u32 {
    32
}

fn default_storage() -> u32 {
    5
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SecretSpec {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ScopeSpec {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub tools: Vec<String>,
    /// Whether its tools can hand an agent what the app reads from its
    /// sources. Left out, they're taken to, when the app asks for any data.
    #[serde(default)]
    pub reads_data: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct EventSpec {
    pub kind: String,
    pub scope: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ToolSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Its arguments' JSON schema.
    #[serde(default = "no_parameters")]
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ViewSpec {
    pub id: String,
    pub title: String,
}

/// A `[[data]]` source an app asks to read.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DataSpec {
    /// "activity" or "conversations" (from factory.db), "agent-database"
    /// (an agent's own), "file" (CSV or TSV files the person chooses), or
    /// "factory-records" (all of factory.db: still read, so apps made for
    /// it work, but a new grant of it is refused).
    pub kind: String,
    /// For "agent-database": an agent's id, or "*" for any the person
    /// chooses.
    #[serde(default)]
    pub agent: String,
    /// Why it asks, for the person.
    #[serde(default)]
    pub reason: String,
}

/// Words Claktory shows as an app's own (its name, its views' titles): up
/// to 40 letters, digits, spaces and -, never saying Claktory, even with
/// letters that read alike in a sans-serif font ("CIaktory", "C1aktory").
fn plain(text: &str, what: &str) -> Result<(), String> {
    if text.trim() != text || text.chars().count() > 40 || !text.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-') {
        return Err(format!("the {what} \"{text}\" has to be up to 40 letters, digits, spaces and -"));
    }
    let folded = text.to_ascii_lowercase().replace([' ', '-'], "").replace(['i', '1'], "l").replace('0', "o");
    if folded.contains("claktory") {
        return Err(format!("the {what} \"{text}\" can't say Claktory: only Claktory itself does"));
    }
    Ok(())
}

/// The kinds of `[[data]]`.
pub(crate) const FACTORY_RECORDS: &str = "factory-records";
pub(crate) const ACTIVITY: &str = "activity";
pub(crate) const CONVERSATIONS: &str = "conversations";
pub(crate) const AGENT_DATABASE: &str = "agent-database";
/// Files the person (or Rivet, on a card) chooses: a CSV or TSV each
/// (`super::files`).
pub(crate) const FILE: &str = "file";

fn no_parameters() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

impl Manifest {
    /// Reads and checks manifest.toml; why not, a sentence for the person.
    pub(crate) fn parse(text: &str) -> Result<Manifest, String> {
        let manifest: Manifest = toml::from_str(text).map_err(|error| format!("This app's manifest can't be read: {}", error.message()))?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), String> {
        let bad = |what: String| Err(format!("This app's manifest isn't valid: {what}."));
        if self.id.is_empty()
            || self.id.len() > 32
            || !self.id.starts_with(|c: char| c.is_ascii_lowercase())
            || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return bad(format!("the id \"{}\" has to be lowercase letters and digits", self.id));
        }
        if self.name.trim().is_empty() {
            return bad("it has no name".to_owned());
        }
        // Its name frames its views ("From Telegram") and its fields ("Telegram
        // gets what you type here"): plain letters, so no app can pass itself
        // off as Claktory with a name like "Claktory" or a lookalike of it.
        if let Err(why) = plain(&self.name, "name") {
            return bad(why);
        }
        if self.version.trim().is_empty() || self.version.len() > 64 || self.version.contains(['/', '\\']) || self.version.starts_with('.')
        {
            return bad(format!("the version \"{}\" can't be used", self.version));
        }
        if self.world != WORLD && self.world != WORLD_VIEWS {
            return Err(format!("This app needs a different version of Claktory ({}; this one runs {WORLD_VIEWS}).", self.world));
        }
        if !self.views.is_empty() && self.world != WORLD_VIEWS {
            return bad(format!("views need world = \"{WORLD_VIEWS}\""));
        }
        if !self.data.is_empty() && self.world != WORLD_VIEWS {
            return bad(format!("[[data]] needs world = \"{WORLD_VIEWS}\""));
        }
        let mut sources = BTreeSet::new();
        for data in &self.data {
            match data.kind.as_str() {
                FACTORY_RECORDS | ACTIVITY | CONVERSATIONS | FILE if data.agent.is_empty() => {}
                FACTORY_RECORDS | ACTIVITY | CONVERSATIONS | FILE => return bad(format!("{} names no agent", data.kind)),
                AGENT_DATABASE if data.agent == "*" || (!data.agent.is_empty() && data.agent.len() <= 64) => {}
                AGENT_DATABASE => return bad("agent-database needs agent = \"*\" or an agent's id".to_owned()),
                other => return bad(format!("the data kind \"{other}\" isn't activity, conversations, agent-database or file")),
            }
            if !sources.insert((data.kind.as_str(), data.agent.as_str())) {
                return bad(format!("the data \"{}\" is there twice", data.kind));
            }
        }
        let mut views = BTreeSet::new();
        for view in &self.views {
            if view.id.is_empty()
                || view.id.len() > 32
                || !view.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                return bad(format!("the view \"{}\" has to be named with lowercase letters, digits, - and _", view.id));
            }
            if !views.insert(view.id.as_str()) {
                return bad(format!("the view \"{}\" is there twice", view.id));
            }
            if view.title.trim().is_empty() {
                return bad(format!("the view \"{}\" has no title", view.id));
            }
            // Shown after the app's name, and alone where a surface forgets
            // it: held to the same rule (Fugo).
            if let Err(why) = plain(&view.title, &format!("view \"{}\"'s title", view.id)) {
                return bad(why);
            }
        }
        for host in &self.hosts {
            let name = host.strip_prefix("*.").unwrap_or(host);
            if name.is_empty() || !name.contains('.') || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
                return bad(format!("\"{host}\" isn't a host name"));
            }
        }
        if self.memory_mb == 0 || self.memory_mb > MAX_MEMORY_MB {
            return bad(format!("memory_mb has to be 1 to {MAX_MEMORY_MB}"));
        }
        if self.storage_mb > MAX_STORAGE_MB {
            return bad(format!("storage_mb has to be at most {MAX_STORAGE_MB}"));
        }
        let mut names = BTreeSet::new();
        for secret in &self.secrets {
            if secret.name.is_empty() || !secret.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return bad(format!("the secret \"{}\" has to be named with lowercase letters, digits and _", secret.name));
            }
            if !names.insert(secret.name.as_str()) {
                return bad(format!("the secret \"{}\" is there twice", secret.name));
            }
        }
        let prefix = format!("{}_", self.id);
        let mut tools = BTreeSet::new();
        for tool in &self.tools {
            if !tool.name.starts_with(&prefix)
                || tool.name.len() > 64
                || !tool.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return bad(format!("the tool \"{}\" has to be named {prefix}… in lowercase letters, digits and _", tool.name));
            }
            if !tool.parameters.is_object() {
                return bad(format!("the tool \"{}\" has no parameters schema", tool.name));
            }
            if !tools.insert(tool.name.as_str()) {
                return bad(format!("the tool \"{}\" is there twice", tool.name));
            }
        }
        let mut scopes = BTreeSet::new();
        let mut scoped = BTreeMap::new();
        for scope in &self.scopes {
            if scope.id.is_empty() || !scopes.insert(scope.id.as_str()) {
                return bad(format!("the scope \"{}\" is there twice or has no id", scope.id));
            }
            for tool in &scope.tools {
                if !tools.contains(tool.as_str()) {
                    return bad(format!("the scope \"{}\" gives a tool it doesn't have, \"{tool}\"", scope.id));
                }
                scoped.insert(tool.as_str(), scope.id.as_str());
            }
        }
        if let Some(tool) = tools.iter().find(|tool| !scoped.contains_key(*tool)) {
            return bad(format!("no scope gives the tool \"{tool}\""));
        }
        let mut kinds = BTreeSet::new();
        for event in &self.events {
            if event.kind.is_empty() || !kinds.insert(event.kind.as_str()) {
                return bad(format!("the event \"{}\" is there twice or has no kind", event.kind));
            }
            if !scopes.contains(event.scope.as_str()) {
                return bad(format!("the event \"{}\" needs a scope it doesn't have, \"{}\"", event.kind, event.scope));
            }
        }
        Ok(())
    }

    pub(crate) fn scope(&self, id: &str) -> Option<&ScopeSpec> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    /// Whether scope `scope`'s tools can hand an agent what the app reads
    /// (its manifest says, or else: it asks to read anything at all).
    pub(crate) fn reads_data(&self, scope: &ScopeSpec) -> bool {
        scope.reads_data.unwrap_or(!self.data.is_empty())
    }

    pub(crate) fn event(&self, kind: &str) -> Option<&EventSpec> {
        self.events.iter().find(|event| event.kind == kind)
    }

    pub(crate) fn secret(&self, name: &str) -> Option<&SecretSpec> {
        self.secrets.iter().find(|secret| secret.name == name)
    }

    /// The view `id`; empty for its first.
    pub(crate) fn view(&self, id: &str) -> Option<&ViewSpec> {
        if id.is_empty() { self.views.first() } else { self.views.iter().find(|view| view.id == id) }
    }
}

/// Where a bundle comes from, and how far it's trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    Claktory,
    Local,
}

/// A bundle read and checked: its signature holds and its manifest is valid.
pub(crate) struct Bundle {
    pub manifest: Manifest,
    pub manifest_text: String,
    pub component: Vec<u8>,
    /// The publisher's key, base64.
    pub key: String,
    pub tier: Tier,
    /// The whole file's SHA-256, hex.
    pub sha256: String,
}

impl Bundle {
    /// Reads a .clapp's bytes; why not, a sentence for the person.
    pub(crate) fn read(bytes: &[u8]) -> Result<Bundle, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!("This file is too big for an app (over {} MB).", MAX_BYTES / 1024 / 1024));
        }
        let sha256 = hex(&Sha256::digest(bytes));
        let files = untar(bytes)?;
        let file = |name: &str| files.get(name).ok_or_else(|| format!("This file isn't an app: it has no {name}."));
        let signature = std::str::from_utf8(file("signature")?).map_err(|_| "Claktory can't read this app's signature.".to_owned())?;
        let (key, signed) = parse_signature(signature)?;
        let mut message = SIGNED_PREFIX.to_owned();
        for (name, content) in files.iter().filter(|(name, _)| *name != "signature") {
            message.push_str(&format!("{}  {name}\n", hex(&Sha256::digest(content))));
        }
        let verifying = ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|_| "This app's signature isn't valid.".to_owned())?;
        if verifying.verify_strict(message.as_bytes(), &ed25519_dalek::Signature::from_bytes(&signed)).is_err() {
            return Err("This app was changed after it was signed, so Claktory won't install it.".to_owned());
        }
        let manifest_text = String::from_utf8(file("manifest.toml")?.clone())
            .map_err(|_| "This app's manifest can't be read: it isn't text.".to_owned())?;
        let manifest = Manifest::parse(&manifest_text)?;
        let component = file("app.wasm")?.clone();
        if !component.starts_with(b"\0asm") {
            return Err("This app's code (app.wasm) isn't WebAssembly.".to_owned());
        }
        let key = STANDARD.encode(key);
        let tier = tier_of(&key);
        Ok(Bundle { manifest, manifest_text, component, key, tier, sha256 })
    }
}

/// How far a publisher's key (base64) is trusted.
pub(crate) fn tier_of(key: &str) -> Tier {
    if CLAKTORY_KEYS.contains(&key) { Tier::Claktory } else { Tier::Local }
}

/// How a key shows: "ed25519:" and its first 16 base64 characters.
pub(crate) fn short_key(key: &str) -> String {
    format!("ed25519:{}", key.chars().take(16).collect::<String>())
}

fn parse_signature(text: &str) -> Result<([u8; 32], [u8; 64]), String> {
    let bad = || "Claktory can't read this app's signature.".to_owned();
    let mut key = None;
    let mut sig = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("key ") {
            let encoded = rest.trim().strip_prefix("ed25519:").ok_or_else(bad)?;
            key = Some(STANDARD.decode(encoded).map_err(|_| bad())?);
        } else if let Some(rest) = line.strip_prefix("sig ") {
            sig = Some(STANDARD.decode(rest.trim()).map_err(|_| bad())?);
        }
    }
    let key: [u8; 32] = key.ok_or_else(bad)?.try_into().map_err(|_| bad())?;
    let sig: [u8; 64] = sig.ok_or_else(bad)?.try_into().map_err(|_| bad())?;
    Ok((key, sig))
}

/// The regular files of a ustar archive, by name; only flat names.
fn untar(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let bad = |why: &str| format!("This file isn't an app: {why}.");
    let mut files = BTreeMap::new();
    let mut at = 0;
    while at + 512 <= bytes.len() {
        let header = &bytes[at..at + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let field = |range: std::ops::Range<usize>| {
            let raw = &header[range];
            let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).trim().to_owned()
        };
        if !header[257..262].eq(b"ustar") {
            return Err("This file isn't an app.".to_owned());
        }
        let size = usize::from_str_radix(&field(124..136), 8).map_err(|_| "This file is damaged. Download it again.".to_owned())?;
        let (prefix, name) = (field(345..500), field(0..100));
        let name = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let name = name.strip_prefix("./").unwrap_or(&name).to_owned();
        let start = at + 512;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "This file is incomplete. Download it again.".to_owned())?;
        match header[156] {
            b'0' | 0 => {
                if name.is_empty() || name.contains('/') || name.starts_with('.') {
                    return Err(bad(&format!("it contains \"{name}\", which apps can't have")));
                }
                if files.insert(name.clone(), bytes[start..end].to_vec()).is_some() {
                    return Err(bad(&format!("it contains \"{name}\" twice")));
                }
            }
            b'5' if name.trim_end_matches('/') == "." || name.is_empty() => {}
            _ => return Err(bad(&format!("it contains \"{name}\", which apps can't have"))),
        }
        at = start + size.div_ceil(512) * 512;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claktorys_apps_key_is_the_vaults_and_only_it_is_claktorys() {
        // The vault's public key as ssh shows it: its last 32 bytes are the key.
        let ssh = STANDARD.decode("AAAAC3NzaC1lZDI1NTE5AAAAII5l9hw1FpwDhaXLFH/NhQkOYKrjZSBr6fKotr5j8vaH").expect("base64");
        let key = STANDARD.encode(&ssh[ssh.len() - 32..]);
        assert_eq!(CLAKTORY_KEYS, [key.as_str()]);
        assert_eq!(tier_of(&key), Tier::Claktory);
        assert_eq!(short_key(&key), "ed25519:jmX2HDUWnAOFpcsU");
        let other = STANDARD.encode([9u8; 32]);
        assert_eq!(tier_of(&other), Tier::Local);
        assert_eq!(tier_of(""), Tier::Local);
    }

    #[test]
    fn zoes_telegram_manifest_reads() {
        let text = r#"
id = "telegram"
name = "Telegram"
version = "0.1.0"
world = "claktory:app@1"
icon = "send"
hosts = ["api.telegram.org"]
instructions = "Hi."

[[secrets]]
name = "bot_token"
label = "Bot token"

[[scopes]]
id = "send"
label = "Send"
tools = ["telegram_send"]

[[events]]
kind = "message"
scope = "send"

[[tools]]
name = "telegram_send"
parameters = { type = "object", required = ["text"], properties = { text = { type = "string" } } }
"#;
        let manifest = Manifest::parse(text).expect("valid");
        assert_eq!(manifest.tools[0].parameters["properties"]["text"]["type"], "string");
        assert_eq!((manifest.memory_mb, manifest.storage_mb), (32, 5));
        let wrong = text.replace("tools = [\"telegram_send\"]", "tools = [\"telegram_sned\"]");
        assert!(Manifest::parse(&wrong).unwrap_err().contains("gives a tool it doesn't have"));
        let world = text.replace("claktory:app@1", "claktory:app@2");
        assert!(Manifest::parse(&world).unwrap_err().contains("needs a different version of Claktory"));
        let host = text.replace("api.telegram.org", "http://127.0.0.1:8123");
        assert!(Manifest::parse(&host).unwrap_err().contains("isn't a host name"));

        for name in [
            "Claktory",
            "claktory vault",
            "Clak-tory Security",
            "Сlaktory",
            "CIaktory",
            "C1aktory",
            "CLAKT0RY",
            "Telegram\u{200b}",
            " Telegram",
            "Tele/gram",
        ] {
            let named = text.replace("name = \"Telegram\"", &format!("name = \"{name}\""));
            assert!(Manifest::parse(&named).is_err(), "{name} refused");
        }
        let named = text.replace("name = \"Telegram\"", "name = \"Charts 2-day\"");
        assert_eq!(Manifest::parse(&named).expect("a plain name").name, "Charts 2-day");

        let views = format!("{text}\n[[views]]\nid = \"main\"\ntitle = \"Charts\"\n\n[[views]]\nid = \"day-by-day\"\ntitle = \"Days\"\n");
        assert!(Manifest::parse(&views).unwrap_err().contains("views need world = \"claktory:app@1.1\""));
        let views = views.replace("claktory:app@1\"", "claktory:app@1.1\"");
        let manifest = Manifest::parse(&views).expect("valid with views");
        assert_eq!(manifest.view("").map(|view| view.title.as_str()), Some("Charts"));
        assert_eq!(manifest.view("day-by-day").map(|view| view.title.as_str()), Some("Days"));
        assert!(manifest.view("nope").is_none());
        let twice = views.replace("day-by-day", "main");
        assert!(Manifest::parse(&twice).unwrap_err().contains("the view \"main\" is there twice"));
        let untitled = views.replace("title = \"Days\"", "title = \" \"");
        assert!(Manifest::parse(&untitled).unwrap_err().contains("has no title"));
        for title in ["Claktory Settings", "CIaktory", "Days\u{200b}", "Days: all", " Days"] {
            let titled = views.replace("title = \"Days\"", &format!("title = \"{title}\""));
            assert!(Manifest::parse(&titled).is_err(), "{title} refused");
        }
        let named = views.replace("day-by-day", "Day By Day");
        assert!(Manifest::parse(&named).unwrap_err().contains("lowercase letters, digits, - and _"));

        let data = "\n[[data]]\nkind = \"factory-records\"\nreason = \"Charts of your usage\"\n\n[[data]]\nkind = \"agent-database\"\nagent = \"*\"\n";
        assert!(Manifest::parse(&format!("{text}{data}")).unwrap_err().contains("[[data]] needs world = \"claktory:app@1.1\""));
        let manifest = Manifest::parse(&format!("{views}{data}")).expect("valid with data");
        let kinds: Vec<(&str, &str)> = manifest.data.iter().map(|data| (data.kind.as_str(), data.agent.as_str())).collect();
        assert_eq!(kinds, [("factory-records", ""), ("agent-database", "*")]);
        for (wrong, why) in [
            ("kind = \"agent-database\"\n", "needs agent = \"*\" or an agent's id"),
            ("kind = \"factory-records\"\nagent = \"lea\"\n", "factory-records"),
            ("kind = \"factory-records\"\n", "is there twice"),
            ("kind = \"files\"\n", "isn't activity, conversations, agent-database or file"),
            ("kind = \"file\"\nagent = \"lea\"\n", "file names no agent"),
            ("kind = \"activity\"\nagent = \"lea\"\n", "activity names no agent"),
        ] {
            let error = Manifest::parse(&format!("{views}{data}\n[[data]]\n{wrong}")).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
        }
    }
}
