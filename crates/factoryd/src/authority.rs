//! Who may do what only the person does: no agent, on any side, sees or
//! handles a pairing code, a connect code or the four digits; agents only
//! raise cards.
//!
//! **Person authority** comes from exactly two places:
//! - a paired device, over the private tunnel (its key lives on the device);
//! - Claktory's own Mac app on this computer, over the app's socket
//!   (`<home>/app.sock`), proven by its code signature: the socket's audit
//!   token names the very process, whose signature must be Claktory's
//!   Developer ID team, the app's exact identifier, the hardened runtime and
//!   no get-task-allow. A Dev Claktory (`--dev`, signed ad hoc) takes the
//!   Dev app pinned by its cdhash in `<home>/app-pins` instead.
//!
//! Nothing else carries it: no file, no terminal, no client_version, no
//! environment variable. Linux has no local person authority at all: the
//! person approves from a paired device.
//!
//! **Tool authority** is what agents' commands get: `factoryd status`,
//! `settings set`, and scripts, over the loopback port with the token in
//! `<home>/tool-token` (0600, bare hex), which every agent's command can
//! read. A tool connection reads the Workspace, with every code and the
//! digits left out (`strip_workspace`), and sends only what `tool_allowed`
//! names; everything else is refused with PERSON_ONLY before it's handled.

#[cfg_attr(windows, allow(unused_imports))]
use std::path::Path;
use std::path::PathBuf;

use factory_protocol::v1::{self, client_message::Body};
use sha2::{Digest, Sha256};

use crate::encoding::hex;
use crate::home::Home;

/// Who's on the other end of a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authority {
    /// A paired device, or this computer's own app over its socket.
    Person,
    /// An agent's command or script, with the tool token.
    Tool,
    /// The Windows app, with this start's app token (`app-token` in the home,
    /// windows-constraints.md §9 and §14, position B): local and
    /// uncontained, so the same user's programs can read the token too.
    /// It may send what `local_may` allows, never the boundary set; its
    /// Workspace is stripped as a tool's. Nothing in its Hello upgrades it.
    LocalUncontained,
}

/// The device id a tool connection is welcomed as: never a paired
/// device's, never this computer's app's (`remote::LOCAL_DEVICE`).
pub const TOOL_DEVICE: &str = "tool";

/// The tool token's file, in the home's root.
const TOOL_TOKEN: &str = "tool-token";

/// The app's socket, in the home's root.
pub(crate) const APP_SOCKET: &str = "app.sock";

/// The Dev app's pinned cdhashes (hex, one a line), read only by a Dev
/// Claktory (`--dev`): scripts/dev.sh writes the one it builds.
pub(crate) const APP_PINS: &str = "app-pins";

/// The real Mac app's identifier, and the Dev and debug builds'.
#[cfg(target_os = "macos")]
const APP: &str = "dev.factory.app";
#[cfg(target_os = "macos")]
const DEV_APPS: [&str; 2] = ["dev.factory.app.dev", "dev.factory.app.debug"];

/// The longest path a Unix socket's address holds, with its NUL: macOS's
/// sun_path is 104 bytes, Linux's 108.
const SOCKET_PATH_MOST: usize = 103;

/// What a tool connection may send (the allowlist): Hello, the settings
/// `factoryd settings set` changes, calling off a move, asking the person
/// to add a device (`factoryd pair`, answered with no code), and asking the
/// person's devices to approve a Claktory being set up, whose handler
/// checks it's being set up (`account::tools_may_authorize`): the messages
/// `class` says are Tool's. Nothing goes on it without a reason sent to
/// Fugo first.
pub fn tool_allowed(body: &Body) -> bool {
    class(body) == Class::Tool
}

/// Whether only the person may send `body`: everything off the tool
/// allowlist.
pub fn needs_person(body: &Body) -> bool {
    !tool_allowed(body)
}

/// UpdateSettings from a tool connection: only the fields `factoryd
/// settings set` takes (the person's name, this Claktory's name and color,
/// the default model and effort). A field added later is refused until
/// it's named here.
fn settings_allowed(update: &v1::UpdateSettings) -> bool {
    let kept = v1::UpdateSettings {
        user_name: update.user_name.clone(),
        name: update.name.clone(),
        color: update.color.clone(),
        default_model: update.default_model.clone(),
        default_effort: update.default_effort.clone(),
        ..Default::default()
    };
    kept == *update
}

/// The refusal a tool connection gets for anything off its allowlist.
pub(crate) fn person_only() -> (v1::ErrorCode, &'static str) {
    (
        v1::ErrorCode::PersonOnly,
        "Do this in the Claktory app, on this computer or on one of your devices. Commands in a terminal can't.",
    )
}

/// The refusal the Windows app's token gets for the boundary set
/// (`local_may`), with PERSON_ONLY's code: the person does it on a phone, a
/// tablet or a Mac.
pub(crate) fn not_on_windows() -> (v1::ErrorCode, &'static str) {
    (v1::ErrorCode::PersonOnly, "Claktory on Windows can't do this. Do it on your phone, tablet or Mac.")
}

/// A tool connection's Workspace: the codes and digits it could carry left
/// out (a pipe's or a move's digits to compare, a provider sign-in's code),
/// and the no-code setup's requests waiting (an agent can't watch for one
/// to race it). A waiting device's digits go to no one
/// (`remote_view`).
pub(crate) fn strip_workspace(workspace: &mut v1::Workspace) {
    strip_workspace_for(workspace, Authority::Tool);
}

/// `strip_workspace` for a connection with `authority`: the Windows app's
/// (LocalUncontained) keeps a model provider's sign-in code, since it may
/// sign in to one (StartLogin) and the code only finishes a sign-in whose
/// tokens go to this Claktory (53125).
pub(crate) fn strip_workspace_for(workspace: &mut v1::Workspace, authority: Authority) {
    for pipe in &mut workspace.pipes {
        pipe.digits.clear();
    }
    if let Some(seen) = &mut workspace.r#move {
        seen.digits.clear();
    }
    if let Some(login) = &mut workspace.login
        && authority != Authority::LocalUncontained
    {
        login.user_code.clear();
    }
    if let Some(account) = &mut workspace.account {
        account.authorizations_waiting.clear();
    }
    if let Some(remote) = &mut workspace.remote {
        for device in &mut remote.pending_devices {
            device.number.clear();
        }
    }
}

// ---- what each message needs (windows-constraints.md §14) ----

/// Who may send a message: each class may also send what the classes before
/// it may. Every ClientMessage has one (`class`), so a message added later
/// doesn't build until it's classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// An agent's command with the tool token may (`tool_allowed`).
    Tool,
    /// The Windows app's token too (local, uncontained: §9's position B):
    /// everything an ordinary session needs.
    Local,
    /// The Windows app's token only while this Claktory has never been
    /// signed in (`account::never_signed_in`): the person signs in on
    /// Windows by typing the emailed code (§9's onboarding). After the
    /// first sign-in, the person's.
    LocalBeforeFirstSignIn,
    /// The Windows app's token unless it approves (or answers) a proposal
    /// whose grants change a pipe's settings, which only the person may.
    LocalUnlessPipeGrant,
    /// Only the person: a paired device, or Claktory's own Mac app. The
    /// boundary set (§9): devices, the tunnel, approving a computer's
    /// sign-in or a move, the account, the recovery key and codes, moves,
    /// pipes, direct messages and containment.
    Person,
}

/// What `body` needs (`Class`).
pub fn class(body: &Body) -> Class {
    match body {
        // Turning an agent's browser on is the person's alone: on Windows a
        // same-user agent could otherwise press it. Turning it off, and
        // every other change, is as any UpdateAgent.
        Body::UpdateAgent(update) if crate::errands::turns_browser_on(update) => Class::Person,
        Body::Hello(_)
        | Body::CancelMove(_)
        | Body::AccountAuthorize(_)
        | Body::RequestAddDevice(_) => Class::Tool,
        Body::CreateProject(_)
        | Body::CreateAgent(_)
        | Body::UpdateAgent(_)
        | Body::SelectAgent(_)
        | Body::SendMessage(_)
        | Body::StopAgent(_)
        | Body::StartLogin(_)
        | Body::MoveAccount(_)
        | Body::UpdateAccount(_)
        | Body::AnswerLogin(_)
        | Body::CancelLogin(_)
        | Body::SignOut(_)
        | Body::DeleteAgent(_)
        | Body::DeleteProject(_)
        | Body::UpdateProject(_)
        | Body::RestoreSettings(_)
        | Body::InstallUpdate(_)
        | Body::FocusProject(_)
        | Body::ExpandFolder(_)
        | Body::ShowAllFiles(_)
        | Body::OpenPanel(_)
        | Body::ArrangeAgent(_)
        | Body::RestoreArrangement(_)
        | Body::PreviewArrange(_)
        | Body::PreviewProjectFolder(_)
        | Body::SetProjectCollapsed(_)
        | Body::SetAgentCollapsed(_)
        | Body::RunCommand(_)
        | Body::CreateFolder(_)
        | Body::UpdateFolder(_)
        | Body::DeleteFolder(_)
        | Body::SetFolderCollapsed(_)
        | Body::SetHighlights(_)
        | Body::SetAttribution(_)
        | Body::PaymentOpened(_)
        | Body::StopTask(_)
        | Body::ChangeMonitor(_)
        | Body::FetchApp(_)
        | Body::ListFolders(_)
        | Body::ClearBuildCache(_)
        | Body::ReadFile(_)
        | Body::SetLayout(_)
        | Body::CreateVault(_)
        | Body::UnlockVault(_)
        | Body::LockVaultSession(_)
        | Body::CreateSecret(_)
        | Body::UpdateSecret(_)
        | Body::DeleteSecret(_)
        | Body::GrantSecret(_)
        | Body::RevokeSecret(_)
        | Body::RevealSecret(_)
        | Body::PreviewEnv(_)
        | Body::ImportEnv(_)
        | Body::DecideSecretRequest(_)
        | Body::ListSecretUses(_)
        | Body::ChangePassphrase(_)
        | Body::SetVaultLockedMode(_)
        | Body::LoadEntries(_)
        | Body::StartUpload(_)
        | Body::UploadChunk(_)
        | Body::FinishUpload(_)
        | Body::CancelUpload(_)
        | Body::ListThreads(_)
        | Body::CreateThread(_)
        | Body::ReplyThread(_)
        | Body::EditComment(_)
        | Body::DeleteComment(_)
        | Body::SetThreadStatus(_)
        | Body::ReaddressThread(_)
        | Body::ReattachThread(_)
        | Body::SendComments(_)
        | Body::DecideSuggestion(_)
        | Body::WatchFiles(_)
        | Body::StartConnect(_)
        | Body::FinishConnect(_)
        | Body::CancelConnect(_)
        | Body::DismissConnectProposal(_)
        | Body::ListConnectChoices(_)
        | Body::DeriveSecret(_)
        | Body::DisconnectService(_)
        | Body::SetZoom(_)
        | Body::UseUsageReset(_)
        | Body::LoadUsage(_)
        | Body::InstallLocalApp(_)
        | Body::SetAppRunning(_)
        | Body::UninstallApp(_)
        | Body::GrantApp(_)
        | Body::SubscribeApp(_)
        | Body::SetAppSecret(_)
        | Body::ShuffleAvatar(_)
        | Body::ScreenOpened(_)
        | Body::SkipAccountSignIn(_)
        | Body::SetOnboarding(_)
        | Body::SelectPipe(_)
        | Body::LoadPipeMessages(_)
        | Body::DecideSecretApproval(_)
        | Body::InstallSpeechModel(_)
        | Body::RemoveSpeechModel(_)
        | Body::RetryVoiceMessage(_)
        | Body::DiscardVoiceMessage(_)
        | Body::AnswerAsk(_)
        | Body::CreateChat(_)
        | Body::SelectChat(_)
        | Body::SendChatMessage(_)
        | Body::StopChat(_)
        | Body::DeleteChat(_)
        | Body::LoadChatEntries(_)
        | Body::RenameChat(_)
        | Body::SetSecretAskEachTime(_)
        | Body::AddGoal(_)
        | Body::UpdateGoal(_)
        | Body::DismissPipeStart(_)
        | Body::SetActive(_)
        | Body::RegisterNotifications(_)
        | Body::SendTestNotification(_)
        | Body::OpenAppView(_)
        | Body::AppViewAction(_)
        | Body::CloseAppView(_)
        | Body::GrantAppSource(_)
        | Body::PinApp(_)
        | Body::ListNeedsYouHistory(_)
        | Body::ListQuietCopies(_)
        | Body::DecideAppGrant(_)
        | Body::BrowserCapability(_)
        | Body::BrowserGrant(_)
        | Body::BrowserResult(_)
        | Body::BrowserHeartbeat(_)
        | Body::BrowserStop(_)
        | Body::ListBrowserLog(_)
        | Body::ClearBrowserLog(_)
        | Body::ReadInkBlob(_)
        | Body::ListInkLayers(_)
        | Body::SaveInkLayer(_)
        | Body::DeleteInkLayer(_)
        | Body::SetPresentation(_)
        | Body::CreateSketch(_)
        | Body::CheckForUpdates(_)
        | Body::SetDiagnostics(_)
        | Body::SendDiagnostics(_)
        | Body::ListPipeReports(_)
        | Body::ReportReply(_)
        | Body::SearchConversation(_)
        | Body::TranscribeAudio(_)
        | Body::TryAgainNow(_)
        | Body::DecideAppAccess(_)
        | Body::UndoAppAccessChange(_)
        | Body::ContinueOnAlternative(_)
        | Body::SwitchBack(_)
        | Body::UndoModelChange(_)
        | Body::ListAppFiles(_)
        | Body::SetChatModel(_)
        | Body::SetKeepAwake(_)
        | Body::SecretRequestDone(_)
        | Body::ListMonitors(_)
        | Body::ListMonitorHistory(_)
        | Body::GetChecklist(_)
        | Body::ListTasks(_)
        | Body::CreatePipeGroup(_)
        | Body::RenamePipeGroup(_)
        | Body::DeletePipeGroup(_)
        | Body::SetPipeGroup(_)
        | Body::SetPipeGroupCollapsed(_)
        | Body::PaneOpened(_)
        | Body::ForkConversation(_)
        | Body::ListBranches(_)
        | Body::SwitchBranch(_)
        | Body::DeleteBranch(_)
        | Body::SetUsageAutoContinue(_)
        | Body::SetLidClosed(_)
        | Body::RefreshModels(_)
        | Body::AddModel(_)
        | Body::RestoreBackendService(_)
        | Body::HidePipeShare(_)
        | Body::StopEngine(_)
        | Body::SetFileCache(_)
        | Body::GetHistoryOverview(_)
        | Body::GetHistoryEvents(_)
        | Body::WatchHistory(_)
        | Body::WatchAgentBrowser(_)
        | Body::AgentBrowserFrameAck(_)
        | Body::AgentBrowserInput(_)
        | Body::TakeOverAgentBrowser(_)
        | Body::HandBackAgentBrowser(_)
        | Body::StopAgentBrowser(_)
        | Body::OpenAgentBrowser(_)
        | Body::DeleteAgentBrowserData(_)
        | Body::ListErrandLog(_)
        | Body::OpenErrand(_)
        | Body::SetAgentBrowserLocale(_)
        | Body::ListTimeZones(_)
        | Body::SetFocusMode(_)
        | Body::WatchTranscript(_)
        | Body::DismissAllowNotice(_)
        | Body::SetSecretSites(_)
        | Body::SignOutAgentBrowserAccount(_)
        | Body::ProvidersShown(_)
        | Body::ReadAlerts(_)
        | Body::CheckProtectedPlaces(_)
        | Body::DismissProtectedPlace(_)
        | Body::PinAgent(_)
        | Body::UnpinAgent(_)
        | Body::DismissNotificationsOffer(_) => Class::Local,
        Body::AccountSignIn(_)
        | Body::AccountVerify(_)
        | Body::AccountResend(_)
        | Body::AccountSignInCancel(_) => Class::LocalBeforeFirstSignIn,
        Body::CreatePairing(_)
        | Body::UnpairDevice(_)
        | Body::WithdrawPairing(_)
        | Body::SetRemoteAccess(_)
        | Body::NewRecoveryKey(_)
        | Body::DeleteSealedCopy(_)
        | Body::AccountSignOut(_)
        | Body::SetAccountService(_)
        | Body::ShowRecoveryKey(_)
        | Body::ApproveDevice(_)
        | Body::DenyDevice(_)
        | Body::CancelPairing(_)
        | Body::ShowPairingCode(_)
        | Body::ConfirmDevice(_)
        | Body::CreatePipeInvite(_)
        | Body::ReadPipeInvite(_)
        | Body::AcceptPipeInvite(_)
        | Body::ApprovePipe(_)
        | Body::UpdatePipe(_)
        | Body::SetPipeState(_)
        | Body::SendPipeMessage(_)
        | Body::DecidePipeMessage(_)
        | Body::AddPipeLiaison(_)
        | Body::StartMove(_)
        | Body::ConfirmMove(_)
        | Body::FreezeMove(_)
        | Body::DeleteMovedCopy(_)
        | Body::CheckRecoveryKey(_)
        | Body::ConfirmRecoveryKey(_)
        | Body::DmLeafOffer(_)
        | Body::DmEnrollLeaf(_)
        | Body::AcceptPipeIdentity(_)
        | Body::SetPipeForward(_)
        | Body::SetReportReader(_)
        | Body::DecidePipeReports(_)
        | Body::SetSpendCap(_)
        | Body::SaveReportType(_)
        | Body::DeleteReportType(_)
        | Body::SharePipeWebsite(_)
        | Body::StopPipeShare(_)
        | Body::OpenPipeShare(_)
        | Body::CheckPipeSharePort(_)
        | Body::DmSend(_)
        | Body::DmFollow(_)
        | Body::DmAck(_)
        | Body::DmRoutes(_)
        | Body::SetAgentContained(_)
        | Body::VouchDevice(_)
        | Body::ShowRecoveryCodes(_)
        | Body::KeepRecoveryCodes(_)
        | Body::UnpairAccountKey(_)
        | Body::ProveAllowedDevice(_)
        | Body::SetPipeShareExpiry(_)
        | Body::PublishRecoveryKey(_)
        | Body::UseRecoveryKey(_)
        | Body::WatchAgentTab(_)
        | Body::AgentTabFrameAck(_)
        | Body::AgentTabInput(_)
        | Body::ListDeviceActivity(_) => Class::Person,
        Body::UpdateSettings(update) => {
            if settings_allowed(update) {
                Class::Tool
            } else {
                Class::Local
            }
        }
        Body::DecideProposal(_) => Class::LocalUnlessPipeGrant,
    }
}

/// Whether the Windows app's token may send `body` (`Class`), for this
/// Claktory as it is now.
pub(crate) fn local_may(factory: &crate::Factory, body: &Body) -> bool {
    match class(body) {
        Class::Tool | Class::Local => true,
        Class::LocalBeforeFirstSignIn => crate::account::never_signed_in(factory),
        Class::LocalUnlessPipeGrant => match body {
            Body::DecideProposal(decision) if decision.approve || decision.reply => {
                // One that isn't a proposal is refused by its handler.
                decision.proposal_id.trim().parse().map_or(true, |number| !crate::proposal_grants::widens_a_pipe(&factory.store.db(), number))
            }
            _ => true,
        },
        Class::Person => false,
    }
}

/// Tests: `local_may`.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn local_may_for_tests(factory: &crate::Factory, body: &Body) -> bool {
    local_may(factory, body)
}

// ---- the tool token ----

fn tool_token_path(home: &Home) -> PathBuf {
    home.root().join(TOOL_TOKEN)
}

fn well_formed(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Writes `<home>/tool-token` unless a well-formed one is there: agents'
/// commands read it, so it stays the same across restarts.
pub(crate) fn prepare_tool_token(home: &Home) -> std::io::Result<()> {
    let path = tool_token_path(home);
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    if well_formed(kept.trim()) {
        return Ok(());
    }
    let mut token = [0_u8; 32];
    getrandom::fill(&mut token).map_err(std::io::Error::other)?;
    crate::remote::write_private(&path, format!("{}\n", hex(&token)).as_bytes())
}

/// Whether `token` is this home's tool token, compared in full.
pub(crate) fn is_tool_token(home: &Home, token: &str) -> bool {
    let kept = std::fs::read_to_string(tool_token_path(home)).unwrap_or_default();
    let kept = kept.trim();
    well_formed(kept) && same(kept, token)
}

/// Compares two tokens by their hashes, in full whatever matches.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (Sha256::digest(a.as_bytes()), Sha256::digest(b.as_bytes()));
    a.iter().zip(b.iter()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

/// What `factoryd`'s commands connect with: the tool token, if there's one.
pub(crate) fn read_tool_token(home: &Home) -> Option<String> {
    let token = std::fs::read_to_string(tool_token_path(home)).ok()?;
    Some(token.trim().to_owned()).filter(|token| well_formed(token))
}

// ---- tests' and QA's person token ----

#[cfg(all(feature = "test-hooks", not(debug_assertions)))]
compile_error!("The test-hooks feature is for QA builds only (the tests): a release build never carries it.");

#[cfg(all(feature = "qa-person", not(debug_assertions)))]
compile_error!("The qa-person feature is for QA builds only: a release build never carries it.");

/// The person tokens of this process's Claktories, by home (tests and QA
/// builds only): each lets a client in on the loopback port as this
/// computer's app.
#[cfg(any(feature = "test-hooks", feature = "qa-person"))]
fn person_tokens() -> std::sync::MutexGuard<'static, std::collections::HashMap<PathBuf, String>> {
    static TOKENS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, String>>> =
        std::sync::LazyLock::new(Default::default);
    TOKENS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(any(feature = "test-hooks", feature = "qa-person"))]
fn person_token(home: &Path) -> String {
    person_tokens()
        .entry(home.to_owned())
        .or_insert_with(|| {
            let mut token = [0_u8; 32];
            getrandom::fill(&mut token).expect("random numbers");
            hex(&token)
        })
        .clone()
}

/// Tests: the token that lets a client in on the loopback port as this
/// computer's app (Person) at the Claktory whose home is `home`, in this
/// process; or, for a backend the test runs as a process of its own, the
/// one it was given in FACTORY_TEST_PERSON_TOKEN.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn person_token_for_test(home: &Path) -> String {
    person_token(home)
}

/// QA builds: the person token is `<home>/qa-person-token`, where
/// claktory-qa's driver reads it; kept across restarts, as the tool token.
#[cfg(feature = "qa-person")]
pub(crate) fn prepare_qa_person(home: &Home) {
    let path = home.root().join("qa-person-token");
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    if well_formed(kept.trim()) {
        person_tokens().insert(home.root().to_owned(), kept.trim().to_owned());
        return;
    }
    let token = person_token(home.root());
    if let Err(error) = crate::remote::write_private(&path, format!("{token}\n").as_bytes()) {
        tracing::error!(%error, "could not write the QA person token");
    }
}

/// Whether `token` lets a client in as the person on the loopback port:
/// never, except in test and QA builds.
pub(crate) fn is_person_token(home: &Home, token: &str) -> bool {
    #[cfg(any(feature = "test-hooks", feature = "qa-person"))]
    {
        if !token.is_empty() && same(&person_token(home.root()), token) {
            return true;
        }
        #[cfg(feature = "test-hooks")]
        if let Ok(given) = std::env::var("FACTORY_TEST_PERSON_TOKEN")
            && !given.is_empty()
            && same(&given, token)
        {
            return true;
        }
    }
    let _ = (home, token);
    false
}

// ---- the app's socket ----

/// Where the Mac app's socket is: `<home>/app.sock`, or, when that path is
/// too long for a socket's address, `claktory-<hash of the home>/app.sock`
/// in this user's temporary folder (on macOS, confstr's
/// _CS_DARWIN_USER_TEMP_DIR, as NSTemporaryDirectory() gives it to the
/// app), in a folder only this user can open.
pub fn app_socket_path(factory: &crate::Factory) -> PathBuf {
    socket_path(&factory.home)
}

pub fn socket_path(home: &Home) -> PathBuf {
    let path = home.root().join(APP_SOCKET);
    if path.as_os_str().len() <= SOCKET_PATH_MOST {
        return path;
    }
    let hash = hex(&Sha256::digest(home.root().as_os_str().as_encoded_bytes()));
    user_temp().join(format!("claktory-{}", &hash[..16])).join(APP_SOCKET)
}

/// This user's own temporary folder.
fn user_temp() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = vec![0_u8; 1024];
        let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buffer.as_mut_ptr().cast(), buffer.len()) };
        if len > 1 && len <= buffer.len() {
            buffer.truncate(len - 1);
            return PathBuf::from(String::from_utf8_lossy(&buffer).into_owned());
        }
    }
    std::env::temp_dir()
}

/// Whether this Claktory is a Dev one (`--dev`), which takes the Dev app
/// pinned by its cdhash: main sets it before serving.
static DEV: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Says this is a Dev Claktory (`factoryd --dev`).
pub fn set_dev(dev: bool) {
    DEV.store(dev, std::sync::atomic::Ordering::Relaxed);
}

/// What a program on the app's socket that isn't Claktory's own app is
/// told, for a person to read: what to do, never the signature's details
/// (those are logged).
pub(crate) fn not_the_app() -> &'static str {
    if DEV.load(std::sync::atomic::Ordering::Relaxed) {
        "Only the Claktory Dev build scripts/dev.sh made can connect to this Claktory. Run scripts/dev.sh app to let this build in."
    } else {
        "Only Claktory's own app on this Mac can connect to it. Install Claktory again from claktory.com."
    }
}

/// The app's socket, listening (macOS only: elsewhere there's no app on
/// this computer to give person authority to).
pub(crate) struct AppSocket {
    #[cfg(target_os = "macos")]
    listener: tokio::net::UnixListener,
    #[cfg(target_os = "macos")]
    home: Home,
    /// The socket file it listens at, by its device and inode, to notice
    /// it removed or replaced (`replaced`).
    #[cfg(target_os = "macos")]
    bound: (u64, u64),
}

/// Sockets opened before their Claktory serves, by home (`open`).
fn opened() -> std::sync::MutexGuard<'static, std::collections::HashMap<PathBuf, AppSocket>> {
    static OPENED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, AppSocket>>> =
        std::sync::LazyLock::new(Default::default);
    OPENED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Opens the app's socket as the Claktory at `home` opens, so the app finds
/// it there from the moment it's started; `take` serves it.
pub(crate) fn open(home: &Home) {
    if let Some(socket) = AppSocket::bind(home) {
        opened().insert(home.root().to_owned(), socket);
    }
}

impl AppSocket {
    /// The socket `open` opened for `home`, or one opened now.
    pub(crate) fn take(home: &Home) -> Option<AppSocket> {
        let opened = opened().remove(home.root());
        opened.or_else(|| AppSocket::bind(home))
    }

    /// Listens at `socket_path(home)`, replacing a socket an earlier run
    /// left (never anything else there). None where there's no app, or when
    /// it can't.
    fn bind(home: &Home) -> Option<AppSocket> {
        #[cfg(target_os = "macos")]
        {
            let path = socket_path(home);
            match listen(&path) {
                Ok((listener, bound)) => return Some(AppSocket { listener, home: home.clone(), bound }),
                Err(error) => tracing::error!(%error, path = %path.display(), "could not open the app's socket"),
            }
        }
        let _ = home;
        None
    }

    /// Why the file at the socket's path is no longer the one it listens
    /// at, if it isn't: removed, or replaced by another (any program running
    /// as this user can, while agents' commands aren't sandboxed), so the
    /// app's next connection would reach whatever took its place.
    pub(crate) fn replaced(&self) -> Option<&'static str> {
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::MetadataExt;
            match std::fs::symlink_metadata(socket_path(&self.home)) {
                Ok(meta) if (meta.dev(), meta.ino()) == self.bound => None,
                Ok(_) => Some("replaced"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some("removed"),
                Err(error) => {
                    tracing::debug!(%error, "could not look at the app's socket");
                    None
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        None
    }

    /// Listens at the socket's path again, over a socket that took its
    /// place (never anything else). Whether it could.
    pub(crate) fn rebind(&mut self) -> bool {
        #[cfg(target_os = "macos")]
        {
            let path = socket_path(&self.home);
            match listen(&path) {
                Ok((listener, bound)) => {
                    self.listener = listener;
                    self.bound = bound;
                    return true;
                }
                Err(error) => tracing::error!(%error, path = %path.display(), "could not open the app's socket again"),
            }
        }
        false
    }

    /// The next client, and whether it's Claktory's own app: Ok, or why
    /// not. It's told at its Hello.
    pub(crate) async fn accept(&self) -> Option<(AppStream, Result<(), String>)> {
        #[cfg(target_os = "macos")]
        {
            let (stream, _) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "the app's socket couldn't accept");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    return None;
                }
            };
            let checked = check_app(&stream, &self.home);
            if let Err(why) = &checked {
                tracing::warn!(why, "turned away a program on the app's socket that isn't Claktory's app");
            }
            Some((stream, checked))
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::future::pending::<()>().await;
            None
        }
    }
}

/// A client on the app's socket: a Unix socket's; on Windows, where the
/// app comes on the loopback port with its token instead, none ever is.
#[cfg(unix)]
pub(crate) type AppStream = tokio::net::UnixStream;
#[cfg(windows)]
pub(crate) type AppStream = tokio::io::DuplexStream;

#[cfg(target_os = "macos")]
fn listen(path: &Path) -> std::io::Result<(tokio::net::UnixListener, (u64, u64))> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let folder = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    crate::remote::private_dir(folder)?;
    // Never followed: the folder is this user's alone and not a link.
    let meta = std::fs::symlink_metadata(folder)?;
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::other(format!("{} isn't a folder only this user can open", folder.display())));
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)?,
        Ok(_) => return Err(std::io::Error::other(format!("{} is there and isn't a socket", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let meta = std::fs::symlink_metadata(path)?;
    Ok((listener, (meta.dev(), meta.ino())))
}

/// Whether the process on the other end of `stream` is Claktory's own app.
#[cfg(target_os = "macos")]
fn check_app(stream: &tokio::net::UnixStream, home: &Home) -> Result<(), String> {
    use claktory_browser_host::peer;
    let dev = DEV.load(std::sync::atomic::Ordering::Relaxed);
    let requirement = requirement(dev, home)?;
    let signature = peer::check(stream, &requirement)?;
    // A real Claktory's app is notarized: the hardened runtime, always.
    if !dev && !signature.runtime {
        return Err(format!("{} isn't signed with the hardened runtime", signature.identifier));
    }
    Ok(())
}

/// The code requirement the app must meet. A real Claktory: Developer ID,
/// this backend's own team, the app's identifier, no get-task-allow; a
/// backend that isn't signed with a team has none, and nobody is let in
/// (the person uses a paired device). A Dev Claktory: the Dev app, pinned
/// by cdhash.
#[cfg(target_os = "macos")]
fn requirement(dev: bool, home: &Home) -> Result<String, String> {
    use claktory_browser_host::peer;
    if dev {
        let pins = std::fs::read_to_string(home.root().join(APP_PINS)).unwrap_or_default();
        let pins: Vec<String> = pins
            .lines()
            .map(str::trim)
            .filter(|pin| !pin.is_empty() && pin.len() <= 128 && pin.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(|pin| format!("cdhash H\"{}\"", pin.to_ascii_lowercase()))
            .collect();
        if pins.is_empty() {
            return Err(format!(
                "no Dev app is pinned in {}: scripts/dev.sh app pins the one it builds",
                home.root().join(APP_PINS).display()
            ));
        }
        let apps: Vec<String> =
            DEV_APPS.iter().map(|app| peer::quoted(app).map(|app| format!("identifier {app}"))).collect::<Result<_, _>>()?;
        return Ok(format!("({}) and ({})", apps.join(" or "), pins.join(" or ")));
    }
    let own = peer::own()?;
    let Some(team) = own.team.filter(|_| !own.adhoc) else {
        return Err("this Claktory isn't signed with a Developer ID, so no app on this Mac can act as you: use a paired device".into());
    };
    Ok(format!(
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = {} and identifier {} and !(entitlement[\"com.apple.security.get-task-allow\"] exists)",
        peer::quoted(&team)?,
        peer::quoted(APP)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Windows app sees a model provider's sign-in code, a tool
    /// doesn't; neither sees the rest.
    #[test]
    fn only_the_windows_app_sees_a_providers_sign_in_code() {
        let workspace = v1::Workspace {
            login: Some(v1::LoginFlow { user_code: "AB12-CD34".into(), ..Default::default() }),
            pipes: vec![v1::Pipe { digits: "1234".into(), ..Default::default() }],
            account: Some(v1::ClaktoryAccount { authorizations_waiting: vec![Default::default()], ..Default::default() }),
            ..Default::default()
        };
        let seen = |authority| {
            let mut workspace = workspace.clone();
            strip_workspace_for(&mut workspace, authority);
            (
                workspace.login.map(|login| login.user_code).unwrap_or_default(),
                workspace.pipes[0].digits.clone(),
                workspace.account.map_or(0, |account| account.authorizations_waiting.len()),
            )
        };
        assert_eq!(seen(Authority::LocalUncontained), ("AB12-CD34".to_owned(), String::new(), 0));
        assert_eq!(seen(Authority::Tool), (String::new(), String::new(), 0));
        let mut tools = workspace.clone();
        strip_workspace(&mut tools);
        assert_eq!(tools.login.map(|login| login.user_code).unwrap_or_default(), "");
    }

    #[test]
    fn a_tool_connection_sets_only_the_settings_the_cli_sets() {
        let name = v1::UpdateSettings { name: Some("Studio".into()), default_effort: Some("low".into()), ..Default::default() };
        assert!(tool_allowed(&Body::UpdateSettings(name)));
        let vault = v1::UpdateSettings { name: Some("Studio".into()), vault_unlock_minutes: Some(1), ..Default::default() };
        assert!(!tool_allowed(&Body::UpdateSettings(vault)));
        assert!(!tool_allowed(&Body::ApproveDevice(v1::ApproveDevice::default())));
        assert!(tool_allowed(&Body::CancelMove(v1::CancelMove::default())));
    }

    /// The table Fugo approved (52030): what the Windows app's token may
    /// send, and the boundary set it may not.
    #[test]
    fn each_message_has_the_class_fugo_approved() {
        let tool = [
            Body::Hello(v1::Hello::default()),
            Body::CancelMove(v1::CancelMove::default()),
            Body::AccountAuthorize(v1::AccountAuthorize::default()),
            Body::RequestAddDevice(v1::RequestAddDevice::default()),
            Body::UpdateSettings(v1::UpdateSettings { user_name: Some("Lee".into()), ..Default::default() }),
        ];
        for body in &tool {
            assert_eq!(class(body), Class::Tool, "{body:?}");
        }
        let local = [
            Body::SendMessage(v1::SendMessage::default()),
            Body::UpdateSettings(v1::UpdateSettings { vault_unlock_minutes: Some(5), ..Default::default() }),
            Body::RevealSecret(v1::RevealSecret::default()),
            Body::SetSecretAskEachTime(v1::SetSecretAskEachTime::default()),
            Body::DecideSecretApproval(v1::DecideSecretApproval::default()),
            Body::RegisterNotifications(v1::RegisterNotifications::default()),
            Body::AnswerAsk(v1::AnswerAsk::default()),
            Body::LoadPipeMessages(v1::LoadPipeMessages::default()),
            Body::SelectPipe(v1::SelectPipe::default()),
            Body::StartLogin(v1::StartLogin::default()),
            // A model provider's accounts, not claktory.com's.
            Body::MoveAccount(v1::MoveAccount::default()),
            Body::UpdateAccount(v1::UpdateAccount::default()),
        ];
        for body in &local {
            assert_eq!(class(body), Class::Local, "{body:?}");
        }
        for body in [
            Body::AccountSignIn(v1::AccountSignIn::default()),
            Body::AccountVerify(v1::AccountVerify::default()),
            Body::AccountResend(v1::AccountResend::default()),
            Body::AccountSignInCancel(v1::AccountSignInCancel::default()),
        ] {
            assert_eq!(class(&body), Class::LocalBeforeFirstSignIn, "{body:?}");
        }
        assert_eq!(class(&Body::DecideProposal(v1::DecideProposal::default())), Class::LocalUnlessPipeGrant);
        let person = [
            Body::DecidePipeMessage(v1::DecidePipeMessage::default()),
            Body::ConfirmDevice(v1::ConfirmDevice::default()),
            Body::ApproveDevice(v1::ApproveDevice::default()),
            Body::CreatePairing(v1::CreatePairing::default()),
            Body::SetRemoteAccess(v1::SetRemoteAccess::default()),
            Body::ProveAllowedDevice(v1::ProveAllowedDevice::default()),
            Body::AccountSignOut(v1::AccountSignOut::default()),
            Body::SetAccountService(v1::SetAccountService::default()),
            Body::UseRecoveryKey(v1::UseRecoveryKey::default()),
            Body::ShowRecoveryCodes(v1::ShowRecoveryCodes::default()),
            Body::StartMove(v1::StartMove::default()),
            Body::ApprovePipe(v1::ApprovePipe::default()),
            Body::WatchAgentTab(v1::WatchAgentTab::default()),
            Body::AgentTabFrameAck(v1::AgentTabFrameAck::default()),
            Body::AgentTabInput(v1::AgentTabInput::default()),
            Body::ListDeviceActivity(v1::ListDeviceActivity::default()),
            Body::SendPipeMessage(v1::SendPipeMessage::default()),
            Body::SharePipeWebsite(v1::SharePipeWebsite::default()),
            Body::DmSend(v1::DmSend::default()),
            Body::SetAgentContained(v1::SetAgentContained::default()),
        ];
        for body in &person {
            assert_eq!(class(body), Class::Person, "{body:?}");
            assert!(!tool_allowed(body));
        }
    }

    /// Only the person turns an agent's browser on; turning it off, or any
    /// other extension on, is any UpdateAgent's.
    #[test]
    fn turning_an_agents_browser_on_is_the_persons() {
        let change = |id: &str, enabled: Option<bool>| {
            Body::UpdateAgent(v1::UpdateAgent {
                extension: Some(v1::ExtensionChange { id: id.into(), enabled, ..Default::default() }),
                ..Default::default()
            })
        };
        assert_eq!(class(&change("browser", Some(true))), Class::Person);
        let ordinary = class(&Body::UpdateAgent(v1::UpdateAgent::default()));
        assert_ne!(ordinary, Class::Person);
        assert_eq!(class(&change("browser", Some(false))), ordinary);
        assert_eq!(class(&change("browser", None)), ordinary);
        assert_eq!(class(&change("monitors", Some(true))), ordinary);
    }

    #[test]
    fn the_tool_token_is_kept_and_only_it_matches() {
        let dir = std::env::temp_dir().join(format!("authority-{}", std::process::id()));
        let home = Home::new(dir.clone());
        std::fs::create_dir_all(&dir).expect("home");
        prepare_tool_token(&home).expect("written");
        let token = read_tool_token(&home).expect("a token");
        prepare_tool_token(&home).expect("kept");
        assert_eq!(read_tool_token(&home).as_deref(), Some(token.as_str()));
        assert!(is_tool_token(&home, &token));
        assert!(!is_tool_token(&home, ""));
        assert!(!is_tool_token(&home, &"a".repeat(64)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join(TOOL_TOKEN)).expect("file").permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_long_home_puts_the_socket_in_a_folder_of_its_own() {
        let short = Home::new(PathBuf::from("/Users/lee/.factory"));
        assert_eq!(socket_path(&short), PathBuf::from("/Users/lee/.factory/app.sock"));
        let long = Home::new(PathBuf::from(format!("/Users/lee/{}/.factory", "x".repeat(120))));
        let path = socket_path(&long);
        assert!(path.as_os_str().len() <= SOCKET_PATH_MOST, "{}", path.display());
        assert!(path.parent().and_then(Path::file_name).is_some_and(|name| name.to_string_lossy().starts_with("claktory-")));
        assert_eq!(path, socket_path(&long), "the same every time");
    }
}
