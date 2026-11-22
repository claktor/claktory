//! One task per client: handshake, then requests in and events out.

use std::sync::Arc;
use std::time::Duration;

use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, ErrorCode, ServerMessage, Welcome, client_message, server_message::Body};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::factory::{AgentChange, Event, Factory, RequestError};
use crate::patch::Workspaces;
use crate::remote::{Authorized, Pairing};

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Pause after a failed accept (e.g. out of file descriptors) so the loop doesn't spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// A connection's bytes: plain TCP from this machine, TLS from a remote device.
pub(crate) trait Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Stream for T {}

type Socket<S> = WebSocketStream<S>;

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
}

/// Accepts clients forever, serving each connection on its own task.
pub async fn serve(listener: TcpListener, factory: Arc<Factory>) {
    if let Ok(addr) = listener.local_addr() {
        factory.remote.set_listener_port(addr.port());
    }
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let factory = factory.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_connection(stream, factory).await {
                        tracing::warn!(%peer, %error, "connection failed");
                    }
                });
            }
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
            }
        }
    }
}

/// Refuses a handshake from a web page, with 403: otherwise any page open in
/// the person's browser could drive the agents. Browsers always send `Origin`;
/// the app (dart:io) and Factory's own clients never do.
#[allow(clippy::result_large_err)] // The shape tungstenite's callback takes.
fn refuse_web_pages(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    let Some(origin) = request.headers().get(http::header::ORIGIN) else { return Ok(response) };
    tracing::warn!(?origin, "refused a WebSocket connection from a web page");
    let mut refusal = ErrorResponse::new(Some("Claktory doesn't accept connections from web pages.".into()));
    *refusal.status_mut() = http::StatusCode::FORBIDDEN;
    Err(refusal)
}

enum Frame {
    Message(Box<ClientMessage>),
    Closed,
    /// Nothing for the protocol (control frame, or garbage already answered).
    Skip,
}

async fn serve_connection(stream: TcpStream, factory: Arc<Factory>) -> Result<(), ConnectionError> {
    // Protocol messages are small and latency-sensitive; don't let Nagle batch them.
    stream.set_nodelay(true)?;
    serve_stream(stream, factory, None).await
}

/// Serves one client: on the loopback port, or a device through the
/// private tunnel (`peer`), which must give a paired device's token.
pub(crate) async fn serve_stream<S: Stream>(
    stream: S,
    factory: Arc<Factory>,
    peer: Option<crate::remote::Peer>,
) -> Result<(), ConnectionError> {
    let mut socket = match tokio_tungstenite::accept_hdr_async(stream, refuse_web_pages).await {
        Ok(socket) => socket,
        // Port checks and stray connections that never speak WebSocket.
        Err(error) => {
            tracing::debug!(%error, "not a WebSocket client");
            return Ok(());
        }
    };

    // How it reached this Claktory, for diagnostics.
    let via = match &peer {
        None => "port",
        Some(peer) if peer.loopback => "this_computer",
        Some(peer) if peer.address.is_some() => "direct",
        Some(_) => "relay",
    };
    let device;
    let kind;
    let workspace_patches;
    let (platform, app_version);
    let hello_id = loop {
        let frame = socket.next().await;
        let message = match read(&mut socket, frame).await? {
            Frame::Message(message) => message,
            Frame::Skip => continue,
            Frame::Closed => return Ok(()),
        };
        let Some(client_message::Body::Hello(hello)) = message.body else {
            return reject(socket, message.id, ErrorCode::HandshakeRequired, "first message must be hello").await;
        };
        if hello.protocol_version != PROTOCOL_VERSION {
            let reason = format!("server speaks protocol {PROTOCOL_VERSION}, client sent {}", hello.protocol_version);
            return reject(socket, message.id, ErrorCode::UnsupportedProtocolVersion, reason).await;
        }
        kind = hello.device_kind();
        workspace_patches = hello.workspace_patches;
        (platform, app_version) = (hello.platform.clone(), hello.client_version.clone());
        let knock = crate::remote::Knock {
            token: &hello.token,
            assertion: &hello.assertion,
            can_confirm: hello.can_confirm,
            kind,
            name: &hello.device_name,
            platform: &hello.platform,
            app_version: &hello.client_version,
        };
        if let Some(peer) = &peer {
            let connected = match factory.authorize(&knock, peer).await {
                Authorized::Connected(connected) => connected,
                Authorized::Pending(waiting) => {
                    // Remote access stays on until it's heard how this ends.
                    let _waiting = factory.waiting();
                    match wait_to_be_allowed(&mut socket, &factory, message.id, &waiting, peer).await? {
                        Pairing::Allowed => match factory.authorize(&knock, peer).await {
                            Authorized::Connected(connected) => connected,
                            _ => {
                                let reason = "This device was allowed, then removed.";
                                return reject(socket, message.id, ErrorCode::Unauthorized, reason).await;
                            }
                        },
                        Pairing::Ended(reason) => return reject(socket, message.id, ErrorCode::Unauthorized, reason).await,
                        // It went away while it waited.
                        Pairing::Waiting => return Ok(()),
                    }
                }
                Authorized::Refused => {
                    crate::diagnostics::turned_away(&factory, via, "unpaired");
                    let reason = "This device is no longer allowed to connect. Add it again in Settings → Devices on the computer that runs Claktory.";
                    return reject(socket, message.id, ErrorCode::Unauthorized, reason).await;
                }
                Authorized::Turned(code, reason) => {
                    let why = code.as_str_name().trim_start_matches("ERROR_CODE_").to_lowercase();
                    crate::diagnostics::turned_away(&factory, via, &why);
                    return reject(socket, message.id, code, reason).await;
                }
            };
            tracing::info!(client_version = %hello.client_version, kind = ?kind, device = %connected.device.id, endpoint = %peer.endpoint_id, "device connected");
            device = connected;
        } else {
            // The loopback port, which any user of this machine can reach:
            // only this machine's app's token (from its local pairing code,
            // which only this user can read) lets a client in.
            let local = crate::remote::Peer { endpoint_id: String::new(), loopback: true, address: None };
            let Authorized::Connected(connected) = factory.authorize(&knock, &local).await else {
                // Another user's program, or one of this user's without the
                // token: an app build or test made without FACTORY_PORT, say.
                tracing::info!(client_version = %hello.client_version, kind = ?kind, token = !hello.token.is_empty(), "refused a client on the loopback port without this machine's token");
                crate::diagnostics::turned_away(&factory, via, if hello.token.is_empty() { "no_token" } else { "wrong_token" });
                let reason = format!(
                    "This port wants a token: connect with the pairing code in {}.",
                    factory.home.root().join("remote/local-pairing").display()
                );
                return reject(socket, message.id, ErrorCode::Unauthorized, reason).await;
            };
            tracing::info!(client_version = %hello.client_version, kind = ?kind, "client connected with this machine's token");
            device = connected;
        }
        break message.id;
    };
    // Only a client let in counts as an app trying an updated backend: one
    // turned away (another user's, say) can't make an update roll back.
    factory.health.connecting();
    crate::account::analytics::device_connected(&factory.store.db(), &device.device.id);
    let claktory = factory.identity();
    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION,
        server_version: SERVER_VERSION.to_owned(),
        device_id: device.device.id.clone(),
        name: claktory.name,
        color: claktory.color,
        confirm_by_ms: device.confirm_by,
    };
    send(&mut socket, hello_id, Body::Welcome(welcome)).await?;
    // This machine's own app may do what the loopback port's may.
    let remote = !device.device.local;
    // Where the person's messages come from: a paired device by its name,
    // this computer's own app by the computer's.
    let from = if remote { device.device.name.clone() } else { crate::remote::machine_name().to_owned() };
    let sent_from = crate::transcript::device(&from, kind);
    let files = crate::explorer::Watches::watch(&factory);
    let version: String = app_version.chars().take(40).map(|c| if c.is_whitespace() || c == '=' { '_' } else { c }).collect();
    let connection = Connection {
        remote,
        device: device.device.name.clone(),
        sent_from,
        binding: device.device.id.clone(),
        zoom_device: device.device.id.clone(),
        endpoint_id: peer.as_ref().map(|peer| peer.endpoint_id.clone()).unwrap_or_default(),
        workspace_patches,
        files,
        views: crate::apps::Viewer::new(&factory),
        browser: crate::browser::Port::new(&factory, &device.device.id),
        presence: factory.notify.connect(&device.device.id),
        presentation: crate::presentation::Port::new(&factory, &device.device.id),
        dictations: crate::speech::dictation::Port::new(),
        diagnostics: crate::diagnostics::Source::device(&device.device.id, from, kind as i32, platform, app_version),
    };
    let (id, since) = (device.device.id.clone(), std::time::Instant::now());
    crate::diagnostics::record(&factory, crate::diagnostics::CONNECTION_AREA, "connected", &format!("device={id} via={via} app={version}"), 0);
    let ended = session(&mut socket, &factory, connection, device.closed.clone()).await;
    let how = match &ended {
        Ok(()) => "closed".to_owned(),
        Err(ConnectionError::Io(error) | ConnectionError::WebSocket(tungstenite::Error::Io(error))) => {
            format!("io_{:?}", error.kind()).to_lowercase()
        }
        Err(ConnectionError::WebSocket(tungstenite::Error::Protocol(_))) => "protocol".to_owned(),
        Err(ConnectionError::WebSocket(_)) => "websocket".to_owned(),
    };
    let stayed = i64::try_from(since.elapsed().as_millis()).unwrap_or(i64::MAX);
    crate::diagnostics::record(&factory, crate::diagnostics::CONNECTION_AREA, "disconnected", &format!("device={id} via={via} end={how}"), stayed);
    ended?;
    tracing::info!("client disconnected");
    Ok(())
}

/// Who's on the other end of a connection.
struct Connection {
    /// A device on another machine.
    remote: bool,
    /// The device's name, for the vault.
    device: String,
    /// Where the person's messages come from.
    sent_from: factory_agent::SentFrom,
    /// What a flow connecting a service is bound to: the paired device's id.
    binding: String,
    /// Whose zoom SetZoom sets: the paired device's id, "local" for this
    /// machine's own app. Also who confirms with ConfirmDevice.
    zoom_device: String,
    /// The tunnel key it connected with; empty on the loopback port.
    endpoint_id: String,
    /// The app applies WorkspacePatch.
    workspace_patches: bool,
    /// The project whose files it shows on their own (WatchFiles).
    files: crate::explorer::Watch,
    /// The app views it shows (OpenAppView); their messages come through it.
    views: crate::apps::Viewer,
    /// Whether the person is using its app now (SetActive).
    presence: crate::notify::Presence,
    /// Browser control's commands for this connection's app alone, when it
    /// brokers for its Chrome (`crate::browser`).
    browser: crate::browser::Port,
    /// Where this device was reading, whenever another of its connections
    /// changes it (`crate::presentation`).
    presentation: crate::presentation::Port,
    /// Who sends its diagnostics (`crate::diagnostics`).
    diagnostics: crate::diagnostics::Source,
    /// What its TranscribeAudio requests heard, for it alone.
    dictations: crate::speech::dictation::Port,
}

/// Requests in, events out, until the client leaves. `unpaired` is the
/// device's: notified, the connection closes.
async fn session<S: Stream>(
    socket: &mut Socket<S>,
    factory: &Arc<Factory>,
    connection: Connection,
    unpaired: Arc<tokio::sync::Notify>,
) -> Result<(), ConnectionError> {
    let mut vault_session = crate::vault::requests::Session::new(connection.device.clone(), connection.zoom_device.clone());
    let mut sent = Workspaces::new(connection.workspace_patches);
    let mut events = sync(socket, &mut sent, factory).await?;
    // Where this device was reading, for it alone, when it kept anything.
    if let Some(presentation) = crate::presentation::welcome(factory, &connection.binding) {
        send(socket, 0, Body::Presentation(presentation)).await?;
    }
    factory.health.served();
    // A vault still locked after a restart tries the account service again.
    tokio::spawn({
        let factory = factory.clone();
        async move { crate::account::unlock_now(&factory).await }
    });

    loop {
        tokio::select! {
            () = unpaired.notified() => {
                tracing::info!("closing an unpaired device's connection");
                socket.close(None).await?;
                return Ok(());
            }
            frame = socket.next() => {
                let message = match read(socket, frame).await? {
                    Frame::Message(message) => message,
                    Frame::Skip => continue,
                    Frame::Closed => return Ok(()),
                };
                let reply = handle(factory, message.body, &connection, &mut vault_session).await;
                // Deliver what the request changed before answering it, so a
                // client that sees the ack also sees the new state.
                flush(socket, &mut sent, &mut events, factory).await?;
                send(socket, message.id, reply).await?;
            }
            body = connection.views.next() => send(socket, 0, body).await?,
            body = connection.browser.next() => send(socket, 0, body).await?,
            body = connection.presentation.next() => send(socket, 0, body).await?,
            body = connection.dictations.next() => send(socket, 0, body).await?,
            event = events.recv() => match event {
                Ok(event) => forward(socket, &mut sent, event).await?,
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "client fell behind; resending state");
                    events = sync(socket, &mut sent, factory).await?;
                }
                Err(RecvError::Closed) => return Ok(()),
            },
        }
    }
}

/// Sends the full current state, the Workspace whole, and returns the events
/// that follow it.
async fn sync<S: Stream>(socket: &mut Socket<S>, sent: &mut Workspaces, factory: &Factory) -> Result<Receiver<Event>, ConnectionError> {
    let snapshot = factory.connect().await;
    sent.reset();
    let workspace = ServerMessage { reply_to: 0, body: Some(Body::Workspace(snapshot.workspace)) };
    forward(socket, sent, workspace.encode_to_vec().into()).await?;
    if let Some(transcript) = snapshot.transcript {
        send(socket, 0, Body::Transcript(transcript)).await?;
    }
    for explorer in snapshot.explorers {
        send(socket, 0, Body::Explorer(explorer)).await?;
    }
    if let Some(view) = snapshot.view {
        send(socket, 0, Body::ExtensionView(view)).await?;
    }
    if let Some(queued) = snapshot.queued {
        send(socket, 0, Body::QueuedMessages(queued)).await?;
    }
    if let Some((transcript, queued)) = snapshot.chat {
        send(socket, 0, Body::ChatTranscript(transcript)).await?;
        if !queued.messages.is_empty() {
            send(socket, 0, Body::ChatQueuedMessages(queued)).await?;
        }
    }
    Ok(snapshot.events)
}

async fn flush<S: Stream>(
    socket: &mut Socket<S>,
    sent: &mut Workspaces,
    events: &mut Receiver<Event>,
    factory: &Factory,
) -> Result<(), ConnectionError> {
    loop {
        match events.try_recv() {
            Ok(event) => forward(socket, sent, event).await?,
            Err(TryRecvError::Empty | TryRecvError::Closed) => return Ok(()),
            Err(TryRecvError::Lagged(missed)) => {
                tracing::warn!(missed, "client fell behind; resending state");
                *events = sync(socket, sent, factory).await?;
                return Ok(());
            }
        }
    }
}

/// Answers a request. A remote device's requests are the same, except that
/// it can't have an app bundle on this machine replaced.
async fn handle(
    factory: &Arc<Factory>,
    body: Option<client_message::Body>,
    connection: &Connection,
    vault_session: &mut crate::vault::requests::Session,
) -> Body {
    let remote = connection.remote;
    use crate::vault::requests as vault;
    use client_message::Body as Request;
    let result = match body {
        Some(Request::Hello(_)) => return error(ErrorCode::InvalidRequest, "already connected"),
        Some(Request::CreateProject(request)) => factory.create_project(&request.name, &request.path).await,
        Some(Request::CreateAgent(request)) => factory.create_agent(&request).await,
        Some(Request::UpdateAgent(request)) => {
            // `folder` (an absolute path) is deprecated and ignored.
            let v1::UpdateAgent {
                agent_id,
                name,
                prompt,
                model,
                effort,
                project_id,
                color,
                extension,
                placeholder,
                folder: _,
                folder_id,
                avatar_shape,
                // Faces are gone: ignored.
                avatar_face: _,
                role,
                lead_id,
                fallback_model,
                notify,
            } = request;
            let change = AgentChange {
                name: name.as_deref(),
                prompt: prompt.as_deref(),
                model: model.as_deref(),
                effort: effort.as_deref(),
                project: project_id.as_deref(),
                color: color.as_deref(),
                extension: extension.as_ref(),
                placeholder: placeholder.as_deref(),
                folder: folder_id.as_deref(),
                avatar_shape: avatar_shape.as_deref(),
                role: role.as_deref(),
                lead: lead_id.as_deref(),
                fallback_model: fallback_model.as_deref(),
                notify: notify.map(|mode| v1::NotifyMode::try_from(mode).unwrap_or_default()),
            };
            factory.update_agent(&agent_id, change).await
        }
        Some(Request::SelectAgent(request)) => factory.select_agent(&request.agent_id).await,
        Some(Request::SendMessage(request)) if request.voice && request.checklist_item != 0 => {
            Err(RequestError::Checklist("Type your reply. Voice messages can't be used here.".to_owned()))
        }
        Some(Request::SendMessage(request)) if request.voice => crate::speech::voice::send(
            factory,
            &request.agent_id,
            &request.text,
            Some(connection.sent_from.clone()),
            &request.client_id,
            &request.attachments,
        ),
        Some(Request::SendMessage(request)) => factory.send_message_about(
            &request.agent_id,
            &request.text,
            Some(connection.sent_from.clone()),
            &request.client_id,
            &request.attachments,
            (request.checklist_item != 0).then_some((request.checklist_agent_id.as_str(), request.checklist_item)),
        ),
        Some(Request::StopAgent(request)) => factory.stop_agent(&request.agent_id),
        Some(Request::DeleteAgent(request)) => factory.delete_agent(&request.agent_id).await,
        Some(Request::UpdateProject(request)) => {
            factory.update_project(&request.project_id, request.name.as_deref(), request.archived).await
        }
        Some(Request::DeleteProject(request)) => {
            let deleted = factory.delete_project(&request.project_id).await;
            if deleted.is_ok() {
                crate::pipes::project_gone(factory, &request.project_id);
            }
            deleted
        }
        Some(Request::UpdateSettings(request)) => factory.update_settings(request).await,
        Some(Request::ShuffleAvatar(request)) => factory.shuffle_avatar(request).await,
        Some(Request::ScreenOpened(answer)) => {
            crate::extensions::highlight::screen_opened(&answer.id, &connection.sent_from.device, answer.opened);
            Ok(())
        }
        Some(Request::SetZoom(request)) => factory.set_zoom(&connection.zoom_device, request.zoom).await,
        Some(Request::ConfirmDevice(request)) => factory.confirm_device(&connection.zoom_device, &connection.endpoint_id, &request).await,
        Some(Request::RestoreSettings(_)) => factory.restore_settings().await,
        Some(Request::CheckForUpdates(_)) => factory.check_for_updates(),
        Some(Request::SetDiagnostics(request)) => crate::diagnostics::set(factory, &request),
        Some(Request::SendDiagnostics(request)) => crate::diagnostics::receive(factory, &connection.diagnostics, request),
        Some(Request::AcceptPipeIdentity(request)) => crate::pipes::identity::accept(factory, &request.pipe_id),
        // Automated pipes (pipes-automation.md).
        Some(Request::SetPipeForward(request)) => crate::pipes::handoff::set_forward_request(factory, &request).await,
        Some(Request::SetReportReader(request)) => crate::pipes::handoff::set_reader_request(factory, &request).await,
        Some(Request::SaveReportType(request)) => crate::pipes::handoff::save_type_request(factory, &request),
        Some(Request::DeleteReportType(request)) => crate::pipes::handoff::delete_type_request(factory, &request),
        Some(Request::DecidePipeReports(request)) => {
            crate::pipes::handoff::decide_waiting(factory, &request.pipe_id, request.send).map_err(RequestError::Remote)
        }
        Some(Request::ListPipeReports(request)) => {
            return answer(crate::pipes::handoff::list(factory, &request), Body::PipeReportPage);
        }
        // Direct messages end to end (dms-e2ee.md §2): this Claktory's leaf,
        // given only to the person's own devices, which are all that connect
        // here (a paired device, or this computer's app with its token).
        Some(Request::DmLeafOffer(_) | Request::DmEnrollLeaf(_)) if !crate::user_apps::dms::preview() => {
            return error(ErrorCode::InvalidRequest, "Direct messages aren't available in this Claktory yet.");
        }
        Some(Request::DmLeafOffer(_)) => {
            return match crate::user_apps::dms::leaf::offer(factory) {
                Ok(offer) => Body::DmLeafOffer(offer),
                Err(why) => error(ErrorCode::InvalidRequest, why),
            };
        }
        Some(Request::DmEnrollLeaf(request)) => {
            return match crate::user_apps::dms::leaf::enroll(factory, &request) {
                Ok(()) => Body::Ack(v1::Ack {}),
                Err(why) => error(ErrorCode::InvalidRequest, why),
            };
        }
        // A remote device's app is on another machine: its path means nothing here.
        Some(Request::InstallUpdate(request)) => factory.install_update(if remote { "" } else { &request.app_path }).await,
        Some(Request::FocusProject(request)) => factory.focus_project(&request.project_id).await,
        Some(Request::ExpandFolder(request)) => {
            factory.expand_folder(&request.project_id, &request.folder_id, &request.agent_id, &request.path, request.expanded).await
        }
        Some(Request::ShowAllFiles(request)) => factory.show_all_files(&request.project_id, request.show_all).await,
        Some(Request::OpenPanel(request)) => factory.open_panel(&request.agent_id, &request.extension_id).await,
        Some(Request::ArrangeAgent(request)) => {
            let v1::ArrangeAgent { agent_id, parent_id, before_id, project_id, folder_id } = request;
            factory.arrange_agent(&agent_id, &parent_id, &before_id, project_id.as_deref(), folder_id.as_deref()).await
        }
        Some(Request::SetAgentCollapsed(request)) => factory.set_agent_collapsed(&request.agent_id, request.collapsed),
        Some(Request::CreateFolder(request)) => factory.create_folder(&request.project_id, &request.name, &request.path).await,
        Some(Request::UpdateFolder(request)) => {
            let v1::UpdateFolder { project_id, folder_id, name, path, before_id } = request;
            factory.update_folder(&project_id, &folder_id, name.as_deref(), path.as_deref(), before_id.as_deref()).await
        }
        Some(Request::DeleteFolder(request)) => factory.delete_folder(&request.project_id, &request.folder_id).await,
        Some(Request::SetHighlights(request)) => factory.set_highlights(request.on).await,
        Some(Request::StopTask(request)) => factory.stop_task(&request.agent_id, &request.task_id),
        Some(Request::ChangeMonitor(request)) => factory.change_monitor(&request.agent_id, request.monitor_id, request.action()),
        Some(Request::SetFolderCollapsed(request)) => {
            factory.set_folder_collapsed(&request.project_id, &request.folder_id, request.collapsed)
        }
        Some(Request::RunCommand(request)) => factory.run_command(&request.agent_id, &request.name, &request.args).await,
        Some(Request::StartLogin(request)) => factory.start_login(&request.provider_id, &request.method_id, request.add_account).await,
        Some(Request::AnswerLogin(request)) => factory.answer_login(&request.text),
        Some(Request::CancelLogin(_)) => {
            factory.cancel_login();
            Ok(())
        }
        Some(Request::SignOut(request)) => factory.sign_out(&request.provider_id, &request.account_id).await,
        Some(Request::MoveAccount(request)) => factory.move_account(&request.provider_id, &request.account_id, request.position).await,
        Some(Request::UpdateAccount(request)) => {
            factory.update_account(&request.provider_id, &request.account_id, request.label.as_deref(), request.retry).await
        }
        Some(Request::SetOnboarding(request)) => factory.set_onboarding(&request),
        Some(Request::ShowPairingCode(request)) => {
            return match crate::extensions::devices::code(factory, &request.pairing_id).await {
                Ok(code) => Body::PairingCode(code),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::CreatePairing(request)) => {
            return match factory.create_pairing(&request.name).await {
                Ok(code) => Body::PairingCode(code),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::StartUpload(request)) => {
            return match factory.start_upload(request) {
                Ok(progress) => Body::UploadProgress(progress),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::UploadChunk(request)) => {
            return match factory.upload_chunk(request) {
                Ok(progress) => Body::UploadProgress(progress),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::FinishUpload(request)) => {
            return match factory.finish_upload(request) {
                Ok(uploaded) => Body::UploadedFile(uploaded),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::CancelUpload(request)) => {
            factory.cancel_upload(&request);
            Ok(())
        }
        Some(Request::FetchApp(request)) => {
            return match factory.fetch_app(&request.version, request.offset).await {
                Ok(chunk) => Body::AppChunk(chunk),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(
            Request::InstallLocalApp(_)
            | Request::SetAppRunning(_)
            | Request::UninstallApp(_)
            | Request::GrantApp(_)
            | Request::SubscribeApp(_)
            | Request::SetAppSecret(_)
            | Request::GrantAppSource(_)
            | Request::PinApp(_),
        ) if !factory.apps.available() => return error(ErrorCode::InvalidRequest, "Apps aren't available in this Claktory yet."),
        Some(Request::InstallLocalApp(request)) => {
            return match factory.install_local_app(&request).await {
                Ok(Some(permissions)) => Body::AppPermissions(permissions),
                Ok(None) => Body::Ack(v1::Ack {}),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::SetAppRunning(request)) => factory.set_app_running(&request).await,
        Some(Request::UninstallApp(request)) => factory.uninstall_app(&request).await,
        Some(Request::GrantApp(request)) => factory.grant_app(&request).await,
        Some(Request::SubscribeApp(request)) => factory.subscribe_app(&request).await,
        Some(Request::SetAppSecret(request)) => factory.set_app_secret(&request).await,
        Some(Request::GrantAppSource(request)) => factory.grant_app_source(&request).await,
        Some(Request::PinApp(request)) => factory.pin_app(&request),
        Some(Request::OpenAppView(request)) => return factory.open_app_view(&connection.views, &request),
        Some(Request::AppViewAction(request)) => return factory.app_view_action(&request),
        Some(Request::CloseAppView(request)) => return factory.close_app_view(&connection.views, &request.surface_id),
        Some(Request::ListNeedsYouHistory(request)) => {
            return match factory.needs_you_history(&request) {
                Ok(history) => Body::NeedsYouHistory(history),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::LoadUsage(request)) => {
            return match factory.load_usage(&request) {
                Ok(report) => Body::UsageReport(report),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::ListThreads(request)) => {
            return match factory.list_threads(&request.path) {
                Ok(list) => Body::ThreadList(list),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::CreateThread(request)) => {
            return match factory.create_thread(&request) {
                Ok(thread) => Body::Thread(thread),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::ReplyThread(request)) => factory.reply_thread(&request),
        Some(Request::EditComment(request)) => factory.edit_comment(&request),
        Some(Request::DeleteComment(request)) => factory.delete_comment(&request),
        Some(Request::SetThreadStatus(request)) => factory.set_thread_status(&request),
        Some(Request::ReaddressThread(request)) => factory.readdress_thread(&request),
        Some(Request::ReattachThread(request)) => factory.reattach_thread(&request),
        Some(Request::SendComments(request)) => factory.send_comments(&request),
        Some(Request::DecideSuggestion(request)) => factory.decide_suggestion(&request),
        Some(Request::WatchFiles(request)) => factory.watch_files(&connection.files, &request.project_id).await,
        Some(Request::LoadEntries(request)) => {
            return match factory.load_entries(&request).await {
                Ok(page) => Body::EntryPage(page),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::ListFolders(request)) => {
            return match factory.list_folders(&request.path).await {
                Ok(list) => Body::FolderList(list),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::CreateVault(request)) => {
            return answer(vault::create_vault_in(factory, vault_session, &request), Body::RecoveryKey);
        }
        Some(Request::UnlockVault(request)) => {
            return answer(vault::unlock_in(factory, vault_session, &request.passphrase).await, Body::VaultSession);
        }
        // Nothing to lock: requests need no proof.
        Some(Request::LockVaultSession(_)) => return Body::VaultSession(vault_session.answer()),
        Some(Request::ChangePassphrase(request)) => {
            return answer(vault::change_passphrase_in(factory, vault_session, &request).await, Body::VaultSession);
        }
        Some(Request::CreateSecret(request)) => vault::create_secret(factory, &request),
        Some(Request::UpdateSecret(request)) => vault::update_secret(factory, &request),
        Some(Request::RevokeSecret(request)) => vault::revoke_secret(factory, &request),
        Some(Request::ImportEnv(request)) => vault::import_env(factory, &request),
        Some(Request::PreviewEnv(request)) => return Body::EnvPreview(vault::preview_env(&request.text)),
        Some(Request::ListSecretUses(request)) => return answer(vault::list_uses(factory, &request), Body::SecretUses),
        Some(Request::DeleteSecret(request)) => {
            return answer(vault::delete_secret(factory, vault_session, &request).await, Body::VaultSession);
        }
        Some(Request::GrantSecret(request)) => {
            return answer(vault::grant_secret(factory, vault_session, &request).await, Body::VaultSession);
        }
        Some(Request::SetSecretAskEachTime(request)) => {
            return answer(vault::set_ask_each_time(factory, vault_session, &request).await, Body::VaultSession);
        }
        Some(Request::RevealSecret(request)) => {
            return answer(vault::reveal_secret(factory, vault_session, &request).await, Body::SecretValue);
        }
        Some(Request::NewRecoveryKey(request)) => {
            return answer(vault::new_recovery_key(factory, vault_session, &request).await, Body::RecoveryKey);
        }
        Some(Request::SetVaultLockedMode(request)) => {
            return answer(vault::set_locked_mode(factory, vault_session, &request).await, Body::VaultSession);
        }
        Some(Request::DecideSecretRequest(request)) => vault::decide_request(factory, &request),
        Some(Request::AccountSignIn(request)) => crate::account::sign_in(factory, &request.email).await,
        Some(Request::AccountVerify(request)) => crate::account::verify(factory, &request.code).await,
        Some(Request::AccountResend(_)) => crate::account::resend(factory).await,
        Some(Request::AccountSignOut(_)) => crate::account::sign_out(factory).await,
        Some(Request::SkipAccountSignIn(_)) => crate::account::skip(factory),
        // Pipes (#196): `crate::pipes`.
        Some(Request::CreatePipeInvite(request)) => return answer(crate::pipes::create_invite(factory, &request).await, Body::PipeInvite),
        Some(Request::ReadPipeInvite(request)) => return answer(crate::pipes::read_invite(&request.code), Body::PipeInvitePreview),
        Some(Request::AcceptPipeInvite(request)) => {
            crate::pipes::accept(factory, &request).inspect(|()| crate::chats::leave_for_pipe(factory))
        }
        Some(Request::ApprovePipe(request)) => crate::pipes::approve(factory, &request).await,
        Some(Request::UpdatePipe(request)) => crate::pipes::update(factory, &request),
        Some(Request::SetPipeState(request)) => crate::pipes::set_pipe_state(factory, &request),
        Some(Request::SelectPipe(request)) => {
            crate::pipes::select(factory, &request.pipe_id).await.inspect(|()| crate::chats::leave_for_pipe(factory))
        }
        Some(Request::SendPipeMessage(request)) => crate::pipes::send_person(factory, &request),
        Some(Request::DecidePipeMessage(request)) => crate::pipes::decide_message(factory, &request),
        Some(Request::LoadPipeMessages(request)) => return answer(crate::pipes::load_messages(factory, &request), Body::PipeMessagePage),
        Some(Request::DecideSecretApproval(request)) => crate::pipes::agents::decide(factory, &request).await,
        Some(Request::AddPipeLiaison(request)) => crate::pipes::add_liaison(factory, &request).await,
        Some(Request::DismissPipeStart(request)) => crate::pipes::start::dismiss(factory, &request),
        Some(Request::DecideAppGrant(request)) => crate::user_apps::decide(factory, &request).await,
        // Browser control: each binds to this connection itself, never to
        // anything the message says about who sent it.
        Some(Request::BrowserCapability(request)) => connection.browser.capability(&request),
        Some(Request::BrowserGrant(request)) => connection.browser.grant(&request),
        Some(Request::BrowserResult(request)) => connection.browser.result(&request),
        Some(Request::BrowserHeartbeat(request)) => connection.browser.heartbeat(&request),
        Some(Request::BrowserStop(request)) => connection.browser.stop(&request),
        Some(Request::ListBrowserLog(request)) => return answer(factory.browser_log(&request), Body::BrowserLog),
        Some(Request::ClearBrowserLog(_)) => factory.clear_browser_log(),
        // Ink (#209 phase 3): `crate::ink`.
        Some(Request::ReadInkBlob(request)) => return answer(factory.read_ink_blob(&request), Body::InkBlobData),
        Some(Request::ListInkLayers(request)) => return answer(factory.list_ink_layers(&request), Body::InkLayerList),
        Some(Request::SaveInkLayer(request)) => {
            let saver = crate::ink::layers::Saver { device_id: &connection.binding, device_kind: &connection.sent_from.kind };
            return answer(factory.save_ink_layer(&request, &saver), Body::InkLayerSaved);
        }
        Some(Request::DeleteInkLayer(request)) => factory.delete_ink_layer(&request),
        Some(Request::SetPresentation(request)) => crate::presentation::set(factory, &connection.binding, &request),
        Some(Request::CreateSketch(request)) => return answer(crate::sketch::create(factory, request).await, Body::SketchCreated),
        // Chats (#213): `crate::chats`.
        Some(Request::CreateChat(request)) => {
            crate::chats::create(factory, &request.text, &request.client_id, Some(connection.sent_from.clone())).await
        }
        Some(Request::SelectChat(request)) => crate::chats::select(factory, &request.chat_id).await,
        Some(Request::SendChatMessage(request)) => {
            crate::chats::send(factory, &request.chat_id, &request.text, &request.client_id, Some(connection.sent_from.clone()))
        }
        Some(Request::StopChat(request)) => crate::chats::stop(factory, &request.chat_id),
        Some(Request::DeleteChat(request)) => crate::chats::delete(factory, &request.chat_id),
        Some(Request::LoadChatEntries(request)) => return answer(crate::chats::load_entries(factory, &request).await, Body::ChatEntryPage),
        Some(Request::SearchConversation(request)) => {
            return answer(crate::find::search(factory, &request).await, Body::ConversationMatches);
        }
        Some(Request::RenameChat(request)) => crate::chats::rename(factory, &request.chat_id, &request.title),
        // Moving this Claktory (#217): `crate::move_run`.
        Some(Request::StartMove(request)) => crate::move_run::start(factory, &request.code),
        Some(Request::ConfirmMove(_)) => crate::move_run::confirm(factory),
        Some(Request::CancelMove(_)) => crate::move_run::cancel(factory),
        Some(Request::FreezeMove(_)) => crate::move_run::freeze(factory),
        Some(Request::InstallSpeechModel(_)) => crate::speech::model::install(factory),
        Some(Request::RemoveSpeechModel(_)) => crate::speech::model::remove(factory),
        Some(Request::RetryVoiceMessage(request)) => crate::speech::voice::retry(factory, &request.agent_id, request.message_id),
        Some(Request::DiscardVoiceMessage(request)) => crate::speech::voice::discard(factory, &request.agent_id, request.message_id),
        Some(Request::TranscribeAudio(request)) => crate::speech::dictation::request(factory, &request, &connection.dictations),
        Some(Request::AnswerAsk(request)) => crate::extensions::ask::answer(factory, &request).await,
        Some(Request::SetSpendCap(request)) => crate::pipes::spend::set_caps(factory, &request),
        Some(Request::ReportReply(request)) => return answer(crate::account::reports::report(factory, request).await, Body::ReplyReport),
        Some(Request::SetAccountService(request)) => crate::account::set_service(factory, &request.url).await,
        Some(Request::ShowRecoveryKey(_)) => return answer(vault::show_recovery_key(factory, vault_session), Body::RecoveryKey),
        Some(Request::CheckRecoveryKey(_)) => return answer(vault::check_recovery_key(factory, vault_session), Body::RecoveryKeyCheck),
        Some(Request::ConfirmRecoveryKey(request)) => vault::confirm_recovery_key(factory, vault_session, &request),
        Some(Request::DecideProposal(request)) => {
            // A chat's proposal names the chat instead of an agent.
            let from = if request.chat_id.is_empty() { &request.agent_id } else { &request.chat_id };
            crate::proposals::decide(factory, from, &request.proposal_id, request.approve, &request.comment)
        }
        Some(Request::ClearBuildCache(request)) => {
            crate::extensions::tree::caches::clear_on_request(factory, &request.path).map(|_| ()).map_err(RequestError::BuildCache)
        }
        Some(Request::ReadFile(request)) => {
            let content =
                tokio::task::spawn_blocking(move || crate::extensions::viewer::read_requested(&request))
                    .await;
            return match content {
                Ok(content) => Body::FileContent(content),
                Err(problem) => error(ErrorCode::Internal, problem.to_string()),
            };
        }
        Some(Request::SetLayout(request)) => {
            crate::extensions::viewer::set_layout(factory, &request.agent_id, request.root, request.focus.as_deref())
        }
        Some(Request::AddGoal(request)) => crate::extensions::checklist::person_add_goal(factory, &request),
        Some(Request::UpdateGoal(request)) => crate::extensions::checklist::person_update_goal(factory, &request),
        // Only a Claktory that moved away answers this (move_import::moved).
        Some(Request::DeleteMovedCopy(_)) => return error(ErrorCode::InvalidRequest, "This Claktory hasn't moved to another computer."),
        Some(Request::UnpairDevice(request)) => factory.unpair_device(&request.device_id).await,
        Some(Request::ApproveDevice(request)) => factory.approve_device(&request.device_id).await,
        Some(Request::DenyDevice(request)) => factory.deny_device(&request.device_id).await,
        Some(Request::CancelPairing(request)) => factory.cancel_pairing(&request.device_id).await,
        // Remote access is on while devices are paired: only the relays are set here.
        Some(Request::SetRemoteAccess(request)) if request.set_relays => factory.set_relays(&request.relays).await,
        Some(Request::SetRemoteAccess(_)) => Ok(()),
        // Empty, or from a newer client that this server doesn't know.
        Some(Request::StartConnect(request)) => {
            return match factory.start_connect(&request, &connection.binding, &connection.sent_from.device).await {
                Ok(started) => Body::ConnectStarted(started),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::FinishConnect(request)) => factory.finish_connect(&request, &connection.binding),
        Some(Request::CancelConnect(request)) => {
            factory.cancel_connect(&request.flow_id);
            Ok(())
        }
        Some(Request::DismissConnectProposal(request)) => crate::connectors::proposals::dismiss(factory, &request),
        Some(Request::ListConnectChoices(request)) => {
            return match factory.connect_choices(&request.secret_id).await {
                Ok(choices) => Body::ConnectChoices(choices),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::ListQuietCopies(request)) => {
            return match crate::extensions::agents::provenance::list(factory, &request) {
                Ok(copies) => Body::QuietCopies(copies),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        Some(Request::DeriveSecret(request)) => factory.derive_secret(&request, "").await.map(|_| ()),
        Some(Request::DisconnectService(request)) => factory.disconnect_service(&request.secret_id, request.revoke).await,
        Some(Request::UseUsageReset(request)) => {
            return match factory.use_usage_reset(&request.provider_id, &request.account_id, &request.reset_id).await {
                Ok(answer) => Body::UsageResetAnswer(answer),
                Err(problem) => error(problem.code(), problem.to_string()),
            };
        }
        // Notifications: `crate::notify`.
        Some(Request::SetActive(request)) => {
            connection.presence.set_active(request.active);
            Ok(())
        }
        Some(Request::RegisterNotifications(request)) => crate::notify::push::register(factory, &connection.zoom_device, &request),
        Some(Request::SendTestNotification(_)) => {
            crate::notify::send_test(factory);
            Ok(())
        }
        None => return error(ErrorCode::UnknownMessage, "unknown message"),
    };
    match result {
        Ok(()) => Body::Ack(v1::Ack {}),
        Err(problem) => error(problem.code(), problem.to_string()),
    }
}

async fn read<S: Stream>(socket: &mut Socket<S>, frame: Option<Result<Message, tungstenite::Error>>) -> Result<Frame, ConnectionError> {
    let frame = match frame {
        Some(Ok(frame)) => frame,
        None => return Ok(Frame::Closed),
        // The client vanished without a close frame (app quit, laptop slept, network
        // dropped). That's a normal way for a client to leave.
        Some(Err(tungstenite::Error::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => return Ok(Frame::Closed),
        Some(Err(problem)) => return Err(problem.into()),
    };
    let payload = match frame {
        Message::Binary(payload) => payload,
        Message::Close(_) => return Ok(Frame::Closed),
        Message::Text(_) => {
            send(socket, 0, error(ErrorCode::MalformedMessage, "text frames are not part of the protocol")).await?;
            return Ok(Frame::Skip);
        }
        // tungstenite answers pings on its own.
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => return Ok(Frame::Skip),
    };
    match ClientMessage::decode(payload) {
        Ok(message) => Ok(Frame::Message(Box::new(message))),
        Err(decode_error) => {
            tracing::debug!(%decode_error, "undecodable frame");
            send(socket, 0, error(ErrorCode::MalformedMessage, "frame is not a ClientMessage")).await?;
            Ok(Frame::Skip)
        }
    }
}

/// Sends an event, a Workspace only as what this app doesn't have yet.
async fn forward<S: Stream>(socket: &mut Socket<S>, sent: &mut Workspaces, event: Event) -> Result<(), ConnectionError> {
    if let Some(event) = sent.outgoing(event) {
        socket.send(Message::Binary(event)).await?;
    }
    Ok(())
}

async fn send<S: Stream>(socket: &mut Socket<S>, reply_to: u64, body: Body) -> Result<(), ConnectionError> {
    let message = ServerMessage { reply_to, body: Some(body) };
    socket.send(Message::Binary(message.encode_to_vec().into())).await?;
    Ok(())
}

/// A device that used a pairing code first waits here, its Hello answered
/// with PairingPending, until the person allows it (`Allowed`), its code
/// ends (`Ended`, with why), or it goes away (`Waiting`).
async fn wait_to_be_allowed<S: Stream>(
    socket: &mut Socket<S>,
    factory: &Factory,
    hello_id: u64,
    waiting: &crate::remote::Device,
    peer: &crate::remote::Peer,
) -> Result<Pairing, ConnectionError> {
    let pending = v1::PairingPending {
        device_id: waiting.id.clone(),
        name: waiting.name.clone(),
        machine: crate::remote::machine_name().to_owned(),
        expires_ms: waiting.code_expires.unwrap_or_default(),
        discovered: waiting.discovered,
    };
    send(socket, hello_id, Body::PairingPending(pending)).await?;
    loop {
        match factory.pairing(waiting, &peer.endpoint_id) {
            Pairing::Waiting => {}
            settled => return Ok(settled),
        }
        tokio::select! {
            () = factory.pairing_changed() => {}
            frame = socket.next() => {
                // Nothing it sends counts until it's allowed.
                if let Frame::Closed = read(socket, frame).await? {
                    return Ok(Pairing::Waiting);
                }
            }
        }
    }
}

/// Sends a final error and closes the connection.
async fn reject<S: Stream>(
    mut socket: Socket<S>,
    reply_to: u64,
    code: ErrorCode,
    reason: impl Into<String>,
) -> Result<(), ConnectionError> {
    send(&mut socket, reply_to, error(code, reason)).await?;
    socket.close(None).await?;
    Ok(())
}

/// A request's answer, or why it failed.
fn answer<T>(result: Result<T, RequestError>, body: impl FnOnce(T) -> Body) -> Body {
    match result {
        Ok(value) => body(value),
        Err(problem) => error(problem.code(), problem.to_string()),
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> Body {
    Body::Error(v1::Error { code: code.into(), message: message.into(), ..Default::default() })
}
