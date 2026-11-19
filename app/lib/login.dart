import 'dart:async';
import 'dart:developer' as developer;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'tell_rivet.dart';

/// Opens a sign-in page in the browser for flows this window started.
///
/// Every window shows the same `Workspace.login`, but only the one where the
/// user clicked "Sign in" opens the browser, once, when the provider publishes
/// the address. That may be after StartLogin's ack, since the provider's flow
/// runs on its own.
class LoginLauncher {
  LoginLauncher({
    required this.workspace,
    required this._send,
    Future<bool> Function(Uri url)? open,
  }) : _open = open ?? launchUrl {
    workspace.addListener(_check);
  }

  final WorkspaceStore workspace;
  final Future<void> Function(StartLogin start) _send;
  final Future<bool> Function(Uri url) _open;

  /// The flow this window started and hasn't opened yet.
  StartLogin? _waiting;

  /// An address that was showing when [_waiting] started; it belongs to the
  /// flow being replaced, so it's never opened for this one.
  var _stale = '';
  final _opened = <String>{};

  /// Starts signing in and opens the browser when the address arrives.
  Future<void> start(String providerId, String methodId) async {
    final start = StartLogin(providerId: providerId, methodId: methodId);
    final current = workspace.workspace;
    _stale = current.hasLogin() ? current.login.url : '';
    _waiting = start;
    try {
      await _send(start);
    } on RequestFailure {
      if (_waiting == start) _waiting = null;
      rethrow;
    }
    _check();
  }

  /// The flow was cancelled or dismissed here; don't open anything for it.
  void forget() => _waiting = null;

  /// Whether this window opened [url] already.
  bool opened(String url) => _opened.contains(url);

  /// Opens [url] in the browser, now.
  Future<void> open(String url) async {
    final uri = Uri.tryParse(url);
    if (uri == null) return;
    _opened.add(url);
    try {
      if (!await _open(uri)) _log('the browser did not open $url');
    } on PlatformException catch (error) {
      _log('could not open $url: ${error.message}');
    }
  }

  void _check() {
    final waiting = _waiting;
    final workspace = this.workspace.workspace;
    if (waiting == null || !workspace.hasLogin()) return;
    final login = workspace.login;
    if (login.providerId != waiting.providerId ||
        login.methodId != waiting.methodId) {
      return;
    }
    if (login.error.isNotEmpty) {
      _waiting = null;
      return;
    }
    if (login.url.isEmpty || login.url == _stale) return;
    _waiting = null;
    unawaited(open(login.url));
  }

  void dispose() => workspace.removeListener(_check);
}

void _log(String message) => developer.log(message, name: 'factory.login');

/// The sign-in in progress for one provider: what to open, codes to enter,
/// questions to answer, progress and errors. Driven entirely by
/// `Workspace.login`, so every window shows the same flow.
class LoginPanel extends StatefulWidget {
  const LoginPanel({super.key, required this.flow, required this.method});

  final LoginFlow flow;

  /// The label of the method being used, e.g. "Anthropic (Claude Pro/Max)".
  final String method;

  @override
  State<LoginPanel> createState() => _LoginPanelState();
}

class _LoginPanelState extends State<LoginPanel> {
  String? _failure;
  var _copied = '';
  Timer? _copiedTimer;

  @override
  void dispose() {
    _copiedTimer?.cancel();
    super.dispose();
  }

  Future<void> _run(Future<void> Function() request) async {
    setState(() => _failure = null);
    try {
      await request();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _failure = failure.message);
    }
  }

  void _cancel() {
    final scope = AppScope.scope(context);
    scope.login.forget();
    _run(scope.backend.cancelLogin);
  }

  Future<void> _copy(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (!mounted) return;
    _copiedTimer?.cancel();
    setState(() => _copied = text);
    _copiedTimer = Timer(const Duration(milliseconds: 1500), () {
      if (mounted) setState(() => _copied = '');
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final flow = widget.flow;
    final failed = flow.error.isNotEmpty;
    final children = <Widget>[
      Row(
        children: [
          if (!failed) ...[const LiveDot(), const SizedBox(width: Space.s)],
          Expanded(
            child: Text(
              failed
                  ? "Couldn't sign in with ${widget.method}"
                  : 'Signing in with ${widget.method}',
              style: TextStyles.label.copyWith(color: palette.text),
            ),
          ),
        ],
      ),
    ];

    void add(Widget child) {
      children
        ..add(const SizedBox(height: Space.m))
        ..add(child);
    }

    if (failed) {
      add(
        ErrorLine(
          flow.error,
          about:
              "Signing in to ${flow.providerId} with ${widget.method} didn't work.",
          style: TextStyles.body.copyWith(color: palette.danger),
        ),
      );
      add(
        Row(
          children: [
            Button(
              label: 'Try again',
              primary: true,
              onPressed: () =>
                  _run(() => scope.login.start(flow.providerId, flow.methodId)),
            ),
            const SizedBox(width: Space.s),
            Button(label: 'Close', onPressed: _cancel),
          ],
        ),
      );
    } else {
      if (flow.userCode.isNotEmpty) add(_deviceCode(palette, scope, flow));
      if (flow.url.isNotEmpty) {
        add(
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                flow.instructions.isEmpty
                    ? 'Finish signing in in your browser.'
                    : flow.instructions,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.s),
              Row(
                children: [
                  Button(
                    label: scope.login.opened(flow.url)
                        ? 'Open again'
                        : 'Open in browser',
                    onPressed: () {
                      scope.login.open(flow.url);
                      setState(() {});
                    },
                  ),
                  const SizedBox(width: Space.xs),
                  Button(
                    label: _copied == flow.url ? 'Copied' : 'Copy link',
                    onPressed: () => _copy(flow.url),
                  ),
                ],
              ),
            ],
          ),
        );
      }
      if (flow.url.isEmpty && flow.userCode.isEmpty && !flow.hasPrompt()) {
        add(
          Text(
            'Starting…',
            style: TextStyles.body.copyWith(color: palette.textFaint),
          ),
        );
      }
      if (flow.progress.isNotEmpty) {
        add(
          Text(
            flow.progress,
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          ),
        );
      }
      if (flow.hasPrompt()) {
        final prompt = flow.prompt;
        add(
          _PromptView(
            // A new question starts with an empty field.
            key: ValueKey('${prompt.kind.value}:${prompt.message}'),
            prompt: prompt,
            onAnswer: (text) => _run(() => scope.backend.answerLogin(text)),
          ),
        );
      }
      add(
        Align(
          alignment: Alignment.centerLeft,
          child: Button(label: 'Cancel', onPressed: _cancel),
        ),
      );
    }
    if (_failure != null) {
      add(ErrorLine(_failure!, about: 'Signing in to ${flow.providerId}:'));
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: children,
    );
  }

  Widget _deviceCode(Palette palette, AppScope scope, LoginFlow flow) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          'Enter this code',
          style: TextStyles.caption.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.xs),
        Text(
          flow.userCode,
          style: TextStyles.title.copyWith(
            fontFamily: TextStyles.mono,
            fontSize: 28,
            letterSpacing: 2,
            color: palette.text,
          ),
        ),
        const SizedBox(height: Space.s),
        Row(
          children: [
            if (flow.verificationUrl.isNotEmpty) ...[
              Button(
                label:
                    'Open ${Uri.tryParse(flow.verificationUrl)?.host ?? flow.verificationUrl}',
                onPressed: () => scope.login.open(flow.verificationUrl),
              ),
              const SizedBox(width: Space.xs),
            ],
            Button(
              label: _copied == flow.userCode ? 'Copied' : 'Copy code',
              onPressed: () => _copy(flow.userCode),
            ),
          ],
        ),
      ],
    );
  }
}

/// A question from the provider: a pasted code or address, a secret, text,
/// or a choice.
class _PromptView extends StatefulWidget {
  const _PromptView({super.key, required this.prompt, required this.onAnswer});

  final LoginPrompt prompt;
  final ValueChanged<String> onAnswer;

  @override
  State<_PromptView> createState() => _PromptViewState();
}

class _PromptViewState extends State<_PromptView> {
  final _answer = TextEditingController();

  @override
  void dispose() {
    _answer.dispose();
    super.dispose();
  }

  void _submit() {
    final text = _answer.text.trim();
    if (text.isEmpty) return;
    _answer.clear();
    widget.onAnswer(text);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final prompt = widget.prompt;
    final code = prompt.kind == LoginPromptKind.LOGIN_PROMPT_KIND_CODE;
    if (prompt.kind == LoginPromptKind.LOGIN_PROMPT_KIND_SELECT) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (prompt.message.isNotEmpty) ...[
            Text(
              prompt.message,
              style: TextStyles.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.s),
          ],
          Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              for (final option in prompt.options)
                Button(
                  label: option.label,
                  onPressed: () => widget.onAnswer(option.id),
                ),
            ],
          ),
        ],
      );
    }
    final String message;
    if (code) {
      message =
          'Browser on another computer? Paste the address it ended '
          'up on, or the code.';
    } else {
      message = prompt.message;
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (message.isNotEmpty) ...[
          Text(
            message,
            style: (code ? TextStyles.caption : TextStyles.body).copyWith(
              color: code ? palette.textMuted : palette.text,
            ),
          ),
          const SizedBox(height: Space.xs + 2),
        ],
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _answer,
                placeholder: prompt.placeholder.isNotEmpty
                    ? prompt.placeholder
                    : code
                    ? 'Address or code'
                    : '',
                obscureText:
                    prompt.kind == LoginPromptKind.LOGIN_PROMPT_KIND_SECRET,
                autofocus: !code,
                onSubmitted: (_) => _submit(),
              ),
            ),
            const SizedBox(width: Space.s),
            ListenableBuilder(
              listenable: _answer,
              builder: (context, _) => Button(
                label: 'Continue',
                primary: !code,
                onPressed: _answer.text.trim().isEmpty ? null : _submit,
              ),
            ),
          ],
        ),
      ],
    );
  }
}
