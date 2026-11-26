import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart' show TextCapitalization;
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'decision.dart';
import 'extensions.dart' show logTime;
import 'forms.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show Metrics, TypeScale;
import 'mobile/input.dart' show InputKind;
import 'mobile/lines.dart';
import 'mobile/platform_policy.dart' show PhonePlatformPolicy;
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'secrets.dart' show SecretForm;
import 'selection.dart';
import 'usage.dart' show formatAge;
import 'web_auth.dart';

// Connecting services (docs/CONNECTORS.md): the person signs in to a service
// such as Cloudflare on this device, the backend keeps what it gets in the
// vault as a connection, and agents get only secrets made from it, never the
// connection itself. Nothing is given to anyone by connecting.

/// Opens a service's page in the browser. Tests replace it.
Future<bool> Function(Uri url) openConnectPage = launchUrl;

/// Whether this device catches the service's answer in the system's sign-in
/// sheet on the app's own scheme (a touch device: an iPhone, an iPad, an
/// Android phone or tablet, which use the device's system sign-in flow)
/// rather than on an address of its own (a Mac). Asked each time,
/// from the platform's policy, so tests that change the platform get its
/// answer.
bool get connectInSheet =>
    debugConnectInSheet ?? PhonePlatformPolicy.current.touch;

/// Sets [debugConnectInSheet], as tests do.
set connectInSheet(bool value) => debugConnectInSheet = value;

/// One way or the other in tests, whatever the platform; null (as tests
/// leave it when they're done) follows the platform's policy.
@visibleForTesting
bool? debugConnectInSheet;

/// The sign-in sheet, and closing it. Tests replace them.
Future<Uri?> Function(Uri url, String scheme) connectSheet = webAuth;
Future<void> Function() closeConnectSheet = cancelWebAuth;

/// Starts listening for the service's answer on this computer. Tests
/// replace it.
Future<ConnectReceiver> Function(String path) startConnectReceiver = (path) =>
    LoopbackReceiver.start(path: path);

/// The scheme the service sends this app back to in the sign-in sheet: its
/// bundle id.
const connectScheme = bool.fromEnvironment('FACTORY_DEV')
    ? 'com.claktory.app.dev'
    : 'com.claktory.app';

/// A service Claktory can connect, from the backend's catalog.
Connector? connectorOf(Backend backend, String id) => [
  for (final connector in backend.workspace.workspace.vault.connectors)
    if (connector.id == id) connector,
].firstOrNull;

/// A service's name, or its id when this app doesn't know it.
String serviceName(Backend backend, String id) =>
    connectorOf(backend, id)?.name ?? id;

/// One connecting of a service started on this device, kept while the app
/// runs (never on disk), so a card scrolled away or a sheet closed and
/// opened again still shows it: by what it's for ("new", a proposal's id or
/// the connection being reconnected).
class ConnectRun extends ChangeNotifier {
  ConnectRun._(this._backend);

  static final _runs = Expando<Map<String, ConnectRun>>();

  static ConnectRun of(Backend backend, String key) =>
      (_runs[backend] ??= {})[key] ??= ConnectRun._(backend);

  /// Every flow this device started, so others' show apart.
  static Set<String> startedHere(Backend backend) => {
    for (final run in (_runs[backend] ?? {}).values)
      if (run.started case final started?) started.flowId,
  };

  final Backend _backend;

  /// The service being connected.
  Connector? connector;

  /// Asking the backend to start; nothing to open yet.
  var starting = false;

  /// The flow, with the page to open; null before and after.
  ConnectStarted? started;

  /// Why it couldn't go on, before the backend had a flow to say so.
  String? error;

  ConnectReceiver? _receiver;
  var _generation = 0;

  /// PASTE: the token being pasted, cleared as soon as it's sent; never
  /// kept anywhere else on this device.
  final token = TextEditingController();

  /// PASTE: sending the token, until the backend has it.
  var sending = false;

  /// PASTE: why the token couldn't be sent, or that none was pasted.
  String? tokenError;

  /// The service makes a token on its page for the person to paste.
  bool get pastes =>
      connector?.flow == ConnectorFlowKind.CONNECTOR_FLOW_KIND_PASTE;

  /// PASTE: waiting for the person to paste a token.
  bool get waitingForToken =>
      pastes &&
      started != null &&
      error == null &&
      !sending &&
      state == ConnectFlowState.CONNECT_FLOW_STATE_WAITING;

  /// The flow as the backend tells every device.
  ConnectFlow? get flow {
    final id = started?.flowId;
    if (id == null) return null;
    return [
      for (final flow in _backend.workspace.workspace.vault.flows)
        if (flow.id == id) flow,
    ].firstOrNull;
  }

  ConnectFlowState get state =>
      flow?.state ?? ConnectFlowState.CONNECT_FLOW_STATE_WAITING;

  /// Started and not ended: connecting another would replace it.
  bool get busy =>
      starting ||
      (started != null &&
          switch (state) {
            ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED ||
            ConnectFlowState.CONNECT_FLOW_STATE_FAILED ||
            ConnectFlowState.CONNECT_FLOW_STATE_EXPIRED ||
            ConnectFlowState.CONNECT_FLOW_STATE_CANCELLED => false,
            _ => true,
          });

  /// Something to show: started, or ended with a word.
  bool get shown => starting || started != null || error != null;

  /// Connects [connector] with the access [accessId] ("" for its default)
  /// and its [fields], for [proposalId], or again in place of [secretId].
  Future<void> start(
    Connector connector, {
    String accessId = '',
    Map<String, String> fields = const {},
    String proposalId = '',
    String secretId = '',
    String comment = '',
  }) async {
    await _end();
    final generation = ++_generation;
    this.connector = connector;
    starting = true;
    error = null;
    sending = false;
    tokenError = null;
    token.clear();
    notifyListeners();
    final browser =
        connector.flow == ConnectorFlowKind.CONNECTOR_FLOW_KIND_BROWSER;
    final sheet = browser && connectInSheet;
    try {
      var redirect = '';
      if (sheet) {
        redirect = connector.redirectUris.firstWhere(
          (uri) => uri.startsWith('$connectScheme:'),
          orElse: () => throw RequestFailure(
            "${connector.name} can't return to this app. Connect it from "
            'Claktory on your Mac.',
          ),
        );
      } else if (browser) {
        final template = connector.redirectUris.firstWhere(
          (uri) => uri.startsWith('http://127.0.0.1'),
          orElse: () => throw RequestFailure(
            "${connector.name} can't return to this app. Connect it from "
            'Claktory on your Mac.',
          ),
        );
        final path = Uri.parse(template.replaceAll('{port}', '1')).path;
        final receiver = await startConnectReceiver(path);
        _receiver = receiver;
        redirect = template.replaceAll('{port}', '${receiver.address.port}');
      }
      final started = await _backend.startConnect(
        StartConnect(
          connectorId: connector.id,
          accessId: accessId,
          fields: fields.entries,
          proposalId: proposalId,
          redirectUri: redirect,
          secretId: secretId,
          comment: comment,
        ),
      );
      if (generation != _generation) {
        unawaited(_quietly(_backend.cancelConnect(started.flowId)));
        return;
      }
      this.started = started;
      starting = false;
      if (_receiver != null) _backend.workspace.addListener(_flowChanged);
      notifyListeners();
      final page = Uri.parse(started.url);
      if (connector.flow == ConnectorFlowKind.CONNECTOR_FLOW_KIND_PASTE) {
        // The token comes back by hand, in [sendToken].
        await openConnectPage(page);
      } else if (sheet) {
        final back = await connectSheet(page, connectScheme);
        if (generation != _generation) return;
        if (back == null) return await cancel();
        await _backend.finishConnect(started.flowId, back.toString());
      } else {
        await openConnectPage(page);
        final receiver = _receiver;
        if (receiver == null) return;
        final back = await receiver.back;
        if (back == null || generation != _generation) return;
        await _backend.finishConnect(started.flowId, back.toString());
      }
    } on RequestFailure catch (failure) {
      if (generation == _generation) _fail(failure.message);
    } on WebAuthFailure catch (failure) {
      if (generation == _generation) _fail(failure.message);
    } on Exception {
      if (generation == _generation) {
        _fail("Couldn't open ${connector.name}'s page on this device.");
      }
    }
  }

  void _fail(String message) {
    starting = false;
    error = message;
    _stopListening();
    notifyListeners();
  }

  /// Once the flow ended (failed, expired, or connected from the backend's
  /// side), nothing more can come back: this computer stops listening.
  void _flowChanged() {
    if (started != null && !busy) _stopListening();
  }

  void _stopListening() {
    _backend.workspace.removeListener(_flowChanged);
    final receiver = _receiver;
    _receiver = null;
    unawaited(receiver?.close());
  }

  /// PASTE: sends the pasted token to the backend, which checks it with
  /// the service; the field is emptied at once either way.
  Future<void> sendToken() async {
    final flowId = started?.flowId;
    final value = token.text.trim();
    token.clear();
    if (flowId == null || sending) return;
    if (value.isEmpty) {
      tokenError = 'Paste the token to connect.';
      notifyListeners();
      return;
    }
    final generation = _generation;
    sending = true;
    tokenError = null;
    notifyListeners();
    try {
      await _backend.finishConnect(flowId, '', token: value);
      // The flow moves on (checking the token) just after the answer:
      // the field stays away until it has.
      await _nextChange().timeout(const Duration(seconds: 2), onTimeout: () {});
    } on RequestFailure catch (failure) {
      if (generation == _generation) tokenError = failure.message;
    }
    if (generation != _generation) return;
    sending = false;
    notifyListeners();
  }

  Future<void> _nextChange() {
    final changed = Completer<void>();
    final workspace = _backend.workspace;
    void listener() {
      workspace.removeListener(listener);
      if (!changed.isCompleted) changed.complete();
    }

    workspace.addListener(listener);
    return changed.future.whenComplete(
      () => workspace.removeListener(listener),
    );
  }

  /// Opens the service's page again, for a browser tab closed too soon.
  Future<void> openAgain() async {
    final url = started?.url;
    if (url != null && (!connectInSheet || pastes)) {
      await openConnectPage(Uri.parse(url));
    }
  }

  /// Stops connecting, and forgets it.
  Future<void> cancel() async {
    final flowId = started?.flowId;
    await _end();
    _generation++;
    starting = false;
    started = null;
    error = null;
    sending = false;
    tokenError = null;
    token.clear();
    notifyListeners();
    if (flowId != null) await _quietly(_backend.cancelConnect(flowId));
  }

  /// Done with an ended one: forgets it here and takes it off the list.
  Future<void> clear() => cancel();

  Future<void> _end() async {
    _backend.workspace.removeListener(_flowChanged);
    final receiver = _receiver;
    _receiver = null;
    await receiver?.close();
    if (connectInSheet && started != null && busy) await closeConnectSheet();
  }

  static Future<void> _quietly(Future<void> request) async {
    try {
      await request;
    } on RequestFailure {
      // Ended already.
    }
  }
}

/// Where a connecting stands, in a line or two, with what the person can
/// do: open the page again or cancel while waiting, try again when it
/// failed. [onConnected] shows what's next once it's done.
class ConnectProgress extends StatelessWidget {
  const ConnectProgress({
    super.key,
    required this.run,
    required this.onRetry,
    this.cancellable = true,
  });

  final ConnectRun run;
  final VoidCallback onRetry;

  /// Shows Cancel while it runs, and Connect while waiting for a pasted
  /// token (a form has its own).
  final bool cancellable;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([run, backend.workspace]),
      builder: (context, _) => _build(context, backend),
    );
  }

  Widget _build(BuildContext context, Backend backend) {
    final palette = Palette.of(context);
    final name = run.connector?.name ?? 'the service';
    final started = run.started;
    final flow = run.flow;
    final device =
        run.connector?.flow == ConnectorFlowKind.CONNECTOR_FLOW_KIND_DEVICE;
    String line;
    String? detail;
    var tone = palette.textMuted;
    var live = false;
    Widget? input;
    final actions = <Widget>[];
    if (run.error != null) {
      line = run.error!;
      tone = palette.danger;
      actions.add(Button(label: 'Try again', onPressed: onRetry));
    } else if (run.starting || started == null) {
      line = 'Opening $name…';
      live = true;
    } else {
      switch (run.state) {
        case ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED:
          final secret = _secret(backend, flow?.secretId ?? '');
          line = secret == null
              ? 'Connected $name.'
              : 'Connected $name as ${secret.name}.';
          tone = palette.text;
          detail =
              'No agent has it yet. You or Rivet can give an agent a secret '
              'made from it, with only the access it needs.';
        case ConnectFlowState.CONNECT_FLOW_STATE_FAILED:
          line = (flow?.error.isNotEmpty ?? false)
              ? flow!.error
              : "Couldn't connect $name.";
          tone = palette.danger;
          actions.add(Button(label: 'Try again', onPressed: onRetry));
        case ConnectFlowState.CONNECT_FLOW_STATE_EXPIRED:
          line = 'This took too long and stopped. Try again.';
          tone = palette.danger;
          actions.add(Button(label: 'Try again', onPressed: onRetry));
        case ConnectFlowState.CONNECT_FLOW_STATE_CANCELLED:
          line = 'Cancelled.';
          actions.add(Button(label: 'Try again', onPressed: onRetry));
        case ConnectFlowState.CONNECT_FLOW_STATE_EXCHANGING:
          line = run.pastes ? 'Checking the token with $name…' : 'Finishing…';
          live = true;
        case ConnectFlowState.CONNECT_FLOW_STATE_CHECKING:
          line = 'Checking what $name allowed…';
          live = true;
        case _ when run.pastes:
          if (run.sending) {
            line = 'Checking the token with $name…';
            live = true;
            break;
          }
          // The setup note says what to choose on the service's page.
          final setup = run.connector?.setupNote ?? '';
          line = setup.isEmpty
              ? "Make a token on $name's page, then paste it here. Claktory "
                    'keeps it with your secrets and never shows it again.'
              : 'Paste the token here. Claktory keeps it with your secrets '
                    'and never shows it again.';
          if (setup.isNotEmpty) detail = setup;
          final why = run.tokenError ?? flow?.error ?? '';
          input = Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              TextField(
                controller: run.token,
                placeholder: 'Paste token',
                kind: InputKind.secret,
                autofocus: true,
                onSubmitted: (_) => run.sendToken(),
              ),
              if (why.isNotEmpty) ...[
                const SizedBox(height: Space.xs),
                Text(
                  why,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.danger),
                ),
              ],
            ],
          );
          if (cancellable) {
            actions.add(
              Button(label: 'Connect', primary: true, onPressed: run.sendToken),
            );
          }
          actions.add(
            Button(label: 'Open the page again', onPressed: run.openAgain),
          );
          if (cancellable) {
            actions.add(Button(label: 'Cancel', onPressed: run.cancel));
          }
        default:
          live = true;
          line = connectInSheet && !device
              ? 'Sign in to $name and allow access.'
              : 'Sign in to $name in your browser and allow access, then '
                    'come back here.';
          if (device && started.userCode.isNotEmpty) {
            detail = 'Check that the page shows ${started.userCode}.';
          }
          if (!connectInSheet || device) {
            actions.add(
              Button(label: 'Open the page again', onPressed: run.openAgain),
            );
          }
          if (cancellable) {
            actions.add(Button(label: 'Cancel', onPressed: run.cancel));
          }
      }
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (live) ...[
              const Padding(padding: EdgeInsets.only(top: 6), child: LiveDot()),
              const SizedBox(width: Space.s),
            ],
            Expanded(
              child: Text(
                line,
                style: TypeScale.of(context).body.copyWith(color: tone),
              ),
            ),
          ],
        ),
        if (detail != null) ...[
          const SizedBox(height: Space.xs),
          Text(
            detail,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textFaint),
          ),
        ],
        if (input != null) ...[const SizedBox(height: Space.s), input],
        if (actions.isNotEmpty) ...[
          const SizedBox(height: Space.s),
          Wrap(spacing: Space.s, runSpacing: Space.s, children: actions),
        ],
      ],
    );
  }
}

SecretInfo? _secret(Backend backend, String id) => id.isEmpty
    ? null
    : [
        for (final secret in backend.workspace.workspace.vault.secrets)
          if (secret.id == id) secret,
      ].firstOrNull;

/// What connecting a service asks for: the access to allow (when there's a
/// choice) and the fields it needs, such as AWS's portal address, with
/// what the person should have first.
class _ConnectFields {
  _ConnectFields(this.connector, {String accessId = ''})
    : access = _initialAccess(connector, accessId),
      values = {
        for (final field in connector.fields) field.id: TextEditingController(),
      };

  final Connector connector;
  String access;
  final Map<String, TextEditingController> values;

  static String _initialAccess(Connector connector, String wanted) {
    if (connector.accesses.any((access) => access.id == wanted)) return wanted;
    return [
          for (final access in connector.accesses)
            if (access.preselected) access.id,
        ].firstOrNull ??
        connector.accesses.firstOrNull?.id ??
        '';
  }

  void dispose() {
    for (final value in values.values) {
      value.dispose();
    }
  }

  Map<String, String> get filled => {
    for (final MapEntry(:key, :value) in values.entries)
      if (value.text.trim().isNotEmpty) key: value.text.trim(),
  };

  /// What's missing, in a sentence; null when it can go.
  String? get missing {
    for (final field in connector.fields) {
      if (!field.optional && values[field.id]!.text.trim().isEmpty) {
        return 'Enter the ${_sentenceCase(field.label)}.';
      }
    }
    return null;
  }

  List<Widget> build(
    BuildContext context, {
    required VoidCallback onChanged,
    required VoidCallback onSubmit,
  }) {
    final palette = Palette.of(context);
    final chosen = [
      for (final access in connector.accesses)
        if (access.id == this.access) access,
    ].firstOrNull;
    return [
      if (connector.setupNote.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(bottom: Space.l),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                connector.setupNote,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
              if (connector.setupUrl.isNotEmpty) ...[
                const SizedBox(height: Space.xs),
                Button(
                  label: 'Learn more',
                  onPressed: () =>
                      openConnectPage(Uri.parse(connector.setupUrl)),
                ),
              ],
            ],
          ),
        ),
      if (connector.accesses.length > 1)
        Field(
          label: 'Access',
          note: chosen?.why,
          child: Select<String>(
            value: access,
            options: [
              for (final access in connector.accesses)
                SelectOption(access.id, access.label),
            ],
            onChanged: (id) {
              access = id;
              onChanged();
            },
          ),
        )
      else if (chosen != null && chosen.why.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(bottom: Space.l),
          child: Text(
            chosen.why,
            style: TypeScale.of(context).body
                .copyWith(color: palette.textMuted),
          ),
        ),
      for (final field in connector.fields)
        Field(
          label: field.optional ? '${field.label} (optional)' : field.label,
          note: field.hint.isEmpty ? null : field.hint,
          child: TextField(
            controller: values[field.id]!,
            placeholder: field.placeholder,
            // A site's address, an account's id: as typed.
            autocorrect: false,
            textCapitalization: TextCapitalization.none,
            onSubmitted: (_) => onSubmit(),
          ),
        ),
    ];
  }
}

/// [label] inside a sentence: "Access portal URL" → "access portal URL",
/// an acronym such as "AWS region" kept.
String _sentenceCase(String label) {
  if (label.length < 2 || label[1].toUpperCase() == label[1]) return label;
  return label[0].toLowerCase() + label.substring(1);
}

/// Settings → Secrets → Connect a service: the services Claktory can
/// connect, then the chosen one's access and fields, then Connect, which
/// opens the service's page on this device, and how it's going.
class ConnectServiceForm extends StatefulWidget {
  const ConnectServiceForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<ConnectServiceForm> createState() => _ConnectServiceFormState();
}

class _ConnectServiceFormState extends State<ConnectServiceForm> {
  late final ConnectRun _run;
  _ConnectFields? _fields;
  String? _error;

  @override
  void initState() {
    super.initState();
    _run = ConnectRun.of(AppScope.of(context), 'new');
    // One that ended while the sheet was closed starts over.
    if (!_run.busy && _run.started != null) unawaited(_run.clear());
    if (!_run.busy && _run.error != null) unawaited(_run.clear());
    final connector = _run.connector;
    if (_run.busy && connector != null) _fields = _ConnectFields(connector);
  }

  @override
  void dispose() {
    _fields?.dispose();
    super.dispose();
  }

  void _pick(Connector connector) => setState(() {
    _fields?.dispose();
    _fields = _ConnectFields(connector);
    _error = null;
  });

  void _connect() {
    final fields = _fields;
    if (fields == null || _run.busy) return;
    final missing = fields.missing;
    setState(() => _error = missing);
    if (missing != null) return;
    unawaited(
      _run.start(
        fields.connector,
        accessId: fields.access,
        fields: fields.filled,
      ),
    );
  }

  Future<void> _cancel() async {
    if (_run.busy) await _run.cancel();
    widget.close();
  }

  Future<void> _done() async {
    await _run.clear();
    widget.close();
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([_run, backend.workspace]),
      builder: (context, _) {
        final fields = _fields;
        if (fields == null) return _choose(context, backend);
        final connector = fields.connector;
        final connected =
            _run.started != null &&
            _run.state == ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED;
        final showing = _run.shown && _run.connector?.id == connector.id;
        return FormFrame(
          title: 'Connect ${connector.name}',
          body: connector.description.isEmpty ? null : connector.description,
          fields: [
            if (showing) ...[
              ConnectProgress(
                run: _run,
                cancellable: false,
                onRetry: () async {
                  await _run.clear();
                  _connect();
                },
              ),
              const SizedBox(height: Space.l),
            ] else ...[
              ...fields.build(
                context,
                onChanged: () => setState(() {}),
                onSubmit: _connect,
              ),
              Padding(
                padding: const EdgeInsets.only(bottom: Space.l),
                child: Text(
                  'Connecting gives no agent access. Afterwards, you or Rivet '
                  'give an agent a secret made from it, with only what it '
                  'needs.',
                  style: TypeScale.of(context).caption
                      .copyWith(color: Palette.of(context).textFaint),
                ),
              ),
            ],
          ],
          error: _error,
          secondary:
              !showing &&
                  backend.workspace.workspace.vault.connectors.length > 1
              ? Button(
                  label: 'Back',
                  onPressed: () => setState(() {
                    _fields?.dispose();
                    _fields = null;
                    _error = null;
                  }),
                )
              : null,
          submitLabel: connected
              ? 'Done'
              : showing && !_run.waitingForToken
              ? 'Connecting…'
              : 'Connect',
          onSubmit: connected
              ? _done
              : _run.waitingForToken
              ? _run.sendToken
              : showing
              ? null
              : _connect,
          cancelLabel: connected ? 'Close' : 'Cancel',
          onCancel: connected ? _done : _cancel,
        );
      },
    );
  }

  Widget _choose(BuildContext context, Backend backend) {
    final palette = Palette.of(context);
    final connectors = backend.workspace.workspace.vault.connectors;
    return FormFrame(
      title: 'Connect a service',
      body:
          'Sign in to a service once. Claktory can then make secrets from '
          'it for your agents, with only the access each needs.',
      fields: [
        if (connectors.isEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.l),
            child: Text(
              'No services to connect yet.',
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
          ),
        for (final connector in connectors)
          Unselectable(
            child: Semantics(
              button: true,
              label: connector.name,
              child: Pressable(
                onTap: () => _pick(connector),
                builder: (context, hovered) => Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: Space.s,
                    vertical: Space.s,
                  ),
                  decoration: BoxDecoration(
                    color: hovered ? palette.hover : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Row(
                    children: [
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              connector.name,
                              style: TypeScale.of(context).label
                                  .copyWith(color: palette.text),
                            ),
                            if (connector.description.isNotEmpty)
                              Text(
                                connector.description,
                                style: TypeScale.of(context).caption
                                    .copyWith(color: palette.textMuted),
                              ),
                          ],
                        ),
                      ),
                      const SizedBox(width: Space.s),
                      Chevron(
                        color: hovered ? palette.textMuted : palette.textFaint,
                        down: false,
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        const SizedBox(height: Space.l),
      ],
      error: null,
      submitLabel: 'Continue',
      onSubmit: null,
      onCancel: widget.close,
    );
  }
}

/// An agent's proposal to connect a service (its
/// secrets_propose_connection), in its conversation and at the top of
/// Settings → Secrets: what for, the access it asks for and why; the
/// access and fields to connect with, Connect, and how it's going; or
/// Dismiss with a word why. Decided, what was connected or that it was
/// dismissed, and when. The agent gets nothing by connecting: Rivet (or the
/// person) gives it a secret made from the connection.
class ConnectProposalCard extends StatefulWidget {
  const ConnectProposalCard(this.proposal, {super.key});

  final ConnectProposal proposal;

  @override
  State<ConnectProposalCard> createState() => _ConnectProposalCardState();
}

class _ConnectProposalCardState extends State<ConnectProposalCard> {
  final _comment = TextEditingController();
  _ConnectFields? _fields;
  var _dismissing = false;
  String? _error;

  ConnectProposal get proposal => widget.proposal;

  @override
  void dispose() {
    _comment.dispose();
    _fields?.dispose();
    super.dispose();
  }

  _ConnectFields? _fieldsFor(Connector? connector) {
    if (connector == null) return null;
    final fields = _fields;
    if (fields != null && fields.connector.id == connector.id) return fields;
    fields?.dispose();
    return _fields = _ConnectFields(connector, accessId: proposal.accessId);
  }

  void _connect(ConnectRun run) {
    final fields = _fields;
    // A typed comment is a reply (#370): Submit, never Connect.
    if (fields == null || run.busy || commentTyped(_comment)) return;
    final missing = fields.missing;
    setState(() => _error = missing);
    if (missing != null) return;
    unawaited(
      run.start(
        fields.connector,
        accessId: fields.access,
        fields: fields.filled,
        proposalId: proposal.id,
        comment: _comment.text.trim(),
      ),
    );
  }

  /// [reply]: Submit (#370), the comment going to the agent as the
  /// person's reply, and nothing connected; it can ask again. A
  /// backend that doesn't know replies dismisses it with the comment.
  Future<void> _dismiss({bool reply = false}) async {
    if (_dismissing) return;
    setState(() {
      _dismissing = true;
      _error = null;
    });
    try {
      await AppScope.of(context).dismissConnectProposal(
        proposal.id,
        comment: _comment.text.trim(),
        reply: reply,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _dismissing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final run = ConnectRun.of(backend, 'proposal:${proposal.id}');
    return ListenableBuilder(
      listenable: Listenable.merge([run, backend.workspace]),
      builder: (context, _) => _card(context, backend, run),
    );
  }

  Widget _card(BuildContext context, Backend backend, ConnectRun run) {
    final palette = Palette.of(context);
    final agent = backend.workspace.agent(proposal.agentId);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? proposal.agentId;
    final connector = connectorOf(backend, proposal.connectorId);
    final service = connector?.name ?? proposal.connectorId;
    final pending =
        proposal.state == ConnectProposalState.CONNECT_PROPOSAL_STATE_PENDING;
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  '$name suggests connecting $service',
                  style: TypeScale.of(context).caption
                      .copyWith(color: color, fontWeight: FontWeight.w600),
                ),
              ),
            ],
          ),
          if (proposal.purpose.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Markdown(
              proposal.purpose,
              color: pending ? palette.text : palette.textMuted,
              shortLinks: true,
            ),
          ],
          if (proposal.access.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Text(
              'What it needs',
              style: TypeScale.of(context).caption.copyWith(
                color: palette.textFaint,
                fontWeight: FontWeight.w500,
              ),
            ),
            Markdown(
              proposal.access,
              color: palette.textMuted,
              shortLinks: true,
            ),
          ],
          if (proposal.region.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Text(
              'Region',
              style: TypeScale.of(context).caption.copyWith(
                color: palette.textFaint,
                fontWeight: FontWeight.w500,
              ),
            ),
            Text(
              proposal.region,
              style: TypeScale.of(context).code
                  .copyWith(color: palette.textMuted),
            ),
          ],
          if (proposal.statements.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            PolicyStatements(proposal.statements),
          ],
          if (proposal.reason.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Markdown(
              proposal.reason,
              color: palette.textMuted,
              shortLinks: true,
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending)
            ..._pending(context, palette, run, connector, name, service)
          else
            ..._decided(palette, color, service),
        ],
      ),
    );
  }

  List<Widget> _pending(
    BuildContext context,
    Palette palette,
    ConnectRun run,
    Connector? connector,
    String agent,
    String service,
  ) {
    final phone = isPhoneLayout(context);
    if (connector == null) {
      return [
        Text(
          "Claktory can't connect $service yet.",
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.m),
        _dismissRow(agent),
      ];
    }
    final fields = _fieldsFor(connector)!;
    if (run.shown) {
      return [
        ConnectProgress(
          run: run,
          onRetry: () async {
            await run.clear();
            _connect(run);
          },
        ),
      ];
    }
    return [
      ...fields.build(
        context,
        onChanged: () => setState(() {}),
        onSubmit: () => _connect(run),
      ),
      // Once a comment is typed, Submit (below) replies instead: this
      // says what that does.
      CommentTyped(
        controller: _comment,
        builder: (context, typed) {
          final line = Text(
            _error ??
                (typed
                    ? 'Nothing is connected. $agent gets your comment and '
                          'can ask again.'
                    : 'Connecting gives $agent nothing yet. You or Rivet '
                          'then give it only the access it needs.'),
            style: TypeScale.of(context).caption.copyWith(
              color: _error == null ? palette.textFaint : palette.danger,
            ),
          );
          if (typed) return line;
          return ActionLine(
            lead: line,
            action: Button(
              label: phone ? 'Connect' : 'Connect $service',
              primary: true,
              onPressed: _dismissing ? null : () => _connect(run),
            ),
          );
        },
      ),
      const SizedBox(height: Space.m),
      _dismissRow(agent),
    ];
  }

  /// A comment for the agent, and Dismiss; once something is typed, Submit
  /// in its place (Return too), the comment going as a reply (#370).
  Widget _dismissRow(String agent) => CommentTyped(
    controller: _comment,
    builder: (context, typed) => ActionLine(
      lead: DecisionComment(
        controller: _comment,
        agent: agent,
        enabled: !_dismissing,
        onSubmit: () => _dismiss(reply: true),
      ),
      action: typed
          ? SubmitButton(
              onPressed: _dismissing ? null : () => _dismiss(reply: true),
            )
          : Button(label: 'Dismiss', onPressed: _dismissing ? null : _dismiss),
    ),
  );

  List<Widget> _decided(Palette palette, Color color, String service) {
    final dismissed =
        proposal.state == ConnectProposalState.CONNECT_PROPOSAL_STATE_DISMISSED;
    final decided = proposal.decidedMs.toInt();
    return [
      // Who decided: the person.
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (true) ...[
            const PersonAvatar(size: 16),
            const SizedBox(width: Space.xs + 2),
          ],
          Flexible(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: dismissed && proposal.replied
                        // Answered with a comment (#370).
                        ? 'Replied'
                        : dismissed
                        ? 'Dismissed'
                        : proposal.secretName.isEmpty
                        ? 'Connected $service'
                        : 'Connected $service as ${proposal.secretName}',
                    style: TextStyle(
                      color: dismissed ? palette.textMuted : color,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text:
                          ' · ${logTime(DateTime.fromMillisecondsSinceEpoch(decided))}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: TypeScale.of(context).caption,
            ),
          ),
        ],
      ),
      if (proposal.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          '“${proposal.comment}”',
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
      ],
    ];
  }
}

/// Connecting going on from other devices, as every device sees it: one
/// line each, at the top of Settings → Secrets.
class OtherConnectFlows extends StatelessWidget {
  const OtherConnectFlows({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final palette = Palette.of(context);
    final here = ConnectRun.startedHere(backend);
    final flows = [
      for (final flow in backend.workspace.workspace.vault.flows)
        if (!here.contains(flow.id) &&
            switch (flow.state) {
              ConnectFlowState.CONNECT_FLOW_STATE_WAITING ||
              ConnectFlowState.CONNECT_FLOW_STATE_EXCHANGING ||
              ConnectFlowState.CONNECT_FLOW_STATE_CHECKING => true,
              _ => false,
            })
          flow,
    ];
    if (flows.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (final flow in flows)
            Row(
              children: [
                const LiveDot(),
                const SizedBox(width: Space.s),
                Flexible(
                  child: Text(
                    flow.deviceName.isEmpty
                        ? 'Connecting ${serviceName(backend, flow.connectorId)}…'
                        : 'Connecting '
                              '${serviceName(backend, flow.connectorId)} on '
                              '${flow.deviceName}…',
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
                ),
              ],
            ),
        ],
      ),
    );
  }
}

/// "June", "June and Sam", "June, Sam and Lena".
String _and(List<String> names) => names.length < 2
    ? names.join()
    : '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}';

/// A connection's line in the secrets list, under its name: the service
/// and whose account, and whether it needs connecting again.
class ConnectionHeadline extends StatelessWidget {
  const ConnectionHeadline({super.key, required this.secret});

  final SecretInfo secret;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final connection = secret.connection;
    final service = serviceName(backend, connection.connectorId);
    final needs =
        connection.status == ConnectionStatus.CONNECTION_STATUS_NEEDS_RECONNECT;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          [
            '$service connection',
            if (connection.account.isNotEmpty) connection.account,
          ].join(' · '),
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textMuted),
        ),
        if (needs)
          Text(
            connection.problem.isEmpty
                ? 'Connect again to keep using it.'
                : connection.problem,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.danger),
          ),
      ],
    );
  }
}

/// Who can use a connection: never anyone directly; the agents given
/// secrets made from it, if any.
String connectionSummary(Backend backend, SecretInfo connection) {
  final made = [
    for (final secret in backend.workspace.workspace.vault.secrets)
      if (secret.hasExport() && secret.export.connectionId == connection.id)
        secret,
  ];
  final agents = {
    for (final secret in made)
      for (final grant in secret.grants)
        backend.workspace.agent(grant.agentId)?.name ?? grant.agentId,
  };
  return [
    if (agents.isEmpty)
      'No agent has access'
    else
      '${_and(agents.toList())} ${agents.length == 1 ? 'uses' : 'use'} '
          'secrets made from it',
    if (made.isNotEmpty)
      made.length == 1
          ? '1 secret made from it'
          : '${made.length} made from it',
  ].join(' · ');
}

/// An export's line in the secrets list: the connection it was made from,
/// and how its key is renewed.
class ExportHeadline extends StatelessWidget {
  const ExportHeadline({super.key, required this.secret});

  final SecretInfo secret;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final export = secret.export;
    final from = _secret(backend, export.connectionId);
    return Text(
      [
        'Made from ${from?.name ?? 'a removed connection'}',
        if (export.perUse)
          'A new key for each command'
        else if (export.expiresMs > 0)
          'Renewed before it expires',
      ].join(' · '),
      style: TypeScale.of(context).caption.copyWith(color: palette.textMuted),
    );
  }
}

/// What a connection or an export can do and reach, as lists of sentences.
class AccessLines extends StatelessWidget {
  const AccessLines({
    super.key,
    required this.access,
    required this.resources,
    this.expiresMs = 0,
    this.checkedMs = 0,
  });

  final List<String> access;
  final List<String> resources;
  final int expiresMs;
  final int checkedMs;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final now = DateTime.now();
    Widget heading(String text) => Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Text(
        text,
        style: TypeScale.of(context).caption
            .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500),
      ),
    );
    Widget line(String text) => Text(
      text,
      style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
    );
    final dates = [
      if (expiresMs > 0)
        'Valid until ${logTime(DateTime.fromMillisecondsSinceEpoch(expiresMs))}',
      if (checkedMs > 0)
        '${expiresMs > 0 ? 'checked' : 'Checked'} ${formatAge(checkedMs, now)}',
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (access.isNotEmpty) ...[
          heading('Can'),
          for (final sentence in access) line(sentence),
        ],
        if (resources.isNotEmpty) ...[
          heading('Reaches'),
          for (final resource in resources) line(resource),
        ],
        if (dates.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text(
              dates.join(' · '),
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
          ),
      ],
    );
  }
}

/// An open connection in Settings → Secrets: what it can do and reach, the
/// secrets made from it, and Make a secret from it, Connect again and
/// Disconnect. It has no values to show and can't be given to an agent.
class ConnectionDetails extends StatefulWidget {
  const ConnectionDetails({super.key, required this.secret});

  final SecretInfo secret;

  @override
  State<ConnectionDetails> createState() => _ConnectionDetailsState();
}

class _ConnectionDetailsState extends State<ConnectionDetails> {
  SecretInfo get secret => widget.secret;

  Future<void> _reconnect(ConnectRun run) async {
    final connector = connectorOf(
      AppScope.of(context),
      secret.connection.connectorId,
    );
    if (connector == null) return;
    if (connector.fields.isNotEmpty) {
      // Its portal and region are asked again, the way it was connected.
      return showModal(
        context,
        (context, close) => _ReconnectForm(
          secret: secret,
          connector: connector,
          run: run,
          close: close,
        ),
      );
    }
    await run.clear();
    await run.start(connector, secretId: secret.id);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final connection = secret.connection;
    final run = ConnectRun.of(backend, 'secret:${secret.id}');
    final made = [
      for (final other in backend.workspace.workspace.vault.secrets)
        if (other.hasExport() && other.export.connectionId == secret.id) other,
    ];
    return ListenableBuilder(
      listenable: run,
      builder: (context, _) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          AccessLines(
            access: connection.access,
            resources: connection.resources,
            expiresMs: connection.expiresMs.toInt(),
            checkedMs: connection.checkedMs.toInt(),
          ),
          const SizedBox(height: Space.s),
          Text(
            made.isEmpty
                ? 'No agent can use it. Make a secret from it with only the '
                      'access an agent needs, then give that secret.'
                : 'Agents use it only through secrets made from it: '
                      '${made.map((secret) => secret.name).join(', ')}.',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textFaint),
          ),
          if (run.shown) ...[
            const SizedBox(height: Space.m),
            ConnectProgress(run: run, onRetry: () => _reconnect(run)),
          ],
          const SizedBox(height: Space.m),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(
                label: 'Make a secret from it…',
                onPressed: () => showModal(
                  context,
                  (context, close) =>
                      DeriveSecretForm(connection: secret, close: close),
                ),
              ),
              Button(
                label: 'Connect again',
                onPressed: run.busy ? null : () => _reconnect(run),
              ),
              Button(
                label: 'Edit…',
                onPressed: () => showModal(
                  context,
                  (context, close) => SecretForm(secret: secret, close: close),
                ),
              ),
              Button(
                label: 'Disconnect…',
                onPressed: () => showModal(
                  context,
                  (context, close) =>
                      _DisconnectForm(secret: secret, close: close),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Connect again, for a service that asks for fields (AWS's portal and
/// region): the fields, then Connect, as when it was first connected.
class _ReconnectForm extends StatefulWidget {
  const _ReconnectForm({
    required this.secret,
    required this.connector,
    required this.run,
    required this.close,
  });

  final SecretInfo secret;
  final Connector connector;
  final ConnectRun run;
  final VoidCallback close;

  @override
  State<_ReconnectForm> createState() => _ReconnectFormState();
}

class _ReconnectFormState extends State<_ReconnectForm> {
  late final _fields = _ConnectFields(widget.connector);
  String? _error;

  @override
  void dispose() {
    _fields.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final missing = _fields.missing;
    setState(() => _error = missing);
    if (missing != null) return;
    widget.close();
    await widget.run.clear();
    await widget.run.start(
      widget.connector,
      fields: _fields.filled,
      secretId: widget.secret.id,
    );
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Connect ${widget.secret.name} again',
    body:
        'Sign in to the same ${widget.connector.name} account. The secrets '
        'made from it, and who has them, stay as they are.',
    fields: _fields.build(
      context,
      onChanged: () => setState(() {}),
      onSubmit: _submit,
    ),
    error: _error,
    submitLabel: 'Connect',
    onSubmit: _submit,
    onCancel: widget.close,
  );
}

/// "Disconnect Cloudflare?": removes the connection and the secrets made
/// from it, and, if the person keeps it on, revokes it at the service.
class _DisconnectForm extends StatefulWidget {
  const _DisconnectForm({required this.secret, required this.close});

  final SecretInfo secret;
  final VoidCallback close;

  @override
  State<_DisconnectForm> createState() => _DisconnectFormState();
}

class _DisconnectFormState extends State<_DisconnectForm> {
  var _revoke = true;
  var _busy = false;
  String? _error;

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .disconnectService(widget.secret.id, revoke: _revoke);
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final service = serviceName(
      AppScope.of(context),
      widget.secret.connection.connectorId,
    );
    return FormFrame(
      title: 'Disconnect ${widget.secret.name}?',
      body:
          'The secrets made from it are deleted too, and agents that have '
          "them can't use them anymore.",
      fields: [
        ToggleTap(
          value: _revoke,
          onChanged: (value) => setState(() => _revoke = value),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Toggle(
                  value: _revoke,
                  label: 'Also cancel it at $service',
                  onChanged: (value) => setState(() => _revoke = value),
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Also cancel it at $service',
                      style: TypeScale.of(context).body
                          .copyWith(color: palette.text),
                    ),
                    Text(
                      'So keys made from it stop working everywhere, even '
                      'where they were copied.',
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textMuted),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: _busy ? 'Disconnecting…' : 'Disconnect',
      onSubmit: _busy ? null : _submit,
      onCancel: widget.close,
    );
  }
}

/// Make a secret from a connection: its name and description, then what it
/// reaches (an account, its zones or a role) and may do (permissions),
/// chosen from what the connection offers. It's given to no one; the
/// person gives it like any secret.
class DeriveSecretForm extends StatefulWidget {
  const DeriveSecretForm({
    super.key,
    required this.connection,
    required this.close,
  });

  final SecretInfo connection;
  final VoidCallback close;

  @override
  State<DeriveSecretForm> createState() => _DeriveSecretFormState();
}

class _DeriveSecretFormState extends State<DeriveSecretForm> {
  final _name = TextEditingController();
  final _description = TextEditingController();
  List<ConnectChoice>? _choices;

  /// How to pick, in the service's terms.
  var _rules = '';
  String _account = '';
  String _role = '';
  final _zones = <String>{};
  final _permissions = <String>{};
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _name.text = _suggested = widget.connection.name;
    unawaited(_load());
  }

  /// The name last suggested; while the field still holds it, it follows
  /// what's chosen ("AWS DeploySite in prod", "Cloudflare claktory.com").
  var _suggested = '';

  @override
  void setState(VoidCallback fn) {
    super.setState(fn);
    final choices = _choices;
    if (choices == null || _name.text != _suggested) return;
    String label(String id) =>
        [
          for (final choice in choices)
            if (choice.id == id) choice.label,
        ].firstOrNull ??
        '';
    final service = serviceName(
      AppScope.of(context),
      widget.connection.connection.connectorId,
    );
    final zones = [
      for (final zone in _of(choices, 'zone', parent: _parent(choices)))
        if (_zones.contains(zone.id)) zone.label,
    ];
    final account = label(_account).replaceAll(RegExp(r'\s*\(\d+\)$'), '');
    final String suggestion;
    if (_role.isNotEmpty) {
      suggestion = account.isEmpty
          ? '$service ${label(_role)}'
          : '$service ${label(_role)} in $account';
    } else if (zones.isNotEmpty) {
      suggestion = zones.length == 1
          ? '$service ${zones.single}'
          : '$service ${zones.first} and ${zones.length - 1} more';
    } else if (account.isNotEmpty) {
      suggestion = '$service $account';
    } else {
      return;
    }
    _name.text = _suggested = suggestion;
  }

  @override
  void dispose() {
    _name.dispose();
    _description.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final answer = await AppScope.of(context)
          .listConnectChoices(widget.connection.id);
      if (!mounted) return;
      final choices = answer.choices;
      setState(() {
        _choices = choices;
        _rules = answer.rules;
        final accounts = _of(choices, 'account');
        if (accounts.length == 1) _account = accounts.first.id;
      });
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _choices = const [];
          _error = failure.message;
        });
      }
    }
  }

  /// What zones and roles belong to: the chosen account, or anything
  /// when there are no accounts to choose.
  String? _parent(List<ConnectChoice> choices) =>
      _of(choices, 'account').isEmpty ? null : _account;

  static List<ConnectChoice> _of(
    List<ConnectChoice> choices,
    String kind, {
    String? parent,
  }) => [
    for (final choice in choices)
      if (choice.kind == kind && (parent == null || choice.parent == parent))
        choice,
  ];

  Future<void> _submit() async {
    final choices = _choices;
    if (_busy || choices == null) return;
    String? missing;
    if (_name.text.trim().isEmpty) {
      missing = 'Enter a name for the secret.';
    } else if (_of(choices, 'account').isNotEmpty && _account.isEmpty) {
      missing = 'Choose an account.';
    } else if (_of(choices, 'role', parent: _parent(choices)).isNotEmpty &&
        _role.isEmpty) {
      missing = 'Choose a role.';
    } else if (_of(choices, 'permission').isNotEmpty && _permissions.isEmpty) {
      missing = 'Choose what it may do.';
    }
    if (missing != null) {
      setState(() => _error = missing);
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    final zones = {
      for (final zone in _of(choices, 'zone', parent: _parent(choices)))
        zone.id,
    };
    try {
      await AppScope.of(context).deriveSecret(
        DeriveSecret(
          secretId: widget.connection.id,
          name: _name.text.trim(),
          description: _description.text.trim(),
          permissions: _permissions,
          // A role stands for its account; otherwise the account and any
          // of its zones.
          resources: _role.isNotEmpty
              ? [_role]
              : [
                  if (_account.isNotEmpty) _account,
                  ..._zones.where(zones.contains),
                ],
        ),
      );
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final choices = _choices;
    final accounts = choices == null
        ? <ConnectChoice>[]
        : _of(choices, 'account');
    final zones = choices == null
        ? <ConnectChoice>[]
        : _of(choices, 'zone', parent: _parent(choices));
    final roles = choices == null
        ? <ConnectChoice>[]
        : _of(choices, 'role', parent: _parent(choices));
    final permissions = choices == null
        ? <ConnectChoice>[]
        : _of(choices, 'permission');
    return FormFrame(
      title: 'Make a secret from ${widget.connection.name}',
      body:
          'Give it only the access an agent needs. No agent has it until '
          'you give it.',
      fields: [
        Field(
          label: 'Name',
          child: TextField(
            controller: _name,
            autofocus: true,
            kind: InputKind.text,
          ),
        ),
        Field(
          label: 'Description',
          note: 'What it’s for. Agents and the Map show it.',
          child: TextField(
            controller: _description,
            kind: InputKind.multiline,
            maxLines: 3,
          ),
        ),
        if (choices == null)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.l),
            child: Text(
              'Asking ${serviceName(AppScope.of(context), widget.connection.connection.connectorId)} what it can reach…',
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
          )
        else ...[
          if (_rules.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.l),
              child: Text(
                _rules,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
            ),
          if (accounts.isNotEmpty)
            Field(
              label: 'Account',
              child: Select<String>(
                value: _account,
                options: [
                  if (_account.isEmpty)
                    const SelectOption('', 'Choose an account'),
                  for (final account in accounts)
                    SelectOption(account.id, account.label),
                ],
                onChanged: (id) => setState(() {
                  _account = id;
                  _role = '';
                  _error = null;
                }),
              ),
            ),
          if (roles.isNotEmpty)
            Field(
              label: 'Role',
              note: [
                for (final role in roles)
                  if (role.id == _role && role.why.isNotEmpty) role.why,
              ].firstOrNull,
              child: Select<String>(
                value: _role,
                options: [
                  if (_role.isEmpty) const SelectOption('', 'Choose a role'),
                  for (final role in roles) SelectOption(role.id, role.label),
                ],
                onChanged: (id) => setState(() {
                  _role = id;
                  _error = null;
                }),
              ),
            ),
          if (zones.isNotEmpty)
            _Checks(
              label: 'Zones',
              note: 'None chosen: every zone in the account.',
              choices: zones,
              chosen: _zones,
              onChanged: () => setState(() => _error = null),
            ),
          if (permissions.isNotEmpty)
            _Checks(
              label: 'What it may do',
              choices: permissions,
              chosen: _permissions,
              onChanged: () => setState(() => _error = null),
            ),
        ],
      ],
      error: _error,
      submitLabel: _busy ? 'Making…' : 'Make secret',
      onSubmit: _busy || choices == null ? null : _submit,
      onCancel: widget.close,
    );
  }
}

/// A list of choices to check, each with what it allows under it.
class _Checks extends StatelessWidget {
  const _Checks({
    required this.label,
    required this.choices,
    required this.chosen,
    required this.onChanged,
    this.note,
  });

  final String label;
  final String? note;
  final List<ConnectChoice> choices;
  final Set<String> chosen;
  final VoidCallback onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Field(
      label: label,
      note: note,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final choice in choices)
            Unselectable(
              child: Semantics(
                checked: chosen.contains(choice.id),
                label: choice.label,
                child: Pressable(
                  onTap: () {
                    if (!chosen.remove(choice.id)) chosen.add(choice.id);
                    onChanged();
                  },
                  builder: (context, hovered) => Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: Space.xs,
                      vertical: Space.xs,
                    ),
                    decoration: BoxDecoration(
                      color: hovered ? palette.hover : null,
                      borderRadius: BorderRadius.circular(Radii.control),
                    ),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Padding(
                          padding: const EdgeInsets.only(top: 5),
                          child: CheckMark(on: chosen.contains(choice.id)),
                        ),
                        const SizedBox(width: Space.s),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(
                                choice.label,
                                style: TypeScale.of(context).body
                                    .copyWith(color: palette.text),
                              ),
                              if (choice.why.isNotEmpty)
                                Text(
                                  choice.why,
                                  style: TypeScale.of(context).caption
                                      .copyWith(color: palette.textMuted),
                                ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// The exact permissions a proposal asks for (AWS's policy statements, all
/// Allow), folded under one line until opened: each statement in a panel,
/// its actions, what they're on, and when, in words where the condition's
/// operators are the usual ones and as its JSON where they aren't.
class PolicyStatements extends StatefulWidget {
  const PolicyStatements(this.statements, {super.key});

  final List<PolicyStatement> statements;

  @override
  State<PolicyStatements> createState() => _PolicyStatementsState();
}

class _PolicyStatementsState extends State<PolicyStatements> {
  var _open = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final count = widget.statements.length;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Align(
          alignment: Alignment.centerLeft,
          // A caption link, underlined at rest so it reads as one before
          // the pointer finds it; a finger's height on a touch screen.
          child: Semantics(
            button: true,
            expanded: _open,
            child: Pressable(
              onTap: () => setState(() => _open = !_open),
              builder: (context, hovered) {
                final color = hovered ? palette.text : palette.textMuted;
                return ConstrainedBox(
                  constraints: BoxConstraints(
                    minHeight: touchFirst(context)
                        ? Metrics.of(context).target
                        : 0,
                  ),
                  child: Align(
                    alignment: Alignment.centerLeft,
                    widthFactor: 1,
                    child: Text(
                      _open
                          ? 'Hide exact permissions'
                          : count == 1
                          ? 'Show exact permissions (1 rule)'
                          : 'Show exact permissions ($count rules)',
                      style: TypeScale.of(context).caption.copyWith(
                        color: color,
                        fontWeight: FontWeight.w500,
                        decoration: TextDecoration.underline,
                        decorationColor: color,
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
        ),
        if (_open)
          for (final statement in widget.statements) ...[
            const SizedBox(height: Space.xs),
            _Statement(statement),
          ],
      ],
    );
  }
}

class _Statement extends StatelessWidget {
  const _Statement(this.statement);

  final PolicyStatement statement;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Widget label(String text) => Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Text(
        text,
        style: TypeScale.of(context).caption
            .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500),
      ),
    );
    Widget lines(Iterable<String> lines, Color color) => Text(
      lines.join('\n'),
      style: TypeScale.of(context).code.copyWith(fontSize: 12, color: color),
    );
    final condition = conditionLines(statement.condition);
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s,
      ),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.control),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Allows',
            style: TypeScale.of(context).caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w500,
            ),
          ),
          lines(statement.actions, palette.text),
          if (statement.resources.isNotEmpty) ...[
            label('On'),
            lines(
              statement.resources.map(
                (r) => r == '*' ? '* (all resources)' : r,
              ),
              palette.textMuted,
            ),
          ],
          if (condition != null) ...[
            label('Only when'),
            lines(condition, palette.textMuted),
          ],
        ],
      ),
    );
  }
}

/// An IAM Condition block (compact JSON) as lines to read: one per key,
/// "aws:RequestedRegion is eu-central-1", in words for the usual operators,
/// the JSON itself, indented, for anything else; null when there's none.
@visibleForTesting
List<String>? conditionLines(String condition) {
  if (condition.trim().isEmpty) return null;
  Object? parsed;
  try {
    parsed = jsonDecode(condition);
  } on FormatException {
    return [condition];
  }
  String raw() => const JsonEncoder.withIndent('  ').convert(parsed);
  if (parsed is! Map<String, dynamic> || parsed.isEmpty) return [raw()];
  final out = <String>[];
  for (final MapEntry(key: operator, value: keys) in parsed.entries) {
    var name = operator;
    final ifExists = name.endsWith('IfExists');
    if (ifExists) name = name.substring(0, name.length - 'IfExists'.length);
    final words = _conditionWords[name];
    if (words == null || keys is! Map<String, dynamic> || keys.isEmpty) {
      return [raw()];
    }
    for (final MapEntry(key: key, value: value) in keys.entries) {
      final values = [
        for (final v in value is List ? value : [value]) '$v',
      ];
      if (values.isEmpty) return [raw()];
      final list = values.length == 1
          ? values.single
          : '${values.sublist(0, values.length - 1).join(', ')} or '
                '${values.last}';
      out.add('$key $words $list${ifExists ? ' (if the request has it)' : ''}');
    }
  }
  return out;
}

const _conditionWords = {
  'StringEquals': 'is',
  'StringEqualsIgnoreCase': 'is (any case)',
  'StringNotEquals': "isn't",
  'StringLike': 'matches',
  'StringNotLike': "doesn't match",
  'ArnEquals': 'is',
  'ArnLike': 'matches',
  'ArnNotEquals': "isn't",
  'ArnNotLike': "doesn't match",
  'NumericEquals': 'is',
  'NumericNotEquals': "isn't",
  'NumericLessThan': 'is less than',
  'NumericLessThanEquals': 'is at most',
  'NumericGreaterThan': 'is more than',
  'NumericGreaterThanEquals': 'is at least',
  'DateLessThan': 'is before',
  'DateLessThanEquals': 'is at or before',
  'DateGreaterThan': 'is after',
  'DateGreaterThanEquals': 'is at or after',
  'Bool': 'is',
  'IpAddress': 'is in',
  'NotIpAddress': "isn't in",
};
