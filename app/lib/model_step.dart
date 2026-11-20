import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'login.dart';
import 'name_step.dart' show StepCard;
import 'protocol/factory/v1/factory.pb.dart';
import 'provider_picker.dart';
import 'scope.dart';
import 'screens.dart' show ScreenOpener;
import 'tell_rivet.dart' show ErrorLine;
import 'usage.dart';

/// The provider whose sign-in the person started from the step in this
/// window, while the step shows what it connected; null otherwise. The
/// step stays up after the backend calls it done, until Continue.
ValueNotifier<String?> modelStepStarted(Backend backend) =>
    _started[backend] ??= ValueNotifier(null);

final _started = Expando<ValueNotifier<String?>>('model step started');

/// Whether to ask the person to connect a model: the backend is connected,
/// has said what it knows, and is a new Claktory with no model yet
/// (`Settings.onboarding.model` NEEDED), or the step is showing what was
/// just connected from it.
bool asksModel(BuildContext context) {
  final backend = AppScope.of(context);
  final store = backend.workspace;
  if (!backend.connected.value || !store.received) return false;
  final workspace = store.workspace;
  if (workspace.settings.onboarding.model ==
      OnboardingStep.ONBOARDING_STEP_NEEDED) {
    return true;
  }
  // Signed in from the step: it shows what connected until Continue.
  final started = modelStepStarted(backend).value;
  return started != null &&
      workspace.models.isNotEmpty &&
      workspace.providers.any((p) => p.id == started && _signedIn(p));
}

bool _signedIn(Provider provider) =>
    provider.accounts.isNotEmpty || provider.signedInWith.isNotEmpty;

/// The first launch's step after the name: sign in with Claude or ChatGPT,
/// use an API key, or not now. Then the window opens on Rivet, which
/// greets the person first.
class ModelStep extends StatelessWidget {
  const ModelStep({super.key});

  @override
  Widget build(BuildContext context) => StepCard(
    child: ConnectModel(started: modelStepStarted(AppScope.of(context))),
  );
}

/// The same choices as the step, in a dialog: from the composer of an agent
/// with no model to run on.
Future<void> showConnectModel(BuildContext context) {
  final started = ValueNotifier<String?>(null);
  return showModal(
    context,
    (context, close) => ConnectModel(started: started, close: close),
  ).whenComplete(started.dispose);
}

/// Connecting a model: the ways to, a sign-in under way, or what it
/// connected. [started] holds the provider signed in to from here. As a
/// dialog, [close] closes it; as the first launch's step it's null, and
/// Not now is offered instead.
class ConnectModel extends StatefulWidget {
  const ConnectModel({super.key, required this.started, this.close});

  final ValueNotifier<String?> started;
  final VoidCallback? close;

  @override
  State<ConnectModel> createState() => _ConnectModelState();
}

class _ConnectModelState extends State<ConnectModel> {
  /// Choosing the provider of an API key.
  var _keys = false;
  var _busy = false;
  String? _failure;

  bool get _step => widget.close == null;

  Future<void> _start(Provider provider, String methodId) async {
    final scope = AppScope.scope(context);
    setState(() {
      _failure = null;
      _keys = false;
    });
    widget.started.value = provider.id;
    try {
      await scope.login.start(
        provider.id,
        methodId,
        addAccount: provider.accounts.isNotEmpty,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _failure = failure.message);
    }
  }

  Future<void> _skip() async {
    final backend = AppScope.of(context);
    setState(() {
      _busy = true;
      _failure = null;
    });
    // Rivet first: once skipped, the step goes, and this with it.
    _openRivet();
    try {
      await backend.skipModelStep();
      widget.started.value = null;
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _failure = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Done: off to Rivet, which greets the person first; from a dialog,
  /// back to where it was opened.
  void _continue() {
    final close = widget.close;
    if (close != null) {
      close();
    } else {
      _openRivet();
    }
    widget.started.value = null;
  }

  /// Opens Rivet's conversation, under the step (on a phone, over the list).
  void _openRivet() {
    final rivet = rivetIn(AppScope.of(context).workspace.workspace);
    if (rivet != null) ScreenOpener.openConversation(context, rivet.id);
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: Listenable.merge([store, widget.started]),
      builder: (context, _) {
        final workspace = store.workspace;
        final flow = workspace.hasLogin() ? workspace.login : null;
        final started = widget.started.value;
        final connected = started == null || flow?.providerId == started
            ? null
            : workspace.providers
                  .where((p) => p.id == started && _signedIn(p))
                  .firstOrNull;
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: connected != null && workspace.models.isNotEmpty
              ? _connected(context, connected)
              : flow != null
              ? _signingIn(context, workspace, flow)
              : _keys
              ? _keyChoice(context, workspace)
              : _choices(context, workspace),
        );
      },
    );
  }

  List<Widget> _heading(BuildContext context, String title, String body) {
    final palette = Palette.of(context);
    return [
      Text(title, style: TextStyles.emphasis.copyWith(color: palette.text)),
      const SizedBox(height: Space.s),
      Text(body, style: TextStyles.body.copyWith(color: palette.textMuted)),
      const SizedBox(height: Space.xl),
    ];
  }

  List<Widget> _choices(BuildContext context, Workspace workspace) {
    final palette = Palette.of(context);
    Provider? withMethod(String id, String method) => workspace.providers
        .where((p) => p.id == id && p.methods.any((m) => m.id == method))
        .firstOrNull;
    final claude = withMethod('anthropic', 'oauth');
    final chatGpt = withMethod('openai-codex', 'oauth');
    final keys = workspace.providers.any(
      (p) => p.methods.any((m) => m.id == 'api_key'),
    );
    return [
      ..._heading(
        context,
        'Connect a model',
        'Your agents need an AI model to work. Sign in with your Claude or '
            'ChatGPT plan, or use an API key.',
      ),
      if (claude != null)
        _Choice(
          title: 'Sign in with Claude',
          note: 'Pro, Max or Ultra',
          providerId: 'anthropic',
          onTap: () => _start(claude, 'oauth'),
        ),
      if (chatGpt != null)
        _Choice(
          title: 'Sign in with ChatGPT',
          note: 'Plus or Pro',
          providerId: 'openai-codex',
          onTap: () => _start(chatGpt, 'oauth'),
        ),
      if (keys)
        _Choice(
          title: 'Use an API key',
          note: 'From Anthropic, OpenAI, OpenRouter or another provider',
          onTap: () => setState(() {
            _keys = true;
            _failure = null;
          }),
        ),
      if (_failure case final failure?) ...[
        const SizedBox(height: Space.s),
        ErrorLine(failure, about: 'Connecting a model:'),
      ],
      const SizedBox(height: Space.l),
      Row(
        children: [
          if (_step)
            Expanded(
              child: Text(
                'Rivet can answer once a model is connected.',
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            )
          else
            const Spacer(),
          const SizedBox(width: Space.s),
          _step
              ? Button(label: 'Not now', onPressed: _busy ? null : _skip)
              : Button(label: 'Cancel', onPressed: widget.close),
        ],
      ),
    ];
  }

  List<Widget> _keyChoice(BuildContext context, Workspace workspace) {
    // Each provider with only its API key way in.
    final providers = [
      for (final provider in workspace.providers)
        if (provider.methods.any((m) => m.id == 'api_key'))
          provider.deepCopy()..methods.retainWhere((m) => m.id == 'api_key'),
    ];
    return [
      ..._heading(
        context,
        'Use an API key',
        "Choose where your key is from. You'll paste it next.",
      ),
      ProviderPicker(
        providers: providers,
        inline: true,
        autofocus: true,
        placeholder: 'Search providers',
        onPick: (provider, method) => _start(
          workspace.providers.firstWhere((p) => p.id == provider.id),
          method.id,
        ),
      ),
      const SizedBox(height: Space.l),
      Row(
        children: [
          Button(label: 'Back', onPressed: () => setState(() => _keys = false)),
        ],
      ),
    ];
  }

  List<Widget> _signingIn(
    BuildContext context,
    Workspace workspace,
    LoginFlow flow,
  ) {
    final provider = workspace.providers
        .where((p) => p.id == flow.providerId)
        .firstOrNull;
    var method = flow.methodId;
    for (final candidate in provider?.methods ?? const <LoginMethod>[]) {
      if (candidate.id == flow.methodId) method = candidate.label;
    }
    return [
      ..._heading(
        context,
        'Connect a model',
        'Finish signing in, and your agents can get to work.',
      ),
      LoginPanel(flow: flow, method: method),
    ];
  }

  List<Widget> _connected(BuildContext context, Provider provider) {
    final palette = Palette.of(context);
    final meters = usageMeters([provider]);
    final meter = meters.isEmpty ? UsageMeter(provider) : meters.first;
    final plan = meter.plan.isEmpty || meter.plan == 'API key'
        ? provider.name
        : meter.plan;
    final rivet = rivetIn(AppScope.of(context).workspace.workspace);
    return [
      Text(
        'Connected',
        style: TextStyles.emphasis.copyWith(color: palette.text),
      ),
      const SizedBox(height: Space.l),
      Row(
        children: [
          UsageCircle(meter: meter),
          const SizedBox(width: Space.s),
          Flexible(
            child: Text(
              plan,
              style: TextStyles.label.copyWith(color: palette.text),
            ),
          ),
        ],
      ),
      const SizedBox(height: Space.l),
      Text(
        _step && rivet != null
            ? '${rivet.name} is ready.'
            : 'Your agents can work now.',
        style: TextStyles.body.copyWith(color: palette.textMuted),
      ),
      const SizedBox(height: Space.xl),
      Align(
        alignment: Alignment.centerRight,
        child: Button(label: 'Continue', primary: true, onPressed: _continue),
      ),
    ];
  }
}

/// One way to connect: a full-width row with a hairline, its title and a
/// note under it, and the provider's mark when it has one.
class _Choice extends StatelessWidget {
  const _Choice({
    required this.title,
    required this.note,
    required this.onTap,
    this.providerId,
  });

  final String title;
  final String note;
  final VoidCallback onTap;
  final String? providerId;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final mark = providerId == null ? null : providerMark(providerId!);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Semantics(
        button: true,
        label: title,
        excludeSemantics: true,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.l,
              vertical: Space.m,
            ),
            decoration: BoxDecoration(
              color: hovered ? palette.hover : null,
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              children: [
                if (mark != null) ...[
                  CustomPaint(
                    size: const Size.square(20),
                    painter: _MarkPainter(
                      mark,
                      providerColor(providerId!, palette),
                    ),
                  ),
                  const SizedBox(width: Space.m),
                ],
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        style: TextStyles.label.copyWith(color: palette.text),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        note,
                        style: TextStyles.caption.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A provider's mark, filling the square it's given, in its color.
class _MarkPainter extends CustomPainter {
  const _MarkPainter(this.mark, this.color);

  final Path mark;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final bounds = mark.getBounds();
    final scale = size.shortestSide / math.max(bounds.width, bounds.height);
    final center = size.center(Offset.zero);
    canvas
      ..save()
      ..translate(center.dx, center.dy)
      ..scale(scale)
      ..translate(-bounds.center.dx, -bounds.center.dy)
      ..drawPath(mark, Paint()..color = color)
      ..restore();
  }

  @override
  bool shouldRepaint(_MarkPainter old) =>
      old.mark != mark || old.color != color;
}
