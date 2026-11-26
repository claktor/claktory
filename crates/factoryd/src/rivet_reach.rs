//! What Rivet can do of what the app does (#423). Every request the app can
//! send is in `reach`, an exhaustive match with no wildcard, so a new request
//! doesn't build until someone says whether Rivet can do it: through the
//! settings file, through a tool, only the person on purpose, or only the
//! app for now (a gap, with who is closing it). For Rivet's way, the tests
//! check that the manual (docs/MANUAL.md) names the key or tool; for a gap,
//! that the manual says where to click.
//!
//! UpdateSettings and UpdateAgent are taken field by field, with no `..`,
//! since their fields differ: a new field doesn't build either.

use factory_protocol::v1::{self, client_message::Body};

/// How Rivet does what a request does.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
    /// Changes nothing: a read, what a connection shows, or moving bytes.
    View,
    /// Rivet edits these keys of the settings file, as dotted paths whose
    /// every part the manual names (`display.zoom`); empty for the whole
    /// file.
    Config(&'static [&'static str]),
    /// Rivet uses these tools, each named in the manual.
    Tool(&'static [&'static str]),
    /// Only the person, on purpose: why.
    Person(&'static str),
    /// Only the app, for now: who is closing it, and the words in the
    /// manual's "Only in the app" that tell the person where to click.
    // None is open now (#423 closed them all); a request only the app does
    // goes here, with whoever is giving Rivet a way.
    #[allow(dead_code)]
    Gap { owner: &'static str, click: &'static str },
}

use Reach::{Config, Person, Tool, View};

const SIGN_IN: &str = "the person's own accounts: only they sign in or out";
const VAULT: &str = "only the person adds secret values or sets the vault's protections";
const CARD: &str = "the person's answer to a card or a question";
const VOICE: &str = "the person's own words";
const DRAWING: &str = "the person's own drawing";
const CHAT: &str = "chats are the person's, and agents can't see them";
const BROWSER: &str = "Claktory's browser (its live view, turns, errands' log, who's contained for errands, and its time zone and languages) is the person's own: never a tool's or a pipe's (Fugo, #435)";
const CODES: &str = "a site's recovery codes are the person's alone: shown only on their device, never to an agent, a tool or a pipe (Fugo, #435 W8b)";
const ALERTS: &str = "reading an alert is the person's: an agent never clears one from their Needs you (Bruno 44235)";
const HISTORY: &str = "History is the person's own, on their devices: never an agent's or a pipe's (Fugo, #458)";

/// Everything a request can change, and how Rivet does it. Only the tests
/// read it: what matters is that it builds.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn reach(body: &Body) -> Vec<Reach> {
    match body {
        Body::Hello(_) => vec![View],
        // Projects, agents and folders: the settings file.
        Body::CreateProject(_) | Body::UpdateProject(_) | Body::DeleteProject(_) => vec![Config(&["projects"])],
        Body::CreateAgent(_) | Body::DeleteAgent(_) | Body::ArrangeAgent(_) => vec![Config(&["agents"])],
        Body::UpdateAgent(request) => update_agent(request),
        Body::CreateFolder(_) | Body::UpdateFolder(_) | Body::DeleteFolder(_) => vec![Config(&["projects.folders"])],
        Body::UpdateSettings(request) => update_settings(request),
        Body::RestoreSettings(_) => vec![Config(&[])],
        Body::SetHighlights(_) => vec![Config(&["highlights"])],
        Body::SetZoom(_) => vec![Config(&["display.zoom"])],
        Body::SetFileCache(_) => vec![Config(&["display.file_cache"])],
        Body::PinAgent(_) | Body::UnpinAgent(_) => vec![Config(&["pinned_agents"])],
        Body::GetHistoryOverview(_) | Body::GetHistoryEvents(_) | Body::WatchHistory(_) => vec![Person(HISTORY)],
        Body::ReadAlerts(_) => vec![Person(ALERTS)],
        Body::WatchAgentBrowser(_)
        | Body::AgentBrowserFrameAck(_)
        | Body::AgentBrowserInput(_)
        | Body::TakeOverAgentBrowser(_)
        | Body::HandBackAgentBrowser(_)
        | Body::StopAgentBrowser(_)
        | Body::OpenAgentBrowser(_)
        | Body::DeleteAgentBrowserData(_)
        | Body::SetAgentBrowserLocale(_)
        | Body::SignOutAgentBrowserAccount(_)
        | Body::ListTimeZones(_) => vec![Person(BROWSER)],
        // A site's recovery codes from an errand (#435 W8b, Fugo 42966):
        // only the person sees or keeps them.
        Body::ShowRecoveryCodes(_) | Body::KeepRecoveryCodes(_) => vec![Person(CODES)],
        // Page text (Fugo 36686): never read untainted through View.
        Body::ListErrandLog(_) => vec![Person(BROWSER)],
        // The person's while Claktory's browser is off for everyone (#435):
        // once it ships, the manual names errand_open, the lead's tool for
        // it, and this says so.
        Body::OpenErrand(_) => vec![Person(BROWSER)],
        // Fugo's condition 6: only the person contains an agent or lifts it.
        Body::SetAgentContained(_) => vec![Person(BROWSER)],
        Body::ShuffleAvatar(_) => vec![Config(&["user.avatar.seed", "user.avatar.choice"])],
        Body::SetRemoteAccess(_) => vec![Config(&["remote.relays"])],
        // What a connection shows, reads and transfers.
        Body::SelectAgent(_)
        | Body::WatchTranscript(_)
        | Body::SetFocusMode(_)
        | Body::FocusProject(_)
        | Body::ExpandFolder(_)
        | Body::ShowAllFiles(_)
        | Body::OpenPanel(_)
        | Body::SetAgentCollapsed(_)
        | Body::SetFolderCollapsed(_)
        | Body::SetPipeGroupCollapsed(_)
        | Body::SetLayout(_)
        | Body::PaneOpened(_)
        | Body::SetPresentation(_)
        | Body::WatchFiles(_)
        | Body::FetchApp(_)
        | Body::ListFolders(_)
        | Body::ReadFile(_)
        | Body::LoadEntries(_)
        | Body::ListThreads(_)
        | Body::PreviewEnv(_)
        | Body::ListSecretUses(_)
        | Body::ListConnectChoices(_)
        | Body::LoadUsage(_)
        | Body::ScreenOpened(_)
        | Body::ShowPairingCode(_)
        | Body::LockVaultSession(_)
        | Body::ShowRecoveryKey(_)
        | Body::CheckRecoveryKey(_)
        | Body::StartUpload(_)
        | Body::UploadChunk(_)
        | Body::FinishUpload(_)
        | Body::CancelUpload(_)
        | Body::ReadPipeInvite(_)
        | Body::SelectPipe(_)
        | Body::LoadPipeMessages(_)
        | Body::SelectChat(_)
        | Body::LoadChatEntries(_)
        | Body::ListNeedsYouHistory(_)
        | Body::ListQuietCopies(_)
        | Body::ListBrowserLog(_)
        | Body::ReadInkBlob(_)
        | Body::ListInkLayers(_)
        | Body::ListPipeReports(_)
        | Body::SearchConversation(_)
        | Body::ListAppFiles(_)
        | Body::ListMonitors(_)
        | Body::ListMonitorHistory(_)
        | Body::ListBranches(_)
        | Body::OpenAppView(_)
        | Body::CloseAppView(_)
        | Body::TranscribeAudio(_)
        | Body::SendDiagnostics(_)
        | Body::SetActive(_)
        | Body::RegisterNotifications(_)
        | Body::BrowserCapability(_)
        | Body::BrowserGrant(_)
        | Body::BrowserResult(_)
        | Body::BrowserHeartbeat(_)
        | Body::DmLeafOffer(_)
        | Body::DmEnrollLeaf(_) => vec![View],
        // Rivet's tools.
        Body::RunCommand(_) => vec![Tool(&["agents_clear", "agents_compact"])],
        Body::ChangeMonitor(_) => vec![Tool(&["monitors_change"])],
        Body::StopTask(_) => vec![Tool(&["tasks_stop"])],
        // Resources' `clear_cache`.
        Body::ClearBuildCache(_) => vec![Tool(&["resources"])],
        Body::GrantApp(_) | Body::SubscribeApp(_) | Body::GrantAppSource(_) => vec![Tool(&["apps_set_access"])],
        Body::GrantSecret(_) => vec![Tool(&["secrets_grant"])],
        Body::RevokeSecret(_) => vec![Tool(&["secrets_revoke"])],
        Body::DeriveSecret(_) => vec![Tool(&["secrets_derive"])],
        Body::CreatePairing(_) => vec![Tool(&["devices_add"])],
        Body::UnpairDevice(_) | Body::CancelPairing(_) => vec![Tool(&["devices_remove"])],
        Body::StartMove(_) => vec![Tool(&["move_claktory"])],
        Body::CancelMove(_) => vec![Tool(&["move_cancel"])],
        Body::CreatePipeInvite(_) => vec![Tool(&["pipes_invite"])],
        Body::AcceptPipeInvite(_) => vec![Tool(&["pipes_accept"])],
        Body::SetPipeForward(_) => vec![Tool(&["pipes_forward"])],
        Body::SaveReportType(_) | Body::DeleteReportType(_) => vec![Tool(&["pipes_report_type"])],
        Body::CreatePipeGroup(_) | Body::RenamePipeGroup(_) | Body::DeletePipeGroup(_) | Body::SetPipeGroup(_) => {
            vec![Tool(&["pipes_group"])]
        }
        Body::ForkConversation(_) => vec![Tool(&["conversation_fork"])],
        Body::SwitchBranch(_) => vec![Tool(&["conversation_switch"])],
        // models_set checks a model the provider's list doesn't have, and
        // puts an agent waiting on a limit on another provider's model.
        Body::AddModel(_) | Body::ContinueOnAlternative(_) | Body::SwitchBack(_) => vec![Tool(&["models_set"])],
        // Which subscription is spent first: Rivet puts one last when asked.
        Body::MoveAccount(_) => vec![Tool(&[crate::extensions::models::ORDER_TOOL])],
        // A pipe: only who sends has a tool so far.
        Body::UpdatePipe(request) => update_pipe(request),
        // Only the person, on purpose.
        Body::StartLogin(_)
        | Body::AnswerLogin(_)
        | Body::CancelLogin(_)
        | Body::SignOut(_)
        | Body::UpdateAccount(_)
        | Body::AccountSignIn(_)
        | Body::AccountVerify(_)
        | Body::AccountResend(_)
        | Body::AccountSignOut(_)
        | Body::SkipAccountSignIn(_)
        | Body::StartConnect(_)
        | Body::FinishConnect(_)
        | Body::CancelConnect(_)
        | Body::DisconnectService(_) => vec![Person(SIGN_IN)],
        Body::CreateVault(_)
        | Body::UnlockVault(_)
        | Body::ChangePassphrase(_)
        | Body::NewRecoveryKey(_)
        | Body::ConfirmRecoveryKey(_)
        | Body::SetVaultLockedMode(_)
        | Body::CreateSecret(_)
        | Body::UpdateSecret(_)
        | Body::ImportEnv(_)
        | Body::DeleteSecret(_)
        | Body::RevealSecret(_)
        | Body::SetSecretAskEachTime(_)
        | Body::SetSecretSites(_)
        | Body::SetAppSecret(_)
        | Body::SecretRequestDone(_) => vec![Person(VAULT)],
        Body::DecideProposal(_)
        | Body::AnswerAsk(_)
        | Body::DecideSecretRequest(_)
        | Body::DecideSecretApproval(_)
        | Body::DismissConnectProposal(_)
        | Body::DecideAppGrant(_)
        | Body::DecideAppAccess(_)
        | Body::UndoAppAccessChange(_)
        | Body::UndoModelChange(_)
        | Body::DecideSuggestion(_)
        | Body::ApprovePipe(_)
        | Body::AcceptPipeIdentity(_)
        | Body::DecidePipeMessage(_)
        | Body::DismissPipeStart(_)
        | Body::ApproveDevice(_)
        | Body::DenyDevice(_)
        | Body::ConfirmDevice(_)
        | Body::DismissAllowNotice(_)
        | Body::ConfirmMove(_)
        | Body::SetOnboarding(_) => vec![Person(CARD)],
        // A pipe's waiting reports, and letting a reader go: on a card the
        // person approves (#471 G6); making one a reader with pipes_forward.
        Body::DecidePipeReports(_) => vec![Tool(&["pipes_reports"])],
        Body::SetReportReader(_) => vec![Tool(&["pipes_forward", "pipes_release_reader"])],
        // A liaison's daily spending limit: lowered at once, raised on a
        // card (#471 G5).
        Body::SetSpendCap(_) => vec![Tool(&["pipes_settings"])],
        // Sharing a website is the person's, on a card an admin agent asks
        // for (friend bug #32); stopping one, or hiding one shared with
        // them, is made at once, and showing it again is a card (#502).
        // Only they open one shared with them (pipe-port-sharing.md).
        Body::SharePipeWebsite(_) => vec![Tool(&["pipes_share"])],
        Body::StopPipeShare(_) => vec![Tool(&["pipes_settings", "pipes_shares"])],
        Body::HidePipeShare(_) => vec![Tool(&["pipes_shares"])],
        Body::OpenPipeShare(_) => vec![Person("only the person opens a website shared with them, in their own browser")],
        // An agent's card shares for its hours (#46, Fugo 46375); only the
        // person changes how long one lasts.
        Body::SetPipeShareExpiry(_) => vec![Person("only the person changes how long a website of theirs stays shared (#46)")],
        Body::CheckPipeSharePort(_) => vec![View],
        Body::SendMessage(_)
        | Body::RetryVoiceMessage(_)
        | Body::DiscardVoiceMessage(_)
        | Body::SendPipeMessage(_)
        | Body::CreateThread(_)
        | Body::ReplyThread(_)
        | Body::EditComment(_)
        | Body::DeleteComment(_)
        | Body::SetThreadStatus(_)
        | Body::ReaddressThread(_)
        | Body::ReattachThread(_)
        | Body::SendComments(_)
        | Body::AddGoal(_)
        | Body::UpdateGoal(_) => vec![Person(VOICE)],
        Body::ReportReply(_) => vec![Person("a report of a reply goes to claktory.com with the person's note")],
        Body::SaveInkLayer(_) | Body::DeleteInkLayer(_) | Body::CreateSketch(_) => vec![Person(DRAWING)],
        Body::AppViewAction(_) => vec![Person("a control the person pressed in an app's view; agents use the app's own tools")],
        Body::CreateChat(_)
        | Body::SendChatMessage(_)
        | Body::StopChat(_)
        | Body::DeleteChat(_)
        | Body::RenameChat(_)
        | Body::SetChatModel(_) => vec![Person(CHAT)],
        Body::InstallUpdate(_) => vec![Person("the person chooses when Claktory updates")],
        Body::InstallLocalApp(_) => vec![Person("installing an app is the person's review of what it may do")],
        Body::SetLidClosed(_) => vec![Person("only this Mac's own app keeps it working with the lid closed")],
        Body::FreezeMove(_) | Body::DeleteMovedCopy(_) => vec![Person("the move's own screens, on the computers it moves between")],
        Body::BrowserStop(_) | Body::ClearBrowserLog(_) => vec![Person("browser control is the person's")],
        Body::UseUsageReset(_) => vec![Person("it spends one of a subscription's resets")],
        Body::SendTestNotification(_) => vec![Person("it tests the person's own devices")],
        // The settings kept in factory.db: the Settings extension's tool (#422).
        Body::SetDiagnostics(_) | Body::SetKeepAwake(_) | Body::InstallSpeechModel(_) | Body::RemoveSpeechModel(_) | Body::PinApp(_) => {
            vec![Tool(&["settings"])]
        }
        // Ghiaccio's (#423 items 8-12).
        Body::StopAgent(_) => vec![Tool(&["agents_stop"])],
        Body::SetAppRunning(_) => vec![Tool(&["apps_set_running"])],
        Body::CheckForUpdates(_) => vec![Tool(&["updates_check"])],
        Body::RefreshModels(_) => vec![Tool(&["models_refresh"])],
        // Reads what's stale when the person looks (friend bug #41): what
        // they see, which models_refresh reads for Rivet.
        Body::ProvidersShown(_) => vec![View],
        Body::DeleteBranch(_) => vec![Tool(&["conversation_delete"])],
        Body::TryAgainNow(_) => vec![Tool(&["models_try_now"])],
        // Restarting Claktory under its background service (friend bug #24).
        Body::RestoreBackendService(_) => {
            vec![Person("it restarts Claktory under its background service, from the Mac it runs on, on the person's word")]
        }
        // Stopping Claktory on its Mac (friend bug #27).
        Body::StopEngine(_) => vec![Person("it stops Claktory itself, from its app on the Mac it runs on")],
        // #451, Fugo's ruling (a): the routing table, and the person's
        // messages, only ever from their own paired devices.
        Body::DmSend(_) | Body::DmFollow(_) | Body::DmAck(_) | Body::DmRoutes(_) => {
            vec![Person("direct messages go between the person's own devices and their home Claktory")]
        }
        // Through a card the person approves (#423, Bruno 28259); a pipe
        // narrowed at once.
        Body::SetUsageAutoContinue(_) => vec![Tool(&["models_auto_continue"])],
        Body::UninstallApp(_) => vec![Tool(&["apps_uninstall"])],
        Body::AddPipeLiaison(_) => vec![Tool(&["pipes_settings"])],
        // Deleting a closed pipe on a card too (#471 G8).
        Body::SetPipeState(_) => vec![Tool(&["pipes_settings"])],
        Body::SetAccountService(_) => vec![Person("which account service Claktory signs in at is the person's, as signing in is")],
        // Only this computer's own app signs itself in to the account (#529).
        Body::VouchDevice(_) => vec![Person("only this computer's own app signs itself in to the account through this Claktory")],
    }
}

fn update_agent(request: &v1::UpdateAgent) -> Vec<Reach> {
    let v1::UpdateAgent {
        agent_id: _,
        name: _,
        prompt: _,
        model: _,
        effort: _,
        project_id: _,
        color: _,
        extension: _,
        placeholder: _,
        folder: _,
        folder_id: _,
        avatar_shape: _,
        avatar_face: _,
        role: _,
        lead_id: _,
        fallback_model: _,
        notify: _,
    } = request;
    vec![
        Config(&[
            "agents.name",
            "agents.role",
            "agents.prompt",
            "agents.color",
            "agents.project",
            "agents.folder",
            "agents.lead",
            "agents.placeholder",
            "agents.avatar",
            "agents.fallback_model",
            "agents.extensions",
        ]),
        Tool(&["models_set"]),
        Tool(&["settings"]),
    ]
}

fn update_settings(request: &v1::UpdateSettings) -> Vec<Reach> {
    let v1::UpdateSettings {
        default_model: _,
        default_effort: _,
        compact_at_tokens: _,
        ram_percent: _,
        disk_free_gb: _,
        disk_free_percent: _,
        cpu_percent: _,
        sidebar_width: _,
        panel_width: _,
        build_cache_gb: _,
        user_name: _,
        vault_unlock_minutes: _,
        idle_compaction: _,
        idle_compaction_short_cache: _,
        cache_ttl: _,
        name: _,
        color: _,
        default_fallback_model: _,
        projects_collapsed: _,
        chats_collapsed: _,
        max_open_panes: _,
        notifications: _,
        checklist: _,
        notification_change: _,
        when_app_quits: _,
        resource_alerts_to_admins: _,
        resource_alert_agents: _,
    } = request;
    vec![
        Config(&[
            "default_model",
            "default_effort",
            "default_fallback_model",
            "compact_at_tokens",
            "resources.ram_percent",
            "resources.disk_free_gb",
            "resources.disk_free_percent",
            "resources.cpu_percent",
            "resources.build_cache_gb",
            "resources.alerts_to_admins",
            "resources.alert_agents",
            "display.sidebar_width",
            "display.panel_width",
            "user.name",
            "compaction.idle",
            "compaction.idle_short_cache",
            "prompt_cache",
            "name",
            "color",
        ]),
        // vault_unlock_minutes is unused; projects_collapsed and
        // chats_collapsed fold the sidebar's sections.
        View,
        // Notifications, the checklist's intervals and the pane limit: the
        // settings tool (#422), but for where the push relay sends them.
        Tool(&["settings"]),
        Person("where the push relay sends notifications is the person's: an agent could send them elsewhere"),
        // when_app_quits (friend bug #27).
        Person("whether Claktory stops when its app quits is the person's, asked in the Mac app"),
    ]
}

fn update_pipe(request: &v1::UpdatePipe) -> Vec<Reach> {
    let v1::UpdatePipe {
        pipe_id: _,
        name: _,
        prompt: _,
        purpose: _,
        members: _,
        lead_id: _,
        limits: _,
        review_outgoing: _,
        quarantined: _,
        review_incoming: _,
        send_only: _,
    } = request;
    // Its name, what it's for and its instructions on a card (#471 G2).
    vec![Tool(&["pipes_sender", "pipes_settings"])]
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_protocol::v1::ClientMessage;
    use prost::Message;

    const MANUAL: &str = include_str!("../../../docs/MANUAL.md");

    /// Every request the protocol has: each oneof field, decoded from an
    /// empty message under its number.
    fn every_request() -> Vec<(u32, Body)> {
        (2..1000)
            .filter_map(|number: u32| {
                let mut bytes = Vec::new();
                prost::encoding::encode_key(number, prost::encoding::WireType::LengthDelimited, &mut bytes);
                bytes.push(0);
                ClientMessage::decode(bytes.as_slice()).ok()?.body.map(|body| (number, body))
            })
            .collect()
    }

    fn named(word: &str) -> bool {
        MANUAL.contains(&format!("`{word}`"))
    }

    #[test]
    fn every_request_says_how_rivet_does_it_and_the_manual_agrees() {
        let requests = every_request();
        assert!(requests.len() > 200, "{} requests", requests.len());
        let mut missing = Vec::new();
        for (number, body) in &requests {
            for reach in reach(body) {
                match reach {
                    View => {}
                    Config(keys) => {
                        for key in keys {
                            for part in key.split('.') {
                                if !named(part) {
                                    missing.push(format!("{number}: the settings key `{part}` (of {key})"));
                                }
                            }
                        }
                    }
                    Tool(tools) => {
                        assert!(!tools.is_empty(), "{number}");
                        for tool in tools {
                            if !named(tool) {
                                missing.push(format!("{number}: the tool `{tool}`"));
                            }
                        }
                    }
                    Person(why) => assert!(!why.is_empty(), "{number}"),
                    Reach::Gap { owner, click } => {
                        assert!(!owner.is_empty(), "{number}");
                        let only = MANUAL.split("**Only in the app, for now.**").nth(1).expect("the manual's Only in the app");
                        let only = only.split("\n\n").next().unwrap_or_default();
                        if !only.contains(click) {
                            missing.push(format!("{number}: \"{click}\" in the manual's Only in the app ({owner})"));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "the manual doesn't say:\n{}", missing.join("\n"));
    }
}
