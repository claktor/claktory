import 'dart:async';
import 'dart:developer' as developer;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'backend.dart';
import 'controls.dart';
import 'claktory_identity.dart' show claktoryName;
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/input.dart' show InputKind;
import 'mobile/lines.dart';
import 'loopback_forward.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'provider_models.dart' show ProviderModels;
import 'scope.dart';
import 'state.dart';
import 'tell_rivet.dart';
import 'usage.dart';
import 'usage_wait.dart' show UsageWaitOffer;

/// Opens a sign-in page in the browser for flows this window started.
///
/// Every window shows the same `Workspace.login`, but only the one where the
/// user clicked "Sign in" opens the browser, once, when the provider publishes
/// the address. That may be after StartLogin's ack, since the provider's flow
/// runs on its own.
///
/// The provider sends the browser back to `http://localhost:<port>/…` on
/// the backend's machine. When that's another machine, this device
/// forwards the port there while the flow lasts ([forwards]).
class LoginLauncher {
  LoginLauncher({
    required this.workspace,
    required this._send,
    this.forwards,
    this._open,
  }) {
    workspace.addListener(_check);
  }

  final WorkspaceStore workspace;
  final Future<void> Function(StartLogin start) _send;

  /// Null where nothing is forwarded (tests of one view).
  final LoopbackForwards? forwards;

  /// Opens pages; null: the browser (or on iPhone the sign-in sheet).
  final Future<bool> Function(Uri url)? _open;

  /// The forwarding for the flow at its address, while it lasts.
  Forwarding? _forwarding;

  /// The flow this window started and hasn't opened yet.
  StartLogin? _waiting;

  /// An address that was showing when [_waiting] started; it belongs to the
  /// flow being replaced, so it's never opened for this one.
  var _stale = '';
  final _opened = <String>{};

  /// Whether the last sign-in this window started for a provider and method
  /// added an account, so trying it again adds one too.
  final _adding = <String, bool>{};

  /// Whether this window's last sign-in with [providerId]'s [methodId] was
  /// adding an account rather than replacing the first.
  bool adding(String providerId, String methodId) =>
      _adding['$providerId/$methodId'] ?? false;

  /// Starts signing in and opens the browser when the address arrives. With
  /// [addAccount], the account is added to the provider's, keeping the ones
  /// signed in.
  Future<void> start(
    String providerId,
    String methodId, {
    bool addAccount = false,
  }) async {
    final start = StartLogin(
      providerId: providerId,
      methodId: methodId,
      addAccount: addAccount,
    );
    _adding['$providerId/$methodId'] = addAccount;
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
  void forget() {
    _waiting = null;
    _stopForwarding();
  }

  /// The forwarding for the flow at [url], if this window opened it and
  /// it lasts.
  Forwarding? forwardingFor(String url) {
    final forwarding = _forwarding;
    if (forwarding == null || '${forwarding.url}' != url) return null;
    return (forwards?.active.contains(forwarding) ?? false) ? forwarding : null;
  }

  /// Whether this window opened [url] already.
  bool opened(String url) => _opened.contains(url);

  /// Opens [url] in the browser, now.
  Future<void> open(String url) async {
    final uri = Uri.tryParse(url);
    if (uri == null) return;
    _opened.add(url);
    final forwards = this.forwards;
    if (forwards == null) {
      try {
        if (!await (_open ?? launchUrl)(uri)) {
          _log('the browser did not open $url');
        }
      } on PlatformException catch (error) {
        _log('could not open $url: ${error.message}');
      }
      return;
    }
    final forwarding = await openInBrowser(
      uri,
      forwards: forwards,
      launch: _open,
    );
    if (forwarding == null) return;
    final login = workspace.workspace;
    // The flow moved on while its ports opened.
    if (!login.hasLogin() || login.login.url != url) {
      unawaited(forwards.stop(forwarding));
      return;
    }
    if (_forwarding != forwarding) _stopForwarding();
    _forwarding = forwarding;
  }

  void _stopForwarding() {
    final forwarding = _forwarding;
    _forwarding = null;
    if (forwarding == null) return;
    unawaited(forwards?.stop(forwarding));
    unawaited(closeSheet());
  }

  void _check() {
    // The flow forwarded for ended (signed in, failed, cancelled) or
    // moved on.
    final forwarding = _forwarding;
    if (forwarding != null) {
      final shown = this.workspace.workspace;
      if (!shown.hasLogin() ||
          shown.login.url != '${forwarding.url}' ||
          shown.login.error.isNotEmpty) {
        _stopForwarding();
      }
    }
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

  void dispose() {
    workspace.removeListener(_check);
    _stopForwarding();
  }
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
                  : 'Signing in with ${widget.method}…',
              style: TypeScale.of(context).label.copyWith(color: palette.text),
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
          style: TypeScale.of(context).body.copyWith(color: palette.danger),
        ),
      );
      add(
        Row(
          children: [
            Button(
              label: 'Try again',
              primary: true,
              onPressed: () => _run(
                () => scope.login.start(
                  flow.providerId,
                  flow.methodId,
                  addAccount: scope.login.adding(
                    flow.providerId,
                    flow.methodId,
                  ),
                ),
              ),
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
                    ? 'Finish signing in with your browser.'
                    : flow.instructions,
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.s),
              ControlsLine(
                gap: Space.xs,
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
      if (flow.url.isNotEmpty) {
        final forwarding = scope.login.forwardingFor(flow.url);
        if (forwarding != null) {
          add(
            ListenableBuilder(
              listenable: scope.backend.forwards,
              builder: (context, _) => ForwardingLine(
                forwards: scope.backend.forwards,
                forwarding: forwarding,
                machine: _machine(),
              ),
            ),
          );
        }
      }
      if (flow.url.isEmpty && flow.userCode.isEmpty && !flow.hasPrompt()) {
        add(
          Text(
            'Starting…',
            style: TypeScale.of(context).body
                .copyWith(color: palette.textFaint),
          ),
        );
      }
      if (flow.progress.isNotEmpty) {
        add(
          Text(
            flow.progress,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
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

  /// The Claktory's name, for saying where the browser's way back goes.
  String _machine() => claktoryName(AppScope.of(context));

  Widget _deviceCode(Palette palette, AppScope scope, LoginFlow flow) {
    final host = flow.verificationUrl.isEmpty
        ? ''
        : Uri.tryParse(flow.verificationUrl)?.host ?? flow.verificationUrl;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          host.isEmpty ? 'Enter this code' : 'Enter this code at $host',
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.xs),
        Text(
          flow.userCode,
          style: TypeScale.of(context).title.copyWith(
            fontFamily: TextStyles.mono,
            fontSize: 28,
            letterSpacing: 2,
            color: palette.text,
          ),
        ),
        const SizedBox(height: Space.s),
        ControlsLine(
          gap: Space.xs,
          children: [
            if (flow.verificationUrl.isNotEmpty)
              Button(
                label: 'Open $host',
                onPressed: () => scope.login.open(flow.verificationUrl),
              ),
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
              style: TypeScale.of(context).body.copyWith(color: palette.text),
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
          'Signing in on another computer? Paste the address your browser '
          'ended up on, or the code.';
    } else {
      message = prompt.message;
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (message.isNotEmpty) ...[
          Text(
            message,
            style:
                (code
                        ? TypeScale.of(context).caption
                        : TypeScale.of(context).body)
                    .copyWith(color: code ? palette.textMuted : palette.text),
          ),
          const SizedBox(height: Space.xs + 2),
        ],
        ActionLine(
          lead: TextField(
            controller: _answer,
            placeholder: prompt.placeholder.isNotEmpty
                ? prompt.placeholder
                : code
                ? 'Address or code'
                : prompt.kind == LoginPromptKind.LOGIN_PROMPT_KIND_SECRET
                ? 'Paste your API key'
                : '',
            kind: prompt.kind == LoginPromptKind.LOGIN_PROMPT_KIND_SECRET
                ? InputKind.secret
                : InputKind.text,
            // An address, a code, a key: as typed.
            autocorrect: false,
            textCapitalization: TextCapitalization.none,
            autofocus: !code,
            onSubmitted: (_) => _submit(),
          ),
          action: ListenableBuilder(
            listenable: _answer,
            builder: (context, _) => Button(
              label: 'Continue',
              primary: !code,
              onPressed: _answer.text.trim().isEmpty ? null : _submit,
            ),
          ),
        ),
      ],
    );
  }
}

/// A provider in Settings → Providers: the accounts signed in to it, in the
/// order requests try them (drag to reorder, or Use this one), each with who
/// it is, its plan, how much of its limits it used, whether requests go to
/// it now and Sign out; then the ways to sign in, or once it has an account
/// to add another without replacing any; and the sign-in in progress.
class ProviderCard extends StatefulWidget {
  const ProviderCard({super.key, required this.provider, required this.flow});

  final Provider provider;

  /// The sign-in in progress for this provider, if any.
  final LoginFlow? flow;

  @override
  State<ProviderCard> createState() => _ProviderCardState();
}

class _ProviderCardState extends State<ProviderCard> {
  String? _failure;

  Future<void> _run(Future<void> Function() request) async {
    setState(() => _failure = null);
    try {
      await request();
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _failure = error.message);
    }
  }

  /// "Add another Claude subscription", "Add an API key": what adding an
  /// account with [method] adds.
  String _addLabel(LoginMethod method) {
    if (method.id == 'api_key') return 'Add an API key';
    final product = switch (widget.provider.id) {
      'anthropic' => 'Claude ',
      'openai-codex' => 'ChatGPT ',
      _ => '',
    };
    return 'Add another ${product}subscription';
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final provider = widget.provider;
    final flow = widget.flow;
    final accounts = provider.accounts;
    // A backend from before several accounts lists none, and replaces the
    // one credential on every sign-in.
    final listsAccounts = accounts.isNotEmpty;
    final signedIn = listsAccounts || provider.signedInWith.isNotEmpty;
    var methodLabel = flow?.methodId ?? '';
    for (final method in provider.methods) {
      if (method.id == flow?.methodId) methodLabel = method.label;
    }
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Flexible(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      provider.name,
                      style: TypeScale.of(context).label
                          .copyWith(color: palette.text),
                    ),
                    if (!listsAccounts) ...[
                      const SizedBox(height: 2),
                      Text(
                        signedIn
                            ? 'Signed in with ${provider.signedInWith}'
                            : 'Not signed in',
                        style: TypeScale.of(context).caption.copyWith(
                          color: signedIn
                              ? palette.textMuted
                              : palette.textFaint,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              // At the largest text on a narrow phone it gives way to the
              // name, its word on two lines, rather than past the edge.
              if (!listsAccounts && provider.canSignOut)
                Flexible(
                  child: Button(
                    label: 'Sign out',
                    onPressed: () =>
                        _run(() => scope.backend.signOut(provider.id)),
                  ),
                ),
            ],
          ),
          if (listsAccounts) ...[
            const SizedBox(height: Space.s),
            _AccountList(
              provider: provider,
              onMove: (account, position) => _run(
                () => scope.backend.moveAccount(
                  provider.id,
                  account.id,
                  position,
                ),
              ),
              onSignOut: (account) => _run(
                () => scope.backend.signOut(provider.id, accountId: account.id),
              ),
              onUpdate: (update) =>
                  _run(() => scope.backend.updateAccount(update)),
              // Into the usual sign-in, adding: the same account replaces
              // itself in place, and another one never overwrites a
              // working one.
              onSignInAgain: (account) => _run(
                () => scope.login.start(
                  provider.id,
                  account.methodId.isEmpty ? 'oauth' : account.methodId,
                  addAccount: true,
                ),
              ),
            ),
            if (accounts.length > 1) ...[
              const SizedBox(height: Space.xs),
              Text(
                'When one account reaches its limit, Claktory switches to '
                'the next. When all have, agents wait until one resets.',
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
            ],
          ],
          // Signed in isn't in use: none of its models run yet, so the
          // agents are still where they were (#343 U1).
          if (signedIn && provider.agents == 0) ...[
            const SizedBox(height: Space.s),
            Text(
              noAgentsOnProvider(provider),
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textMuted),
            ),
          ],
          // Agents waiting on another provider's limit can carry on here.
          if (signedIn) UsageWaitOffer(provider: provider),
          // How long the provider keeps a conversation ready: a choice
          // where Claktory can make it, else a line saying how long.
          if (cacheChoices(provider).isNotEmpty) ...[
            const SizedBox(height: Space.m),
            Wrap(
              spacing: Space.m,
              runSpacing: Space.xs,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Text(
                  'Keep conversations ready',
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                ),
                SizedBox(
                  width: 140,
                  child: Select(
                    value: cacheLifetime(provider),
                    options: [
                      if (!cacheChoices(provider)
                          .contains(cacheLifetime(provider)))
                        SelectOption(
                          cacheLifetime(provider),
                          cacheLifetimeLabel(cacheLifetime(provider)),
                        ),
                      for (final choice in cacheChoices(provider))
                        SelectOption(choice, cacheLifetimeLabel(choice)),
                    ],
                    onChanged: (ttl) => _run(
                      () => scope.backend.updateSettings(
                        UpdateSettings(
                          cacheTtl: CacheTtl(key: provider.id, ttl: ttl),
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ),
            if (cacheCaption(provider) case final caption?) ...[
              const SizedBox(height: Space.xs),
              Text(
                caption,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
            ],
          ] else if (provider.cacheLifetime.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            Text(
              '${provider.name} keeps conversations ready for '
              '${cacheLifetimeLabel(provider.cacheLifetime)}.',
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textMuted),
            ),
          ],
          // Where its models come from, and Refresh models (friend bug #14).
          if (signedIn) ...[
            const SizedBox(height: Space.m),
            ProviderModels(provider: provider),
          ],
          if (flow != null) ...[
            const SizedBox(height: Space.l),
            Container(height: 1, color: palette.hairline),
            const SizedBox(height: Space.l),
            LoginPanel(flow: flow, method: methodLabel),
          ] else if (provider.methods.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            Wrap(
              spacing: Space.xs,
              runSpacing: Space.xs,
              children: [
                for (var i = 0; i < provider.methods.length; i++)
                  Button(
                    label: listsAccounts
                        ? _addLabel(provider.methods[i])
                        : 'Sign in with ${provider.methods[i].label}',
                    primary: !signedIn && i == 0,
                    onPressed: () => _run(
                      () => scope.login.start(
                        provider.id,
                        provider.methods[i].id,
                        addAccount: listsAccounts,
                      ),
                    ),
                  ),
              ],
            ),
          ],
          if (_failure != null)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: ErrorLine(_failure!, about: 'In Settings:'),
            ),
        ],
      ),
    );
  }
}

/// A provider's accounts, one row each with hairlines between, in the order
/// requests try them. With more than one, each row Claktory keeps has a grip
/// to drag it to another place, and Use this one; a key from the environment
/// stays last, as the backend keeps it.
class _AccountList extends StatelessWidget {
  const _AccountList({
    required this.provider,
    required this.onMove,
    required this.onSignOut,
    required this.onUpdate,
    required this.onSignInAgain,
  });

  final Provider provider;
  final void Function(Account account, int position) onMove;
  final void Function(Account account) onSignOut;
  final Future<void> Function(UpdateAccount update) onUpdate;
  final void Function(Account account) onSignInAgain;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final accounts = provider.accounts;
    // The accounts that can move: all but one from the environment.
    final movable = accounts.where((a) => a.canSignOut).length;
    return ReorderableList(
      shrinkWrap: true,
      physics: const NeverScrollableScrollPhysics(),
      itemCount: accounts.length,
      onReorderItem: (from, to) {
        final position = to.clamp(0, movable - 1);
        if (position != from) onMove(accounts[from], position);
      },
      proxyDecorator: (child, index, animation) => DecoratedBox(
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: BorderRadius.circular(Radii.control),
          border: Border.all(color: palette.hairline),
          boxShadow: floatingShadow,
        ),
        child: child,
      ),
      itemBuilder: (context, index) {
        final account = accounts[index];
        // Claktory keeps it (not a key from the environment).
        final kept = account.canSignOut;
        return KeyedSubtree(
          key: ValueKey(account.id),
          child: DecoratedBox(
            decoration: BoxDecoration(
              border: index == 0
                  ? null
                  : Border(top: BorderSide(color: palette.hairline)),
            ),
            child: _AccountRow(
              provider: provider,
              account: account,
              index: index,
              grip: movable > 1 && kept,
              gripSpace: movable > 1,
              onUseFirst: index == 0 || !kept ? null : () => onMove(account, 0),
              onSignOut: kept ? () => onSignOut(account) : null,
              onUpdate: onUpdate,
              onSignInAgain: () => onSignInAgain(account),
            ),
          ),
        );
      },
    );
  }
}

/// Where a provider's API keys get more credit.
String? _billingUrl(String providerId) => switch (providerId) {
  'anthropic' => 'https://console.anthropic.com/settings/billing',
  'openai' => 'https://platform.openai.com/settings/organization/billing',
  'openrouter' => 'https://openrouter.ai/settings/credits',
  _ => null,
};

class _AccountRow extends StatefulWidget {
  const _AccountRow({
    required this.provider,
    required this.account,
    required this.index,
    required this.grip,
    required this.gripSpace,
    required this.onUseFirst,
    required this.onSignOut,
    required this.onUpdate,
    required this.onSignInAgain,
  });

  final Provider provider;
  final Account account;
  final int index;

  /// It has a grip to drag it by; [gripSpace] keeps the room for one when
  /// it doesn't, so the rows line up.
  final bool grip;
  final bool gripSpace;
  final VoidCallback? onUseFirst;
  final VoidCallback? onSignOut;
  final Future<void> Function(UpdateAccount update) onUpdate;

  /// The provider refused to refresh it (Account.sign_in_again).
  final VoidCallback onSignInAgain;

  @override
  State<_AccountRow> createState() => _AccountRowState();
}

class _AccountRowState extends State<_AccountRow> {
  /// The name being typed while renaming a key; null when not renaming.
  TextEditingController? _name;

  @override
  void dispose() {
    _name?.dispose();
    super.dispose();
  }

  void _rename() =>
      setState(() => _name = TextEditingController(text: widget.account.label));

  void _stopRenaming() {
    final name = _name;
    setState(() => _name = null);
    // After the frame, so the field is gone before its controller.
    WidgetsBinding.instance.addPostFrameCallback((_) => name?.dispose());
  }

  void _saveName() {
    final label = _name?.text.trim() ?? '';
    _stopRenaming();
    if (label == widget.account.label) return;
    // Empty goes back to the name Claktory gives it.
    widget.onUpdate(
      UpdateAccount(
        providerId: widget.provider.id,
        accountId: widget.account.id,
        label: label,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final provider = widget.provider;
    final account = widget.account;
    final key = account.methodId == 'api_key';
    final meter = UsageMeter(provider, account);
    final usage = meter.usage;
    final weekly = usage?.windows
        .where((w) => w.id == headlineWindow)
        .firstOrNull;
    final name = _name;
    return Ticking(
      // Its state changes on its own when a limit's time comes.
      active: account.exhaustedUntil > 0 || account.fableExhaustedUntil > 0,
      builder: (context, now) {
        final state = accountState(provider, account, now);
        final caption = TypeScale.of(context).caption.copyWith(
          color: palette.textMuted,
          fontFeatures: const [FontFeature.tabularFigures()],
        );
        final details = [
          if (account.plan.isNotEmpty) account.plan,
          // An API key's spending limit, what's left of it ("\$12.40 left").
          if (meter.credit?.leftShown case final left? when left.isNotEmpty)
            left,
          // Its own lines below say these.
          if (!account.outOfCredit && !account.signInAgain) state.text,
          if (weekly != null)
            '${formatPercent(weekly.usedPercent)} of the weekly limit used',
        ];
        final billing = _billingUrl(provider.id);
        final danger = TypeScale.of(context).caption
            .copyWith(color: palette.danger);
        // Its actions: beside the account, wrapping under each other when
        // there's no room; on a phone under its name, so each reads on one
        // line.
        final phone = isPhoneLayout(context);
        final buttons = <Widget>[
          if (name != null) ...[
            Button(label: 'Save', primary: true, onPressed: _saveName),
            Button(label: 'Cancel', onPressed: _stopRenaming),
          ] else ...[
            if (account.signInAgain)
              Button(
                label: 'Sign in again…',
                primary: true,
                onPressed: widget.onSignInAgain,
              ),
            if (account.outOfCredit)
              Button(
                label: 'Try again',
                onPressed: () => widget.onUpdate(
                  UpdateAccount(
                    providerId: provider.id,
                    accountId: account.id,
                    retry: true,
                  ),
                ),
              ),
            if (key && account.canSignOut)
              Button(label: 'Rename', onPressed: _rename),
            if (widget.onUseFirst != null)
              Button(label: 'Use this one', onPressed: widget.onUseFirst),
            if (widget.onSignOut != null)
              Button(label: 'Sign out', onPressed: widget.onSignOut),
          ],
        ];
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: Space.s),
          child: Row(
            // On a phone the actions sit under the name: the circle stays
            // by the name, not the middle of the buttons.
            crossAxisAlignment: phone
                ? CrossAxisAlignment.start
                : CrossAxisAlignment.center,
            children: [
              if (widget.grip)
                Semantics(
                  label: 'Drag to reorder',
                  child: ReorderableDragStartListener(
                    index: widget.index,
                    child: const MouseRegion(
                      cursor: SystemMouseCursors.grab,
                      child: _Grip(),
                    ),
                  ),
                )
              else if (widget.gripSpace)
                const SizedBox(width: 16),
              if (widget.gripSpace) const SizedBox(width: Space.s),
              UsageCircle(meter: meter),
              const SizedBox(width: Space.s),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (name != null)
                      SizedBox(
                        width: 240,
                        child: CallbackShortcuts(
                          bindings: {
                            const SingleActivator(LogicalKeyboardKey.escape):
                                _stopRenaming,
                          },
                          child: TextField(
                            controller: name,
                            placeholder: 'Name',
                            kind: InputKind.text,
                            autofocus: true,
                            onSubmitted: (_) => _saveName(),
                          ),
                        ),
                      )
                    else
                      Text(
                        account.label.isEmpty
                            ? 'Account ${widget.index + 1}'
                            : account.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).body
                            .copyWith(color: palette.text),
                      ),
                    Text(details.join(' · '), style: caption),
                    // Requests skip it until it's signed in to again.
                    if (account.signInAgain)
                      Text('Sign-in expired', style: danger),
                    if (account.outOfCredit)
                      Wrap(
                        children: [
                          Text('Out of credit. Add credit at ', style: danger),
                          if (billing == null)
                            Text(provider.name, style: danger)
                          else
                            Pressable(
                              onTap: () => scope.login.open(billing),
                              builder: (context, hovered) => Text(
                                provider.name,
                                style: danger.copyWith(
                                  decoration: TextDecoration.underline,
                                  decorationColor: palette.danger,
                                ),
                              ),
                            ),
                          Text('.', style: danger),
                        ],
                      ),
                    if (phone && buttons.isNotEmpty) ...[
                      const SizedBox(height: Space.s),
                      Wrap(
                        spacing: Space.s,
                        runSpacing: Space.s,
                        children: buttons,
                      ),
                    ],
                  ],
                ),
              ),
              if (!phone && buttons.isNotEmpty) ...[
                const SizedBox(width: Space.s),
                Flexible(
                  child: Wrap(
                    alignment: WrapAlignment.end,
                    spacing: Space.xs,
                    runSpacing: Space.xs,
                    children: buttons,
                  ),
                ),
              ],
            ],
          ),
        );
      },
    );
  }
}

/// Lucide's grip-vertical: two columns of three dots, 16px, faint until
/// hovered.
class _Grip extends StatefulWidget {
  const _Grip();

  @override
  State<_Grip> createState() => _GripState();
}

class _GripState extends State<_Grip> {
  var _hovered = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: CustomPaint(
        size: const Size.square(16),
        painter: _GripPainter(_hovered ? palette.text : palette.textMuted),
      ),
    );
  }
}

class _GripPainter extends CustomPainter {
  const _GripPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = size.width / 24;
    final paint = Paint()..color = color;
    for (final x in [9.0, 15.0]) {
      for (final y in [5.0, 12.0, 19.0]) {
        canvas.drawCircle(Offset(x * scale, y * scale), 1.25 * scale, paint);
      }
    }
  }

  @override
  bool shouldRepaint(_GripPainter old) => old.color != color;
}

/// The lifetimes the person can choose for how long [provider] keeps a
/// conversation ready, default first; a backend from before the choices
/// came (#281) offers Anthropic's two through its cache_ttl alone.
List<String> cacheChoices(Provider provider) {
  if (provider.cacheTtlChoices.isNotEmpty) return provider.cacheTtlChoices;
  return provider.cacheTtl.isEmpty ? const [] : const ['5m', '1h'];
}

/// How long [provider] keeps a conversation ready now.
String cacheLifetime(Provider provider) => provider.cacheLifetime.isNotEmpty
    ? provider.cacheLifetime
    : provider.cacheTtl;

/// "5 minutes", "30 minutes", "1 hour", "12 hours"; as it's sent for one
/// this app doesn't know.
String cacheLifetimeLabel(String lifetime) => switch (lifetime) {
  '5m' => '5 minutes',
  '30m' => '30 minutes',
  '1h' => '1 hour',
  '12h' => '12 hours',
  _ => lifetime,
};

/// What choosing a longer lifetime costs and saves, for the choices
/// [provider] offers.
String? cacheCaption(Provider provider) {
  final choices = cacheChoices(provider);
  if (choices.contains('12h')) {
    return '12 hours costs about twice as much each time a conversation is '
        'made ready. It pays off only if you often come back to '
        'conversations between 30 minutes and 12 hours later.';
  }
  if (choices.contains('1h')) {
    return '${provider.name} keeps a conversation ready so the next message '
        'is cheaper. An hour costs a bit more each time (2× instead of '
        '1.25×) and saves money after longer breaks.';
  }
  return null;
}
