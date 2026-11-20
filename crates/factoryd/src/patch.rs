//! Workspaces on one connection: the first goes whole, each later one only
//! as what changed (WorkspacePatch) to apps that asked for that, and one
//! with nothing new not at all. A busy Claktory changes one agent at a time,
//! and its Workspace (every agent's prompt, the changelog, the map) is far
//! bigger than the change, which matters most over the tunnel.
//!
//! It works on the encoded bytes, field by field, so a field added to
//! Workspace needs nothing here: prost always encodes the same state to the
//! same bytes (maps are BTreeMaps for that).

use std::collections::{BTreeMap, HashMap};

use bytes::Bytes;
use factory_protocol::v1::{self, ServerMessage, server_message::Body};
use prost::Message as _;

/// ServerMessage.workspace.
const WORKSPACE: u32 = 5;
/// Workspace.agents.
const AGENTS: u32 = 2;
/// Agent.id.
const AGENT_ID: u32 = 1;
const LENGTH_DELIMITED: u8 = 2;

/// What one connection was last sent.
pub(crate) struct Workspaces {
    /// The app applies patches (Hello.workspace_patches).
    patches: bool,
    last: Option<Parsed>,
}

impl Workspaces {
    pub(crate) fn new(patches: bool) -> Self {
        Self { patches, last: None }
    }

    /// Starts over: the next Workspace goes whole (after a resync).
    pub(crate) fn reset(&mut self) {
        self.last = None;
    }

    /// What to send for `frame`, an encoded ServerMessage: itself, a patch
    /// in its place, or nothing when it's a Workspace the app already has.
    pub(crate) fn outgoing(&mut self, frame: Bytes) -> Option<Bytes> {
        let Some(workspace) = workspace_in(&frame) else { return Some(frame) };
        let Some(next) = Parsed::new(&workspace) else {
            // Not something this can compare; whole is always right.
            self.last = None;
            return Some(frame);
        };
        let Some(last) = self.last.replace(next) else { return Some(frame) };
        let next = self.last.as_ref().expect("just set");
        if last == *next {
            return None;
        }
        if !self.patches {
            return Some(frame);
        }
        match patch(&last, next) {
            Some(patch) => Some(patch),
            None => Some(frame),
        }
    }
}

/// A message's encoded fields: each field's occurrences (several for a
/// repeated one), keys included.
type Fields = BTreeMap<u32, Vec<Bytes>>;

/// A Workspace's encoded fields, and its agents in order: each one's id and
/// encoded Agent.
#[derive(PartialEq)]
struct Parsed {
    fields: Fields,
    agents: Vec<(String, Bytes)>,
}

impl Parsed {
    fn new(workspace: &Bytes) -> Option<Self> {
        let mut fields = Fields::new();
        let mut agents = Vec::new();
        for field in Field::all(workspace)? {
            if field.number == AGENTS && field.wire_type == LENGTH_DELIMITED {
                let agent = workspace.slice(field.value.clone());
                let id = Field::all(&agent)?
                    .into_iter()
                    .find(|field| field.number == AGENT_ID && field.wire_type == LENGTH_DELIMITED)
                    .map(|field| String::from_utf8_lossy(&agent[field.value]).into_owned())
                    .unwrap_or_default();
                agents.push((id, agent));
            } else {
                fields.entry(field.number).or_default().push(workspace.slice(field.whole));
            }
        }
        Some(Self { fields, agents })
    }
}

/// A message's fields by number.
fn fields_of(message: &Bytes) -> Option<Fields> {
    let mut fields = Fields::new();
    for field in Field::all(message)? {
        fields.entry(field.number).or_default().push(message.slice(field.whole));
    }
    Some(fields)
}

/// The numbers of the fields `next` has as `last` had them, and the encoded
/// rest of `next`. A field `next` doesn't have is in neither: it's empty now.
fn diff(last: &Fields, next: &Fields) -> (Vec<u32>, Vec<u8>) {
    let mut kept = Vec::new();
    let mut changed = Vec::new();
    for (number, occurrences) in next {
        if last.get(number) == Some(occurrences) {
            kept.push(*number);
        } else {
            occurrences.iter().for_each(|occurrence| changed.extend_from_slice(occurrence));
        }
    }
    (kept, changed)
}

/// `next` as a ServerMessage with a WorkspacePatch against `last`.
fn patch(last: &Parsed, next: &Parsed) -> Option<Bytes> {
    let (kept, changed) = diff(&last.fields, &next.fields);
    let mut workspace = v1::Workspace::decode(changed.as_slice()).ok()?;
    let before: HashMap<&str, &Bytes> = last.agents.iter().map(|(id, agent)| (id.as_str(), agent)).collect();
    let mut kept_agents = Vec::new();
    let mut kept_agent_fields = Vec::new();
    for (id, agent) in &next.agents {
        match before.get(id.as_str()) {
            Some(&old) if old == agent => {
                kept_agents.push(id.clone());
                workspace.agents.push(v1::Agent { id: id.clone(), ..Default::default() });
            }
            // Only what changed (its status, mostly), not its prompt again.
            Some(&old) if !id.is_empty() => {
                let (mut kept, changed) = diff(&fields_of(old)?, &fields_of(agent)?);
                kept.retain(|number| *number != AGENT_ID);
                let mut changed = v1::Agent::decode(changed.as_slice()).ok()?;
                changed.id.clone_from(id);
                workspace.agents.push(changed);
                kept_agent_fields.push(v1::KeptFields { id: id.clone(), kept });
            }
            _ => workspace.agents.push(v1::Agent::decode(agent.clone()).ok()?),
        }
    }
    let patch = v1::WorkspacePatch { workspace: Some(workspace), kept, kept_agents, kept_agent_fields };
    let message = ServerMessage { reply_to: 0, body: Some(Body::WorkspacePatch(patch)) };
    Some(message.encode_to_vec().into())
}

/// The Workspace in an encoded ServerMessage, if that's what it carries.
fn workspace_in(frame: &Bytes) -> Option<Bytes> {
    Field::all(frame)?
        .into_iter()
        .find(|field| field.number == WORKSPACE && field.wire_type == LENGTH_DELIMITED)
        .map(|field| frame.slice(field.value))
}

/// One field of an encoded message: where it is, key and all, and where its
/// value is (a length-delimited one's without its length).
struct Field {
    number: u32,
    wire_type: u8,
    whole: std::ops::Range<usize>,
    value: std::ops::Range<usize>,
}

impl Field {
    /// A message's top-level fields, in order; None if it isn't well formed.
    fn all(message: &[u8]) -> Option<Vec<Field>> {
        let mut fields = Vec::new();
        let mut at = 0;
        while at < message.len() {
            let start = at;
            let key = varint(message, &mut at)?;
            let number = u32::try_from(key >> 3).ok()?;
            let wire_type = (key & 7) as u8;
            let value_start;
            match wire_type {
                0 => {
                    value_start = at;
                    varint(message, &mut at)?;
                }
                1 => {
                    value_start = at;
                    at = at.checked_add(8)?;
                }
                LENGTH_DELIMITED => {
                    let length = usize::try_from(varint(message, &mut at)?).ok()?;
                    value_start = at;
                    at = at.checked_add(length)?;
                }
                5 => {
                    value_start = at;
                    at = at.checked_add(4)?;
                }
                // Groups (3, 4) aren't in the protocol.
                _ => return None,
            }
            if at > message.len() || number == 0 {
                return None;
            }
            fields.push(Field { number, wire_type, whole: start..at, value: value_start..at });
        }
        Some(fields)
    }
}

fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, prompt: &str) -> v1::Agent {
        v1::Agent { id: id.into(), name: id.to_uppercase(), prompt: prompt.into(), ..Default::default() }
    }

    fn workspace() -> v1::Workspace {
        v1::Workspace {
            agents: vec![agent("mike", "plans"), agent("sam", "builds"), agent("zoe", "draws")],
            version: "v0.1.4".into(),
            selected_agent_id: "sam".into(),
            changelog: vec![v1::ChangelogVersion { version: "v0.1.4".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    fn frame(workspace: &v1::Workspace) -> Bytes {
        ServerMessage { reply_to: 0, body: Some(Body::Workspace(workspace.clone())) }.encode_to_vec().into()
    }

    /// The encoded occurrences of `message`'s fields numbered in `kept`.
    fn kept_fields(message: &impl prost::Message, kept: &[u32]) -> Vec<u8> {
        let bytes = Bytes::from(message.encode_to_vec());
        let mut fields = Vec::new();
        for field in Field::all(&bytes).unwrap() {
            if kept.contains(&field.number) {
                fields.extend_from_slice(&bytes[field.whole]);
            }
        }
        fields
    }

    /// What an app does with a patch (the contract in the protocol).
    fn apply(last: &v1::Workspace, patch: v1::WorkspacePatch) -> v1::Workspace {
        let mut next = patch.workspace.unwrap();
        assert!(!patch.kept.contains(&AGENTS));
        next.merge(kept_fields(last, &patch.kept).as_slice()).unwrap();
        for agent in &mut next.agents {
            let old = last.agents.iter().find(|old| old.id == agent.id);
            if patch.kept_agents.contains(&agent.id) {
                *agent = old.unwrap().clone();
            } else if let Some(fields) = patch.kept_agent_fields.iter().find(|fields| fields.id == agent.id) {
                agent.merge(kept_fields(old.unwrap(), &fields.kept).as_slice()).unwrap();
            }
        }
        next
    }

    fn decode(frame: Bytes) -> Body {
        ServerMessage::decode(frame).unwrap().body.unwrap()
    }

    #[test]
    fn the_first_workspace_goes_whole_and_an_unchanged_one_not_at_all() {
        for patches in [false, true] {
            let mut sent = Workspaces::new(patches);
            assert!(matches!(decode(sent.outgoing(frame(&workspace())).unwrap()), Body::Workspace(_)));
            assert!(sent.outgoing(frame(&workspace())).is_none());
            sent.reset();
            assert!(matches!(decode(sent.outgoing(frame(&workspace())).unwrap()), Body::Workspace(_)));
        }
    }

    #[test]
    fn other_messages_pass_as_they_are() {
        let mut sent = Workspaces::new(true);
        let delta = ServerMessage { reply_to: 7, body: Some(Body::Delta(v1::EntryDelta { agent_id: "sam".into(), ..Default::default() })) };
        let bytes = Bytes::from(delta.encode_to_vec());
        assert_eq!(sent.outgoing(bytes.clone()), Some(bytes));
    }

    #[test]
    fn an_app_without_patches_gets_each_change_whole() {
        let mut sent = Workspaces::new(false);
        sent.outgoing(frame(&workspace()));
        let mut next = workspace();
        next.agents[1].prompt = "builds the backend".into();
        assert_eq!(decode(sent.outgoing(frame(&next)).unwrap()), Body::Workspace(next));
    }

    #[test]
    fn a_patch_carries_only_what_changed_and_applies_to_the_same_workspace() {
        let mut sent = Workspaces::new(true);
        let first = workspace();
        sent.outgoing(frame(&first));

        let mut next = first.clone();
        next.agents[1].prompt = "builds the backend".into();
        next.agents.push(agent("omar", "pages"));
        next.agents.remove(0);
        next.selected_agent_id = String::new();
        next.config_error = "bad".into();
        let Body::WorkspacePatch(patch) = decode(sent.outgoing(frame(&next)).unwrap()) else { panic!("not a patch") };
        assert_eq!(patch.kept, vec![9, 18]);
        assert_eq!(patch.kept_agents, vec!["zoe".to_owned()]);
        // Sam's prompt changed; its name didn't.
        assert_eq!(patch.kept_agent_fields, vec![v1::KeptFields { id: "sam".into(), kept: vec![3] }]);
        let carried = patch.workspace.as_ref().unwrap();
        assert!(carried.changelog.is_empty() && carried.version.is_empty());
        assert_eq!(carried.agents[0], v1::Agent { id: "sam".into(), prompt: "builds the backend".into(), ..Default::default() });
        assert_eq!(carried.agents[1], v1::Agent { id: "zoe".into(), ..Default::default() });
        assert_eq!(carried.agents[2], agent("omar", "pages"));
        assert_eq!(apply(&first, patch), next);

        // And the next patch is against what was just sent.
        let mut third = next.clone();
        third.changelog.clear();
        third.agents[2].name = String::new();
        let Body::WorkspacePatch(patch) = decode(sent.outgoing(frame(&third)).unwrap()) else { panic!("not a patch") };
        assert_eq!(patch.kept, vec![5, 9]);
        assert_eq!(patch.kept_agents, vec!["sam".to_owned(), "zoe".to_owned()]);
        // A field emptied is neither carried nor kept.
        assert_eq!(patch.kept_agent_fields, vec![v1::KeptFields { id: "omar".into(), kept: vec![4] }]);
        assert_eq!(apply(&next, patch), third);
    }

    #[test]
    fn maps_encode_the_same_every_time() {
        let mut options = BTreeMap::new();
        for id in ["a", "b", "c", "d", "e", "f"] {
            options.insert(id.to_owned(), v1::OptionValue { on: true, agent_ids: Vec::new() });
        }
        let extension = v1::AgentExtension { id: "x".into(), enabled: true, options };
        let mut sent = Workspaces::new(true);
        let mut workspace = workspace();
        workspace.agents[0].extensions = vec![extension];
        sent.outgoing(frame(&workspace));
        assert!(sent.outgoing(frame(&workspace.clone())).is_none());
    }
}
