//! The Move extension: Rivet helps move this Claktory to another computer for
//! the person, over the same machinery as Settings → Move
//! (`crate::move_run`). No agent starts or confirms a move: the
//! other computer asks the person's Claktory account to receive this Claktory
//! (`factoryd receive --email`), the person approves it themselves on one of
//! their devices connected here, in Settings → Move, and this Claktory starts
//! it once it has checked that device's own approval (`account::moves`). No
//! code and no digits exist for an agent to see or relay. `move_claktory`
//! says how (the email, the command), `move_status` where it stands,
//! `move_cancel` calls it off, `move_report` checks the moved Claktory. Only
//! Rivet (`proposals::rivet`, the settings' admin agent, never a tool
//! argument) can use them, and nothing here deletes the copy a move leaves
//! behind: that stays the person's. Without an account, the person moves it
//! themselves (a receive code typed into the app).

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, MovePhase as Phase};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptSection, async_tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};

pub(crate) const ID: &str = "move";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Move",
    icon: "send",
    description: "Lets Rivet set up a move of this Claktory to another computer for you. You approve the move yourself, in Settings → Move on one of your devices; Rivet can't approve it for you.",
    default_enabled: false,
    options: &[],
};

const START: &str = "How to move this Claktory to another computer, when the person asks: the email of the Claktory account it's signed in to, the command that receives it on the other computer, and what the person does to approve it. You can't start or approve a move yourself: the person approves it on one of their devices, and Claktory then starts it by itself. Without an account, it says the person moves it themselves.";
const STATUS: &str = "Where the move stands: its phase, the other computer, which device approved it, what it's copying and any problem.";
const REPORT: &str = "On the new computer, once Claktory runs there after a move: how it went, in plain words, to tell the person before you say the move is done. It says whether its secrets are unlocked (or still locked), whether the account checked in, which devices connected since, whether every project folder is in place, whether each sign-in still works (a refused one shows Sign in again), the tools the agents used that aren't here, and what of each project needs a Mac. Ask for it again once they're unlocked if it said they were locked.";
const CANCEL: &str = "Call off the move, or dismiss one that failed. Use it when the person asks; it can't be called off once the other computer is taking over.";

/// How Rivet sets up a move of this Claktory to a server (Pericolo's tested
/// recipe), with the rule that no agent starts or approves a move; the
/// person approves it on their own device.
const RECIPE: &str = r#"When {person} asks you to move this Claktory to another computer, such as a server, follow this exactly. Only {person} approves the move, in Settings → Move on one of their devices connected to this Claktory, where Claktory shows what the account service saw of the other computer. You can't approve or start it, and there's no code or digits to pass on. Don't ask for the approval with ask_person or otherwise: only tell {person} where to give it. Nothing you can do deletes the copy left here: that's {person}'s to do.

First call move_claktory: it gives EMAIL, the Claktory account this Claktory is signed in to, or says it isn't signed in, in which case tell {person} they move it themselves (Settings → General → Move to another computer…) and stop.

KEY below is the file of an SSH key the vault generated for it (secrets_request with generate: "ssh-ed25519"; {person} adds its public half to the server). Name its secret on every command.

1. First contact records the server's host key; tell {person} its fingerprint. After that never turn host checking off: if the key changes, ssh refuses, and you tell {person} and stop.
   ssh -i "$KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new root@SERVER true && ssh-keygen -l -F SERVER
2. Start receive on the server, detached there, so that nothing here can stop it: not the freeze, when your own tasks stop, and not a dropped connection. It writes receive's JSON lines to ~/.claktory-move/receive.log there. Put EMAIL in it, and run it as it is, with its lines unindented. If receive already runs there, it says so and starts nothing.
ssh -i "$KEY" -o IdentitiesOnly=yes root@SERVER sh -s <<'EOF'
M="$HOME/.claktory-move"; mkdir -p "$M"
if pgrep -f claktory-move/receive.sh >/dev/null; then echo "receive already runs"; exit 0; fi
rm -f "$M/exit"
cat > "$M/receive.sh" <<'END'
M="$HOME/.claktory-move"
s=0; bash -c "$(curl -fsSL https://claktory.com/install.sh)" -- --receive --email EMAIL --json < /dev/null || s=$?
echo "$s" > "$M/exit"
END
command -v loginctl >/dev/null && loginctl enable-linger "$(id -un)" 2>/dev/null
if command -v setsid >/dev/null; then d=setsid; else d=; fi
nohup $d sh "$M/receive.sh" > "$M/receive.log" 2>&1 < /dev/null &
echo started
EOF
3. Follow it as one background task (run_in_background, timeout up to 7200). Its log is receive's JSON lines, and it ends when receive does, with receive's exit status. If this task stops (at the freeze, or when the connection drops), receive carries on there; run it again to follow it, from the first line.
   ssh -i "$KEY" -o IdentitiesOnly=yes -o ServerAliveInterval=30 root@SERVER 'M="$HOME/.claktory-move"; tail -n +1 -f "$M/receive.log" & t=$!; while [ ! -e "$M/exit" ] && pgrep -f claktory-move/receive.sh >/dev/null; do sleep 2; done; sleep 1; kill "$t"; exit "$(cat "$M/exit" 2>/dev/null || echo 1)"'
4. Once the log says to approve the move (a progress line: "On a device connected to the Claktory you're moving, open Settings → Move…", then one with "this computer's code: XXXX XXXX XXXX XXXX"), tell {person}: on one of their devices connected to this Claktory, open Settings → Move and approve moving it to SERVER only if the code shown there is that code, word for word, and the address and network are their server's. It must be them, on their device. Then wait, following move_status: once they approve, this Claktory starts the move by itself, and move_status says which device approved it. If they decline, or it isn't approved within ten minutes, receive ends; run step 2 again when they want to try again.
5. The log's last line is {"ok":true,…} (Claktory runs on the server) or {"ok":false,…}: not moved (exit 3 refused, declined or called off; 1 failed; 130 stopped). Then nothing runs on the server: tell {person} why, in its message. Running step 2 again resumes a move that stopped, and step 3 follows it. To stop it on purpose, stop receive on the server: ssh -i "$KEY" -o IdentitiesOnly=yes root@SERVER 'pkill -TERM -f "factoryd receive"'. If receive stops while the server sets the moved Claktory up, this Claktory gives up after two minutes without an answer, carries on here as it was, and move_status says why.
6. At the freeze your run stops like every agent's, and so may your task from step 3; receive carries on on the server by itself. You carry on on the server: there, call move_report before you tell {person} the move is done, and pass on what it says: the copy left behind can still be gone back to while something is wrong. If it says the secrets are locked, call it again once they're unlocked, to check the sign-ins.
"#;

pub(crate) struct Move;

impl Extension for Move {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        let text = format!("## Moving this Claktory\n\n{}", RECIPE.replace("{person}", person));
        vec![PromptSection::static_text(text)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
        vec![
            super::tool("move_claktory", "Move this Claktory", START, none.clone(), super::typed_handler(factory, agent, start)),
            super::tool("move_status", "Move status", STATUS, none.clone(), super::typed_handler(factory, agent, status)),
            super::tool("move_cancel", "Call off the move", CANCEL, none.clone(), super::typed_handler(factory, agent, cancel)),
            async_tool(factory, agent, "move_report", "Check the moved Claktory", REPORT, none, report),
        ]
    }

    fn admin_default(&self) -> bool {
        true
    }

    /// Moving isn't on Windows yet (`move_run::NOT_ON_WINDOWS`).
    fn hidden(&self) -> bool {
        cfg!(windows)
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

#[derive(Deserialize)]
struct NoArgs {}

/// move_claktory: how the move goes, with this Claktory's account; no move
/// starts here.
fn start(factory: &Arc<Factory>, agent: &str, _args: NoArgs) -> Result<ToolResult, String> {
    only_rivet(factory, agent)?;
    let (person, config) = {
        let inner = factory.lock();
        (inner.config.person().to_owned(), inner.config.account.clone())
    };
    let Some((_, email, _)) = factory.account.signed_in_as(&config) else {
        return Ok(said(format!(
            "This Claktory isn't signed in to a Claktory account, so no move of it can be approved from a device: {person} moves it themselves, from Settings → General → Move to another computer…, typing the code factoryd receive shows on the other computer. You can't do it for them. Nothing was done."
        )));
    };
    if let Some(view) = factory.moving.view().filter(|view| !matches!(view.phase(), Phase::Failed | Phase::Unspecified)) {
        return Ok(said(format!("A move is already under way.\n{}", describe(factory, &view))));
    }
    Ok(said(format!(
        "EMAIL is {email}. On the other computer, run receive with it (install.sh -- --receive --email {email} --json, or factoryd receive --email {email} --json where Claktory is installed). It asks {person}'s Claktory account to receive this Claktory and prints no code. Then {person} approves it in Settings → Move on one of their devices connected to this Claktory, checking that the other computer's address and network shown there are theirs. Once they do, this Claktory starts the move by itself; follow it with move_status. You can't approve it, and nothing has started yet."
    )))
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
        Phase::Confirm => format!("Waiting for {them} to check the digits in Claktory and confirm the move there."),
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
    if !view.approved_by_name.is_empty() {
        let _ = write!(text, "\n{person} approved it on {}.", view.approved_by_name);
    }
    if !view.target_os.is_empty() {
        let _ =
            write!(text, "\nThe other computer: {machine} ({}, {}, Claktory {}).", view.target_os, view.target_arch, view.target_version);
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
