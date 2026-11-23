//! The Move extension (#243 S1): Rivet moves this Claktory to another
//! computer for the person, as Settings → Move does, over the same machinery
//! (`crate::move_run`). `move_claktory` starts the move with the code
//! `factoryd receive` printed there and returns the digits both computers
//! show; `move_status` says where it stands; `move_cancel` calls it off.
//! Only the person confirms the digits, in Claktory's own confirm, whose
//! words no agent writes (Fugo's review: a move hands over everything, so
//! an injected Rivet mustn't be able to word a question that gets the yes);
//! the move says an agent started it (`Move.started_by_agent_id`) so every
//! device shows that confirm. Only Rivet (`proposals::rivet`, the settings'
//! admin agent, never a tool argument) can use them, never while it has
//! read a web page, and nothing here deletes the copy a move leaves behind:
//! that stays the person's.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, MovePhase as Phase};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection, async_tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};

pub(crate) const ID: &str = "move";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Move",
    icon: "send",
    description: "Lets Rivet move this Claktory to another computer for you. You confirm the move and its digits in Claktory itself; Rivet can't confirm it for you.",
    default_enabled: false,
    options: &[],
};

/// How long move_claktory waits for the other computer's digits.
const CONNECT_WAIT: Duration = Duration::from_secs(90);

const START: &str = "Start moving this Claktory to another computer, when the person asks: code, the receive code `factoryd receive` printed there (claktory-move://…). It connects and returns the digits both computers show, or why it can't, and where project folders outside the person's home folder will go there. Claktory then asks the person itself to confirm the move; nothing is copied until they have, and you can't confirm it for them. skip: project folders the person wants left out of the move, by their full paths (a folder holding a project's folder leaves that out too).";
const STATUS: &str = "Where the move stands: its phase, the other computer, the digits, what it's copying and any problem.";
const REPORT: &str = "On the new computer, once Claktory runs there after a move: how it went, in plain words, to tell the person before you say the move is done. It says whether its secrets are unlocked (or still locked), whether the account checked in, which devices connected since, whether every project folder is in place, whether each sign-in still works (a refused one shows Sign in again), the tools the agents used that aren't here, and what of each project needs a Mac. Ask for it again once they're unlocked if it said they were locked.";
const CANCEL: &str = "Call off the move, or dismiss one that failed. Use it when the digits don't match, or when the person asks; it can't be called off once the other computer is taking over.";

/// How Rivet moves this Claktory to a server (Pericolo's tested recipe,
/// #243 S2), with Fugo's S1 rule: only the person confirms, in Claktory's
/// own confirm.
const RECIPE: &str = r#"When {person} asks you to move this Claktory to another computer, such as a server, follow this exactly. Only {person} confirms the move: Claktory itself asks them, on every device, naming the other computer and the digits. Never write that question yourself, with ask_person or otherwise; you can't confirm the move, and you never answer for them. Nothing you can do deletes the copy left here: that's {person}'s to do.

KEY below is the file of an SSH key the vault generated for it (secrets_request with generate: "ssh-ed25519"; {person} adds its public half to the server). Name its secret on every command.

1. First contact records the server's host key; tell {person} its fingerprint. After that never turn host checking off: if the key changes, ssh refuses, and you tell {person} and stop.
   ssh -i "$KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new root@SERVER true && ssh-keygen -l -F SERVER
2. The receive side, as one background task (run_in_background, timeout up to 7200). Its log is receive's JSON lines. Keep `|| s=$?`, or a refusal hangs.
   M="$HOME/.claktory-move"; mkdir -p "$M"; : > "$M/answers"; rm -f "$M/done"
   { tail -n +1 -f "$M/answers" & t=$!
     while [ ! -e "$M/done" ]; do sleep 1; done
     kill "$t" 2>/dev/null || true; } |
   { s=0; ssh -i "$KEY" -o IdentitiesOnly=yes -o ServerAliveInterval=30 root@SERVER 'bash -c "$(curl -fsSL https://claktory.com/install.sh)" -- --receive --json' || s=$?
     touch "$M/done"; exit "$s"; }
3. The log's first {"event":"code",…} line's code goes to move_claktory. Compare the digits it returns with the log's {"event":"confirm","digits":…} line: if they differ, call move_cancel at once, since something is between the two computers. If they match, tell {person} that Claktory is asking them to confirm the move, on any of their devices. Then wait, following move_status: once it says {person} confirmed the move here, write their yes, `echo yes >> "$HOME/.claktory-move/answers"`; if they called it off (move_status says no move is under way), write `echo no`. Never before a confirm line, and never your own judgement. A new code line after a no is a fresh code.
4. The task's last line is {"ok":true,…} (Claktory runs on the server) or {"ok":false,…}: not moved (exit 3 refused, or "error":"called_off" when the move was called off after the confirm; 1 failed; 130 stopped). Then nothing runs on the server: tell {person} why, in its message. Running step 2 again resumes a move that stopped. To stop it on purpose, stop receive on the server, not the local task: ssh -i "$KEY" -o IdentitiesOnly=yes root@SERVER 'pkill -TERM -f "factoryd receive"'
5. At the freeze your run stops like every agent's, and you carry on on the server. There, call move_report before you tell {person} the move is done, and pass on what it says: the copy left behind can still be gone back to while something is wrong. If it says the secrets are locked, call it again once they're unlocked, to check the sign-ins.
"#;

pub(crate) struct Move;

impl Extension for Move {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        let text = format!("## Moving this Claktory\n\n{}", RECIPE.replace("{person}", person));
        vec![PromptSection { kind: PromptKind::Static, text }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let code = json!({
            "type": "object",
            "required": ["code"],
            "properties": {
                "code": {"type": "string", "minLength": 1},
                "skip": {"type": "array", "items": {"type": "string", "minLength": 1}},
            },
            "additionalProperties": false,
        });
        let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
        vec![
            async_tool(factory, agent, "move_claktory", "Move this Claktory", START, code, start),
            super::tool("move_status", "Move status", STATUS, none.clone(), super::typed_handler(factory, agent, status)),
            super::tool("move_cancel", "Call off the move", CANCEL, none.clone(), super::typed_handler(factory, agent, cancel)),
            async_tool(factory, agent, "move_report", "Check the moved Claktory", REPORT, none, report),
        ]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

/// Only Rivet moves this Claktory: the settings' admin agent, whoever calls.
fn only_rivet(factory: &Factory, agent: &str) -> Result<(), String> {
    let inner = factory.lock();
    super::caller(&inner.config, agent)?;
    if crate::proposals::rivet(&inner.config).is_some_and(|rivet| rivet.id == agent) {
        Ok(())
    } else {
        Err("Only Rivet, the admin agent, moves this Claktory. Nothing was done.".to_owned())
    }
}

/// An agent that has read a web page doesn't start a move: what it read may be
/// someone else's instructions, and a move hands over everything.
fn untainted(factory: &Factory, agent: &str) -> Result<(), String> {
    if factory.pipes.tainted(agent).is_empty() {
        Ok(())
    } else {
        let person = factory.person_capitalized();
        let read = crate::pipes::agents::what_was_read(factory, agent);
        Err(format!(
            "You've read {read}, so you can't start a move: what you read there may be someone else's instructions. {person} can start the move from Settings → Move. Nothing was done."
        ))
    }
}

#[derive(Deserialize)]
struct StartArgs {
    code: String,
    #[serde(default)]
    skip: Vec<String>,
}

/// The folders to leave out, as full paths ("~" expanded), each holding at
/// least one of the projects' folders.
fn skipped(factory: &Factory, skip: &[String]) -> Result<Vec<String>, String> {
    let folders: Vec<std::path::PathBuf> = {
        let inner = factory.lock();
        crate::move_report::projects_of(&inner.config).into_iter().flat_map(|(_, paths)| paths).collect()
    };
    let user_home = std::env::home_dir().unwrap_or_default();
    let mut kept = Vec::new();
    for given in skip {
        let given = given.trim().trim_end_matches('/');
        let path = match given.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => user_home.join(rest.trim_start_matches('/')),
            _ => std::path::PathBuf::from(given),
        };
        if !path.is_absolute() {
            return Err(format!(
                "Name each folder to leave out by its full path, such as /Volumes/Data/Assets; \"{given}\" isn't one. Nothing was done."
            ));
        }
        if !folders.iter().any(|folder| folder.starts_with(&path)) {
            return Err(format!(
                "{} holds none of the projects' folders, so there's nothing there to leave out. Nothing was done.",
                path.display()
            ));
        }
        kept.push(path.display().to_string());
    }
    Ok(kept)
}

/// The planned folders outside this computer's home folders, and where
/// each goes, for move_claktory's answer.
fn outside_folders(factory: &Factory, view: &v1::Move) -> Vec<String> {
    let user_home = std::env::home_dir().unwrap_or_default();
    let home = factory.home.root();
    view.map
        .iter()
        .filter(|path| {
            let from = std::path::Path::new(&path.from);
            !from.starts_with(home) && !from.starts_with(&user_home)
        })
        .map(|path| format!("- {} → {}", path.from, path.to))
        .collect()
}

#[derive(Deserialize)]
struct NoArgs {}

/// move_claktory: StartMove, then the digits once the other computer
/// answers (or why not).
async fn start(factory: Arc<Factory>, agent: String, args: StartArgs) -> Result<ToolResult, String> {
    only_rivet(&factory, &agent)?;
    untainted(&factory, &agent)?;
    let skip = skipped(&factory, &args.skip)?;
    crate::move_run::start_by(&factory, args.code.trim(), &agent, skip.clone()).map_err(|error| error.to_string())?;
    let until = Instant::now() + CONNECT_WAIT;
    loop {
        let view = factory.moving.view().unwrap_or_default();
        match view.phase() {
            Phase::Connecting if Instant::now() < until => tokio::time::sleep(Duration::from_millis(200)).await,
            Phase::Connecting => {
                return Ok(said(format!(
                    "Still connecting to {}. Check that factoryd receive still runs there; move_status says when it answers.",
                    view.target_machine
                )));
            }
            Phase::Failed => return Err(view.error),
            Phase::Confirm => {
                let (person, projects) = {
                    let inner = factory.lock();
                    (inner.config.person().to_owned(), crate::move_report::projects_of(&inner.config))
                };
                let machine = &view.target_machine;
                let platform = crate::move_report::platform_lines(&projects, &view.target_os, false);
                let mut text = format!(
                    "Connected to {machine} ({}, {}, Claktory {}). Both computers show {}.\n\nCompare them with the digits factoryd receive printed on {machine}. If they differ, call move_cancel now. If they match, tell {person} that Claktory is asking them to confirm the move, on any of their devices, and to check the digits there; you can't confirm it, and don't ask them yourself. Once move_status says they confirmed it, answer yes to receive there.\n\nWhat to tell {person} about their projects on {machine}:\n{platform}",
                    view.target_os, view.target_arch, view.target_version, view.digits
                );
                let outside = outside_folders(&factory, &view);
                if !outside.is_empty() {
                    let _ = write!(
                        text,
                        "\n\nThese folders are outside {person}'s home folder here, so on {machine} they go under its home folder, keeping their paths:\n{}\nTell {person} before they confirm. To leave one out instead, call move_cancel (answer no to receive if it's yours to answer, as for any move called off; it then shows a new code), then call move_claktory again with that new code and skip listing the folder.",
                        outside.join("\n")
                    );
                }
                if !skip.is_empty() {
                    let _ = write!(text, "\n\nLeft out of the move, as asked: {}.", skip.join("; "));
                }
                return Ok(said(text));
            }
            // A move a restart interrupted, picked up where it was: its
            // digits were confirmed then.
            _ => return Ok(said(describe(&factory, &view))),
        }
    }
}

fn said(text: String) -> ToolResult {
    ToolResult::text(text, json!({}))
}

/// move_status.
fn status(factory: &Arc<Factory>, agent: &str, _args: NoArgs) -> Result<ToolResult, String> {
    only_rivet(factory, agent)?;
    Ok(said(factory.moving.view().map_or_else(|| "No move is under way.".to_owned(), |view| describe(factory, &view))))
}

/// The move as the agent reads it: where it stands, in Claktory's words.
fn describe(factory: &Factory, view: &v1::Move) -> String {
    let machine = &view.target_machine;
    let (person, them) = {
        let inner = factory.lock();
        (inner.config.person_capitalized().to_owned(), inner.config.person().to_owned())
    };
    let phase = match view.phase() {
        Phase::Connecting => format!("Connecting to {machine}."),
        Phase::Confirm if factory.moving.confirmed_here() => {
            format!("{person} confirmed the move here: answer yes to receive on {machine}, which waits for it.")
        }
        Phase::Confirm => format!("Waiting for {them} to confirm the move in Claktory, where they check the digits."),
        Phase::Checking => format!("{machine} is checking it can take this Claktory."),
        Phase::Copying => format!("Copying to {machine} while agents keep working."),
        Phase::Frozen => "Frozen for the last round: agents are stopped.".to_owned(),
        Phase::Importing => format!("{machine} is setting it up."),
        Phase::Committing => format!("{machine} is taking over; it can't be called off now."),
        Phase::Done => format!("Moved to {machine}."),
        Phase::Failed => format!("It failed: {}", view.error),
        Phase::Unspecified => "No move is under way.".to_owned(),
    };
    let mut text = phase;
    if !view.target_os.is_empty() {
        let _ =
            write!(text, "\nThe other computer: {machine} ({}, {}, Claktory {}).", view.target_os, view.target_arch, view.target_version);
    }
    if !view.digits.is_empty() {
        let _ = write!(text, "\nThe digits: {}.", view.digits);
    }
    if !view.step_text.is_empty() && view.phase() != Phase::Failed {
        let _ = write!(text, "\nNow: {}", view.step_text);
    }
    if view.bytes_total > 0 {
        let _ = write!(text, "\nCopied: {} of {} bytes (round {}).", view.bytes_have, view.bytes_total, view.round);
    }
    if !view.problems.is_empty() {
        let _ = write!(text, "\nProblems: {}", view.problems.join(" "));
    }
    if !view.missing_tools.is_empty() {
        let _ = write!(text, "\nCommands agents use that {machine} doesn't have: {}.", view.missing_tools.join(", "));
    }
    for path in &view.map {
        let _ = write!(text, "\nFolder: {} → {}", path.from, path.to);
    }
    text
}

/// move_report: on the new computer, how the move went.
async fn report(factory: Arc<Factory>, agent: String, _args: NoArgs) -> Result<ToolResult, String> {
    only_rivet(&factory, &agent)?;
    Ok(said(crate::move_report::report(&factory).await))
}

/// move_cancel.
fn cancel(factory: &Arc<Factory>, agent: &str, _args: NoArgs) -> Result<ToolResult, String> {
    only_rivet(factory, agent)?;
    let failed = factory.moving.view().is_some_and(|view| view.phase() == Phase::Failed);
    crate::move_run::cancel(factory).map_err(|error| error.to_string())?;
    Ok(said(if failed {
        "Dismissed the failed move.".to_owned()
    } else {
        "Called off the move: this Claktory stays as it was.".to_owned()
    }))
}
