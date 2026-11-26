import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'usage_resets.dart';

/// The usage window a provider's circle shows.
const headlineWindow = 'weekly';

/// What one circle stands for: one account signed in to a provider (one of
/// two Claude subscriptions, say), or the provider itself when the backend
/// doesn't list its accounts.
class UsageMeter {
  const UsageMeter(this.provider, [this.account]);

  final Provider provider;
  final Account? account;

  /// Its usage limits; null when it has none to read (an API key).
  ProviderUsage? get usage {
    final account = this.account;
    if (account != null) return account.hasUsage() ? account.usage : null;
    return provider.hasUsage() ? provider.usage : null;
  }

  /// What its API key has to spend, as its provider says (OpenRouter,
  /// #343 U8); null when the provider doesn't say or it isn't read yet.
  AccountCredit? get credit {
    final account = this.account;
    return account != null && account.hasCredit() ? account.credit : null;
  }

  /// The plan as the provider names it, empty when unknown.
  String get plan => account?.plan ?? provider.plan;

  /// Who it is: the account's email or name, empty when unknown.
  String get who => account?.label ?? '';

  /// One of a provider's several accounts, whose order says which
  /// requests go to first.
  bool get oneOfMany => account != null && provider.accounts.length > 1;

  /// Requests go to it now, rather than to the provider's other accounts.
  bool get inUseOfMany => oneOfMany && accountInUse(account!);

  /// First in the order requests try the provider's accounts: used
  /// whenever it has room.
  bool get first =>
      account != null &&
      provider.accounts.isNotEmpty &&
      provider.accounts.first.id == account!.id;

  /// What screen readers and tests call its circle.
  String get name => [
    '${provider.name} usage',
    if (who.isNotEmpty) who,
    if (inUseOfMany) 'in use',
  ].join(', ');
}

/// One meter per account signed in, provider by provider, in the order
/// requests try them.
List<UsageMeter> usageMeters(List<Provider> providers) => [
  for (final provider in providers)
    if (provider.accounts.isNotEmpty)
      for (final account in provider.accounts) UsageMeter(provider, account)
    else if (provider.signedInWith.isNotEmpty)
      UsageMeter(provider),
];

/// One small circle per signed-in account (a provider with two Claude
/// subscriptions has two), in the provider's color with its product mark
/// inside. A subscription's ring fills as its weekly limit is used; the
/// hover card gives who it is, its plan and every limit's numbers. Accounts
/// without limits (API keys) show the mark alone, which doesn't read as a
/// meter.
///
/// With [onAdd], a + after them, a circle's size, to add another provider
/// (a friend with one couldn't find where to add a second). [leading]
/// comes first in the same row (the sidebar's who's-at-work badge): with
/// many accounts they all wrap together onto more lines, each starting at
/// the row's left edge, each line's cells centred on one another (Matt,
/// with four subscriptions).
class ProviderUsageCircles extends StatelessWidget {
  const ProviderUsageCircles({
    super.key,
    required this.providers,
    this.onAdd,
    this.leading,
  });

  final List<Provider> providers;
  final VoidCallback? onAdd;
  final Widget? leading;

  @override
  Widget build(BuildContext context) {
    final onAdd = this.onAdd;
    return Wrap(
      spacing: Space.xs,
      runSpacing: Space.xs,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        ?leading,
        for (final meter in usageMeters(providers)) UsageCircle(meter: meter),
        if (onAdd != null) AddProviderButton(onTap: onAdd),
      ],
    );
  }
}

/// What the + after the usage circles says, on hover and to VoiceOver.
const addProviderLabel = 'Add an AI provider';

/// A + in a usage circle's 24px cell: a faint ring, the + inside, the
/// ring's color on hover. It opens Settings at Providers.
class AddProviderButton extends StatelessWidget {
  const AddProviderButton({super.key, required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      container: true,
      button: true,
      label: addProviderLabel,
      excludeSemantics: true,
      child: HoverCard(
        card: (context) => const Text(addProviderLabel),
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => SizedBox.square(
            dimension: 24,
            child: Center(
              child: AnimatedContainer(
                duration: Motion.fast,
                width: 20,
                height: 20,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: hovered ? palette.hover : null,
                  border: Border.all(
                    color: hovered ? palette.textMuted : palette.hairline,
                    width: 1.5,
                  ),
                ),
                child: ExtensionGlyph(
                  icon: 'plus',
                  name: '+',
                  size: 12,
                  color: hovered ? palette.text : palette.textMuted,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// One account's 20px circle in a 24px cell, its card shown on hover.
class UsageCircle extends StatelessWidget {
  const UsageCircle({super.key, required this.meter});

  final UsageMeter meter;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: meter.name,
      child: HoverCard(
        interactive: true,
        // On iPhone, a touch and hold opens it.
        holdToShow: true,
        card: (context) => UsageCard(meter: meter, now: DateTime.now()),
        child: UsageRing(meter: meter),
      ),
    );
  }
}

/// An account's ring alone: [size] across in a [cell] a little larger,
/// with the reset cue at the cell's corner and, when it's the one of its
/// provider's accounts requests go to now, a dot under it, as the Dock
/// marks an app that's running. The circle's, and a phone's Usage page's
/// rows'.
class UsageRing extends StatelessWidget {
  const UsageRing({
    super.key,
    required this.meter,
    this.size = 20,
    this.cell = 24,
  });

  final UsageMeter meter;
  final double size;
  final double cell;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final provider = meter.provider;
    final usage = meter.usage;
    final (mark, used) = usage == null && meter.credit != null
        ? creditMark(meter.credit!)
        : usageMark(usage);
    final ring = SizedBox.square(
      dimension: cell,
      child: ResetCue(
        shown: resetCue(meter),
        child: Center(
          child: Opacity(
            // Numbers from before a failed read are shown, but faded.
            opacity: usage != null && usage.error.isNotEmpty ? 0.5 : 1,
            child: CustomPaint(
              size: Size.square(size),
              painter: _CirclePainter(
                color: providerColor(provider.id, palette),
                mark: mark,
                used: used,
                logo: providerMark(provider.id),
              ),
            ),
          ),
        ),
      ),
    );
    if (!meter.inUseOfMany) return ring;
    // Just under the ring, in the cell's margin and a little past it.
    final gap = (cell - size) / 2;
    return Stack(
      clipBehavior: Clip.none,
      children: [
        ring,
        Positioned(
          left: 0,
          right: 0,
          bottom: gap - inUseDot - 1,
          child: Center(
            child: SizedBox.square(
              dimension: inUseDot,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: providerColor(provider.id, palette),
                  shape: BoxShape.circle,
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// The dot under the ring of the account requests go to now.
const inUseDot = 4.0;

/// How a circle for [usage] is drawn, and for a meter how full it is (0 to
/// 1): the share of its weekly limit used. Null usage has no limits.
(UsageMark, double) usageMark(ProviderUsage? usage) {
  if (usage == null) return (UsageMark.noLimits, 0);
  final weekly = _window(usage, headlineWindow);
  if (weekly == null) return (UsageMark.unknown, 0);
  return (UsageMark.meter, weekly.usedPercent.clamp(0, 100) / 100);
}

/// How a circle for an API key's [credit] is drawn: a ring filling as its
/// spending limit is used, when it has one; else the mark alone.
(UsageMark, double) creditMark(AccountCredit credit) {
  if (!credit.limited || credit.limit <= 0) return (UsageMark.noLimits, 0);
  return (UsageMark.meter, (1 - credit.left / credit.limit).clamp(0.0, 1.0));
}

/// Dollars as the card says them: "\$10", "\$0.80".
String formatDollars(double usd) => usd == usd.roundToDouble() && usd.abs() >= 1
    ? '\$${usd.round()}'
    : '\$${usd.toStringAsFixed(2)}';

/// What an API key has to spend, in a line: "\$3.20 left of \$10 a day ·
/// \$0.80 today", or with no limit on the key, "\$0.80 today · no spending
/// limit on this key".
String creditLine(AccountCredit credit) {
  final today = '${formatDollars(credit.spentToday)} today';
  if (!credit.limited) return '$today · no spending limit on this key';
  final per = switch (credit.limitReset) {
    'daily' => ' a day',
    'weekly' => ' a week',
    'monthly' => ' a month',
    _ => '',
  };
  final left = credit.left < 0 ? 0.0 : credit.left;
  return '${formatDollars(left)} left of ${formatDollars(credit.limit)}$per · '
      '$today';
}

enum UsageMark {
  /// A ring that fills as the limit is used.
  meter,

  /// Usage is read but there's no number for the weekly limit yet: a dashed
  /// ring.
  unknown,

  /// No usage limits (an API key): a plain dot.
  noLimits,
}

class _CirclePainter extends CustomPainter {
  const _CirclePainter({
    required this.color,
    required this.mark,
    required this.used,
    required this.logo,
  });

  final Color color;
  final UsageMark mark;

  /// 0 to 1.
  final double used;

  /// The provider's mark, drawn in the middle; null draws a dot there.
  final Path? logo;

  @override
  void paint(Canvas canvas, Size size) {
    final center = size.center(Offset.zero);
    final radius = size.shortestSide / 2 - 1;
    final ring = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2
      ..strokeCap = StrokeCap.round;
    final rect = Rect.fromCircle(center: center, radius: radius);
    switch (mark) {
      case UsageMark.noLimits:
        // No ring: nothing to measure.
        break;
      case UsageMark.unknown:
        const dashes = 10;
        for (var i = 0; i < dashes; i++) {
          canvas.drawArc(
            rect,
            i * 2 * math.pi / dashes,
            math.pi / dashes,
            false,
            ring..strokeCap = StrokeCap.butt,
          );
        }
      case UsageMark.meter:
        // The track, then the share used, clockwise from twelve.
        canvas.drawCircle(
          center,
          radius,
          Paint()
            ..color = color.withValues(alpha: 0.22)
            ..style = PaintingStyle.stroke
            ..strokeWidth = 2,
        );
        if (used > 0) {
          canvas.drawArc(
            rect,
            -math.pi / 2,
            2 * math.pi * used,
            false,
            used >= 1 ? (ring..strokeCap = StrokeCap.butt) : ring,
          );
        }
    }
    final logo = this.logo;
    if (logo == null) {
      canvas.drawCircle(center, 3, Paint()..color = color);
      return;
    }
    // The mark fills the middle, clear of the ring.
    final bounds = logo.getBounds();
    final box = size.shortestSide * (mark == UsageMark.noLimits ? 0.8 : 0.5);
    final scale = box / math.max(bounds.width, bounds.height);
    canvas
      ..save()
      ..translate(center.dx, center.dy)
      ..scale(scale)
      ..translate(-bounds.center.dx, -bounds.center.dy)
      ..drawPath(logo, Paint()..color = color)
      ..restore();
  }

  @override
  bool shouldRepaint(_CirclePainter old) =>
      old.color != color ||
      old.mark != mark ||
      old.used != used ||
      old.logo != logo;
}

UsageWindow? _window(ProviderUsage usage, String id) {
  for (final window in usage.windows) {
    if (window.id == id) return window;
  }
  return null;
}

/// What a circle says on hover: the provider, who the account is and its
/// plan (or how it's signed in), whether requests go to it, each limit's
/// share used and when it resets, and when that was read.
class UsageCard extends StatelessWidget {
  const UsageCard({super.key, required this.meter, required this.now});

  final UsageMeter meter;

  /// The time the ages and resets are counted from.
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final provider = meter.provider;
    final account = meter.account;
    final usage = meter.usage;
    // The weekly limit first, then the rest in the provider's order.
    final windows = usage == null
        ? <UsageWindow>[]
        : [
            ...usage.windows.where((w) => w.id == headlineWindow),
            ...usage.windows.where((w) => w.id != headlineWindow),
          ];
    final read = usage != null && usage.updated > 0;
    final state = account == null ? null : accountState(provider, account, now);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Dot(color: providerColor(provider.id, palette)),
            const SizedBox(width: Space.s),
            Text(
              provider.name,
              style: TextStyles.label.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
        if (meter.who.isNotEmpty) Text(meter.who, style: caption),
        // The plan, as the provider names it, once it's known.
        if (meter.plan.isNotEmpty)
          Text(meter.plan, style: caption)
        else if (account == null)
          Text('Signed in with ${provider.signedInWith}', style: caption),
        if (state != null && state.text.isNotEmpty)
          Text(
            state.text,
            style: TextStyles.caption.copyWith(
              color: state.problem ? palette.danger : palette.textMuted,
            ),
          ),
        // An API key's credit, where its provider says it (#343 U8).
        if (meter.credit case final credit? when usage == null) ...[
          if (credit.readMs > 0)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                creditLine(credit),
                style: caption.copyWith(
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ),
          if (credit.error.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                "Couldn't check credit: ${credit.error}",
                style: TextStyles.caption.copyWith(color: palette.danger),
              ),
            )
          else if (credit.readMs == 0)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text('Credit not checked yet.', style: caption),
            ),
          if (credit.readMs > 0)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                '${credit.error.isEmpty ? 'Checked' : 'Last checked'} '
                '${formatAge(credit.readMs.toInt(), now)}',
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ),
        ] else if (usage == null)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text('No usage limits to show.', style: caption),
          ),
        if (windows.isNotEmpty) ...[
          const SizedBox(height: Space.s),
          Table(
            defaultColumnWidth: const IntrinsicColumnWidth(),
            children: [
              for (final window in windows)
                TableRow(
                  children: [
                    Padding(
                      padding: const EdgeInsets.only(right: Space.m),
                      child: Text(window.label),
                    ),
                    Padding(
                      padding: const EdgeInsets.only(right: Space.m),
                      child: Text(
                        formatPercent(window.usedPercent),
                        textAlign: TextAlign.right,
                        style: const TextStyle(
                          fontFeatures: [FontFeature.tabularFigures()],
                        ),
                      ),
                    ),
                    Text(
                      formatReset(window.resetsAt.toInt(), now),
                      style: caption,
                    ),
                  ],
                ),
            ],
          ),
        ],
        if (usage != null && !read && usage.error.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text('Usage not checked yet.', style: caption),
          ),
        if (usage != null && read && windows.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text('No limits reported.', style: caption),
          ),
        if (usage != null && usage.error.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text(
              "Couldn't check usage: ${usage.error}",
              style: TextStyles.caption.copyWith(color: palette.danger),
            ),
          ),
        if (read)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text(
              '${usage.error.isEmpty ? 'Checked' : 'Last checked'} '
              '${formatAge(usage.updated.toInt(), now)}',
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
        UsageResets(meter: meter, now: now),
      ],
    );
  }
}

/// What [account] is doing now: requests go to it ("In use"), it reached a
/// usage limit and is skipped until it resets ("Limit reached, back at
/// 14:00"), an API key ran out of credit, its sign-in expired ("Sign-in
/// expired"), or it waits its turn ("Signed in"). [problem] marks a state the
/// person may need to act on.
({String text, bool problem}) accountState(
  Provider provider,
  Account account,
  DateTime now,
) {
  final until = account.exhaustedUntil.toInt();
  if (until > now.millisecondsSinceEpoch) {
    // Fable has a pool of its own: its requests may still go to it.
    return (
      text: [
        'Limit reached, back at ${formatUntil(until, now)}',
        if (account.fableInUse) 'In use for Fable',
      ].join(' · '),
      problem: false,
    );
  }
  if (account.outOfCredit) {
    return (
      text: 'Out of credit. Add credit at ${provider.name}.',
      problem: true,
    );
  }
  // Its key can't run paid models: no credit yet, or its spending limit is
  // used up (Claktory's sentence, #343 U8).
  if (account.hasCredit() && account.credit.warning.isNotEmpty) {
    return (text: account.credit.warning, problem: true);
  }
  // The provider refused to refresh it: skipped until signed in to again.
  if (account.signInAgain) return (text: 'Sign-in expired', problem: true);
  return (text: accountUseLine(account, now), problem: false);
}

/// Under a provider signed in to whose models no agent or chat runs yet
/// (Provider.agents 0, #343 U1): signing in doesn't move them.
String noAgentsOnProvider(Provider provider) =>
    'No agents or chats use ${provider.name} models yet.';

/// Whether requests go to [account] now: an agent or chat runs on its
/// provider's models and it's the first with room, in its normal pool
/// (#343 U1: only while one does) or Fable's own (U10).
bool accountInUse(Account account) => account.inUse || account.fableInUse;

/// Where [account] stands when nothing's wrong with it: "In use" while
/// requests go to it ("In use for Fable" for Fable's alone), else "Signed
/// in"; with Fable's own weekly limit
/// reached on it, that too ("In use · Fable: limit reached, back at Fri
/// 18:00", #343 U10), since Fable's requests then go to another account.
String accountUseLine(Account account, DateTime now) {
  final fableUntil = account.fableExhaustedUntil.toInt();
  return [
    account.inUse
        ? 'In use'
        : account.fableInUse
        ? 'In use for Fable'
        : 'Signed in',
    if (fableUntil > now.millisecondsSinceEpoch)
      'Fable: limit reached, back at ${formatUntil(fableUntil, now)}',
  ].join(' · ');
}

/// A time to come, from [ms] since 1970: "14:00" today, "Tue 14:00" within
/// the week, "12 Dec 14:00" after.
String formatUntil(int ms, DateTime now) {
  final at = DateTime.fromMillisecondsSinceEpoch(ms);
  final time =
      '${at.hour.toString().padLeft(2, '0')}:'
      '${at.minute.toString().padLeft(2, '0')}';
  final today = DateTime(now.year, now.month, now.day);
  final day = DateTime(at.year, at.month, at.day).difference(today).inDays;
  if (day <= 0) return time;
  if (day < 7) {
    const weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
    return '${weekdays[at.weekday - 1]} $time';
  }
  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  return '${at.day} ${months[at.month - 1]} $time';
}

/// "42%", or "0.5%" below 1 so a little use doesn't look like none.
String formatPercent(double percent) {
  final value = percent.clamp(0, 100);
  if (value > 0 && value < 1) return '${value.toStringAsFixed(1)}%';
  return '${value.round()}%';
}

/// How long ago [ms] (since 1970) was: "just now", "5 min ago", "3 h ago",
/// "2 days ago".
String formatAge(int ms, DateTime now) {
  final age = now.difference(DateTime.fromMillisecondsSinceEpoch(ms));
  if (age.inMinutes < 1) return 'just now';
  if (age.inHours < 1) return '${age.inMinutes} min ago';
  if (age.inDays < 1) return '${age.inHours} h ago';
  return age.inDays == 1 ? '1 day ago' : '${age.inDays} days ago';
}

/// When a limit resets, from [ms] since 1970 (0 when unknown): "resets in
/// 3 h 20 min", or with the day and time when it's a day or more away,
/// "resets in 2 days 20 h, Tue 14:00" (whole days alone would round
/// 2 days 20 h down to "2 days").
String formatReset(int ms, DateTime now) {
  if (ms <= 0) return 'reset time unknown';
  final at = DateTime.fromMillisecondsSinceEpoch(ms);
  final left = at.difference(now);
  if (left.isNegative) return 'has reset since';
  if (left.inMinutes < 1) return 'resets in under a minute';
  if (left.inHours < 1) return 'resets in ${left.inMinutes} min';
  if (left.inDays < 1) {
    final minutes = left.inMinutes % 60;
    return 'resets in ${left.inHours} h'
        '${minutes == 0 ? '' : ' $minutes min'}';
  }
  const weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  final hours = left.inHours % 24;
  final days =
      '${left.inDays == 1 ? '1 day' : '${left.inDays} days'}'
      '${hours == 0 ? '' : ' $hours h'}';
  final time =
      '${at.hour.toString().padLeft(2, '0')}:'
      '${at.minute.toString().padLeft(2, '0')}';
  // The weekday and time stay together on a narrow card (Abbacchio).
  return 'resets in $days, ${weekdays[at.weekday - 1]}\u00A0$time';
}

// Provider marks as SVG path data, filled. Claude's spark and OpenRouter's
// mark are from Simple Icons (24-unit grid); OpenAI's blossom is its
// official mark (40-unit grid). Each is fitted to its circle when drawn.

/// Claude's spark, for Anthropic.
const _claudeMark =
    'm4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486'
    '-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.'
    '3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.25'
    '5h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-'
    '1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607'
    '.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0'
    '182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c'
    '-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419'
    '.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.'
    '255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.'
    '9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.902'
    '1-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.'
    '85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 '
    '1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.'
    '2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614'
    '-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2'
    '247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-'
    '1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.12'
    '75.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.12'
    '75v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-'
    '.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-'
    '.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.'
    '3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.'
    '1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 '
    '2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164'
    '-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z';

/// OpenAI's blossom, for OpenAI and OpenAI Codex.
const _openAiMark =
    'M32.8377 17.282C33.2127 16.25 33.3072 15.218 33.2127 14.1875C33.1197 1'
    '3.1571 32.7447 12.1251 32.2752 11.1876C31.4322 9.78209 30.2127 8.6571 '
    '28.8072 8.0001C27.3072 7.34461 25.7127 7.15711 24.1197 7.53211C23.3698'
    ' 6.78212 22.5253 6.12512 21.5878 5.65713C20.6503 5.18913 19.5253 5.000'
    '13 18.4948 5.00013C16.8851 4.99074 15.3125 5.48246 13.9948 6.40712C12.'
    '6824 7.34311 11.7449 8.6571 11.2754 10.1571C10.1504 10.4376 9.21289 10'
    '.9071 8.27539 11.4696C7.4324 12.1251 6.77541 12.9696 6.21291 13.8126C5'
    '.36992 15.2195 5.08792 16.8125 5.27542 18.407C5.46399 19.9968 6.11605 '
    '21.496 7.1504 22.718C6.79608 23.7086 6.66795 24.7659 6.77541 25.8124C6'
    '.86991 26.8444 7.2449 27.8749 7.7129 28.8124C8.55739 30.2194 9.77538 3'
    '1.3444 11.1824 31.9999C12.6824 32.6569 14.2753 32.8444 15.8698 32.4694'
    'C16.6198 33.2194 17.4628 33.8749 18.4003 34.3444C19.3378 34.8139 20.46'
    '28 34.9999 21.4948 34.9999C23.1043 35.0097 24.6769 34.5185 25.9947 33.'
    '5944C27.3072 32.6569 28.2447 31.3444 28.7127 29.8444C29.7719 29.6432 3'
    '0.7682 29.1934 31.6197 28.5319C32.4627 27.8749 33.2127 27.1249 33.6822'
    ' 26.1874C34.5251 24.7819 34.8071 23.1875 34.6196 21.5945C34.4322 20 33'
    '.8697 18.5015 32.8377 17.282ZM21.5878 33.0304C20.0878 33.0304 18.9628 '
    '32.5609 17.9323 31.7179C17.9323 31.7179 18.0253 31.6234 18.1198 31.623'
    '4L24.1197 28.1554C24.2862 28.0803 24.4196 27.9469 24.4947 27.7804C24.5'
    '698 27.636 24.6021 27.4731 24.5877 27.3109V18.875L27.1197 20.375V27.31'
    '24C27.1455 28.0547 27.0215 28.7945 26.755 29.4878C26.4885 30.181 26.08'
    '5 30.8134 25.5687 31.3473C25.0523 31.8811 24.4337 32.3054 23.7497 32.5'
    '949C23.0658 32.8843 22.3305 33.0314 21.5878 33.0304ZM9.49488 27.8749C8'
    '.83789 26.7499 8.55739 25.4374 8.83789 24.125C8.83789 24.125 8.93239 2'
    '4.2195 9.02539 24.2195L15.0253 27.6874C15.1693 27.7638 15.3325 27.7966'
    ' 15.4948 27.7819C15.6823 27.7819 15.8698 27.7819 15.9628 27.6874L23.27'
    '53 23.4695V26.3749L17.1823 29.9374C16.5506 30.3042 15.8527 30.5427 15.'
    '1287 30.6393C14.4046 30.7358 13.6686 30.6884 12.9629 30.4999C11.4629 3'
    '0.1249 10.2449 29.1874 9.49488 27.8749ZM7.9004 14.8445C8.56239 13.7234'
    ' 9.58826 12.8627 10.8074 12.4056V19.532C10.8074 19.718 10.8074 19.907 '
    '10.9004 20C10.9755 20.1665 11.1089 20.2998 11.2754 20.375L18.5878 24.5'
    '944L16.0573 26.0944L10.0574 22.625C9.41842 22.2639 8.85742 21.7797 8.4'
    '0684 21.2004C7.95627 20.6211 7.62506 19.9582 7.4324 19.25C7.05741 17.8'
    '445 7.1504 16.157 7.9004 14.8445ZM28.6197 19.625L21.3073 15.407L23.837'
    '7 13.9071L29.8377 17.375C30.7752 17.9375 31.5252 18.6875 31.9947 19.62'
    '5C32.4642 20.5625 32.7447 21.5945 32.6502 22.7195C32.5603 23.7755 32.1'
    '699 24.7837 31.5252 25.6249C30.8697 26.4694 30.0252 27.1249 28.9947 27'
    '.4999V20.375C28.9947 20.1875 28.9947 20 28.9002 19.907C28.9002 19.907 '
    '28.8072 19.718 28.6197 19.625ZM31.1502 15.875C31.1502 15.875 31.0572 1'
    '5.782 30.9627 15.782L24.9627 12.3126C24.7752 12.2196 24.6822 12.2196 2'
    '4.4947 12.2196C24.3072 12.2196 24.1197 12.2196 24.0252 12.3126L16.7128'
    ' 16.532V13.6251L22.8073 10.0626C23.7448 9.50009 24.7752 9.31259 25.900'
    '2 9.31259C26.9322 9.31259 27.9627 9.68759 28.9002 10.3446C29.7447 11.0'
    '001 30.4947 11.8446 30.8697 12.7821C31.2447 13.7196 31.3377 14.8445 31'
    '.1502 15.875ZM15.4003 21.125L12.8699 19.625V12.5946C12.8699 11.5626 13'
    '.1503 10.4376 13.7128 9.59459C14.2753 8.6571 15.1198 8.0001 16.0573 7.'
    '53211C17.0127 7.05249 18.0956 6.88812 19.1503 7.06261C20.1823 7.15711 '
    '21.2128 7.62511 22.0573 8.2821C22.0573 8.2821 21.9628 8.3751 21.8698 8'
    '.3751L15.8698 11.8446C15.7033 11.9197 15.57 12.0531 15.4948 12.2196C15'
    '.4003 12.4071 15.4003 12.5001 15.4003 12.6876V21.125ZM16.7128 18.125L1'
    '9.9948 16.25L23.2753 18.125V21.875L19.9948 23.75L16.7128 21.875V18.125'
    'Z';

/// OpenRouter's mark.
const _openRouterMark =
    'M16.778 1.844v1.919q-.569-.026-1.138-.032-.708-.008-1.415.037c-1.93.12'
    '6-4.023.728-6.149 2.237-2.911 2.066-2.731 1.95-4.14 2.75-.396.223-1.34'
    '2.574-2.185.798-.841.225-1.753.333-1.751.333v4.229s.768.108 1.61.333c.'
    '842.224 1.789.575 2.185.799 1.41.798 1.228.683 4.14 2.75 2.126 1.509 4'
    '.22 2.11 6.148 2.236.88.058 1.716.041 2.555.005v1.918l7.222-4.168-7.22'
    '2-4.17v2.176c-.86.038-1.611.065-2.278.021-1.364-.09-2.417-.357-3.979-1'
    '.465-2.244-1.593-2.866-2.027-3.68-2.508.889-.518 1.449-.906 3.822-2.59'
    ' 1.56-1.109 2.614-1.377 3.978-1.466.667-.044 1.418-.017 2.278.02v2.176'
    'L24 6.014Z';

final _marks = <String, Path>{};

/// [providerId]'s product mark, or null for a provider without one here.
Path? providerMark(String providerId) {
  final data = switch (providerId) {
    'anthropic' => _claudeMark,
    'openai' || 'openai-codex' => _openAiMark,
    'openrouter' => _openRouterMark,
    _ => null,
  };
  if (data == null) return null;
  return _marks.putIfAbsent(providerId, () => svgPath(data));
}
