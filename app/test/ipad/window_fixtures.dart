import 'dart:ui' show Size;

import 'package:factory_app/ipad/columns.dart';

/// iPadOS's text sizes as factors of the default body (17 pt), the ones
/// the matrix checks: xxxLarge, the largest without Larger Accessibility
/// Sizes, then AX1, AX3 and AX5.
abstract final class TextFactor {
  static const standard = 1.0;
  static const xxxLarge = 23 / 17;
  static const ax1 = 28 / 17;
  static const ax3 = 40 / 17;
  static const ax5 = 53 / 17;
}

/// One window the iPad workbench must lay out: its safe size in points
/// (the view less its safe area; the keyboard never shrinks it), the text
/// size, the idiom and the composition it should get.
///
/// The same table drives the widget tests now and the Simulator
/// screenshots in phase 1: [simulator] names a device to run it on, and
/// [measured] says whether the size was read from a real run (false: a
/// device's documented full-screen size, or an estimate of a Split View,
/// Slide Over or Stage Manager width still to be measured).
class WindowFixture {
  const WindowFixture(
    this.name,
    this.size, {
    required this.tier,
    this.idiom = Idiom.pad,
    this.textFactor = TextFactor.standard,
    this.keyboard = 0,
    this.simulator,
    this.measured = false,
    this.note = '',
  });

  /// A screenshot's name too: `<name>.png`.
  final String name;
  final Size size;
  final WorkbenchTier tier;
  final Idiom idiom;
  final double textFactor;

  /// The docked keyboard's height over the window, in points; 0 for none
  /// or a floating one. The composition must not change with it.
  final double keyboard;

  final String? simulator;
  final bool measured;
  final String note;

  @override
  String toString() => name;
}

const _mini = 'iPad mini (A17 Pro)';
const _air11 = 'iPad Air 11-inch (M3)';
const _air13 = 'iPad Air 13-inch (M3)';
const _pro11 = 'iPad Pro 11-inch (M5)';
const _pro13 = 'iPad Pro 13-inch (M5)';

/// ipad.md §7's matrix: 320/375 compact; 600/744/820/834 portrait;
/// 1024/1180/1366 wide; each tier's edges; larger text; a keyboard; and the
/// iPhone, which must keep its compact routes on its side too.
const windowFixtures = <WindowFixture>[
  // Compact: the phone's routes, every capability still in reach.
  WindowFixture(
    'slide-over-320',
    Size(320, 834),
    tier: WorkbenchTier.compact,
    simulator: _pro11,
    note: 'Slide Over, or a third of an 11-inch on its side (to measure)',
  ),
  WindowFixture(
    'split-third-375',
    Size(375, 1024),
    tier: WorkbenchTier.compact,
    simulator: _air13,
    note: 'a third of a 13-inch on its side (to measure)',
  ),
  WindowFixture(
    'compact-edge-599',
    Size(599, 900),
    tier: WorkbenchTier.compact,
  ),
  WindowFixture(
    'short-stage-1024x420',
    Size(1024, 420),
    tier: WorkbenchTier.compact,
    note: 'a Stage Manager window too short for header, content and composer',
  ),
  // Focused: one pane at a time, with a Chat/Document switch.
  WindowFixture(
    'focused-edge-600',
    Size(600, 800),
    tier: WorkbenchTier.focused,
  ),
  WindowFixture(
    'split-half-678',
    Size(678, 1024),
    tier: WorkbenchTier.focused,
    simulator: _air13,
    note: 'half of a 13-inch on its side (to measure)',
  ),
  WindowFixture(
    'mini-portrait-744',
    Size(744, 1081),
    tier: WorkbenchTier.focused,
    simulator: _mini,
    measured: true,
    note:
        'full screen: the view 744×1133, safe top 32 and bottom 20 '
        '(Gelato, iPadOS 27.2)',
  ),
  // A mini's window narrowed in Windowed Apps (Gelato, iPadOS 27.2): the
  // view ~1097 tall, its padding 32 top and 10 bottom at every width, so
  // 1055 safe. The idiom stays pad at every width.
  WindowFixture(
    'mini-window-679',
    Size(679, 1055),
    tier: WorkbenchTier.focused,
    simulator: _mini,
    measured: true,
    note: 'regular horizontal size class (it turns compact under 668)',
  ),
  WindowFixture(
    'mini-window-662',
    Size(662, 1055),
    tier: WorkbenchTier.focused,
    simulator: _mini,
    measured: true,
    note:
        'compact horizontal size class from here; still one pane with '
        'the toolbar (docs/ipad-workbench.md §4)',
  ),
  WindowFixture(
    'mini-window-625',
    Size(625, 1055),
    tier: WorkbenchTier.focused,
    simulator: _mini,
    measured: true,
  ),
  WindowFixture(
    'mini-window-554',
    Size(554, 1055),
    tier: WorkbenchTier.compact,
    simulator: _mini,
    measured: true,
  ),
  WindowFixture(
    'mini-window-375',
    Size(375, 1055),
    tier: WorkbenchTier.compact,
    simulator: _mini,
    measured: true,
    note: 'the narrowest a window gets (a 320 target stopped here)',
  ),
  // A Pro 13's windows (Gelato, iPadOS 27.2): upright ~1340 tall, on its
  // side 1032, with insets of 32 top and 10 bottom upright, 32 and 20 on
  // its side. The size class turns compact under 671 upright, under 669
  // on its side.
  WindowFixture(
    'pro13-window-portrait-671',
    Size(671, 1298),
    tier: WorkbenchTier.focused,
    simulator: _pro13,
    measured: true,
    note: 'regular horizontal size class; compact at 666 and narrower',
  ),
  WindowFixture(
    'pro13-window-landscape-667',
    Size(667, 980),
    tier: WorkbenchTier.focused,
    simulator: _pro13,
    measured: true,
    note: 'compact horizontal size class; regular at 669 and wider',
  ),
  WindowFixture(
    'pro13-window-375',
    Size(375, 1298),
    tier: WorkbenchTier.compact,
    simulator: _pro13,
    measured: true,
    note: 'the narrowest a window gets',
  ),
  WindowFixture(
    'air11-portrait-820',
    Size(820, 1180),
    tier: WorkbenchTier.focused,
    simulator: _air11,
  ),
  WindowFixture(
    'pro11-portrait-834',
    Size(834, 1210),
    tier: WorkbenchTier.focused,
    simulator: _pro11,
  ),
  WindowFixture(
    'focused-edge-839',
    Size(839, 1000),
    tier: WorkbenchTier.focused,
  ),
  // Paired: the rail and two columns; the sidebar opens over them.
  WindowFixture('paired-edge-840', Size(840, 1000), tier: WorkbenchTier.paired),
  WindowFixture(
    'split-two-thirds-981',
    Size(981, 1024),
    tier: WorkbenchTier.paired,
    simulator: _air13,
    note: 'two thirds of a 13-inch on its side (to measure)',
  ),
  WindowFixture(
    'air13-portrait-1024',
    Size(1024, 1366),
    tier: WorkbenchTier.paired,
    simulator: _air13,
  ),
  WindowFixture(
    'pro13-portrait-1032',
    Size(1032, 1376),
    tier: WorkbenchTier.paired,
    simulator: _pro13,
  ),
  WindowFixture(
    'mini-landscape-1133',
    Size(1133, 744),
    tier: WorkbenchTier.paired,
    simulator: _mini,
  ),
  WindowFixture(
    'paired-edge-1179',
    Size(1179, 800),
    tier: WorkbenchTier.paired,
  ),
  // Full: the rail, the sidebar pinned and two columns.
  WindowFixture(
    'air11-landscape-1180',
    Size(1180, 820),
    tier: WorkbenchTier.full,
    simulator: _air11,
  ),
  WindowFixture(
    'pro11-landscape-1210',
    Size(1210, 834),
    tier: WorkbenchTier.full,
    simulator: _pro11,
  ),
  WindowFixture(
    'air13-landscape-1366',
    Size(1366, 1024),
    tier: WorkbenchTier.full,
    simulator: _air13,
  ),
  WindowFixture(
    'pro13-landscape-1376',
    Size(1376, 1032),
    tier: WorkbenchTier.full,
    simulator: _pro13,
  ),
  // A docked keyboard changes nothing: the editor scrolls into view.
  WindowFixture(
    'pro11-landscape-keyboard',
    Size(1210, 834),
    tier: WorkbenchTier.full,
    keyboard: 400,
    simulator: _pro11,
    note: 'docked software keyboard; the height is an estimate',
  ),
  // Larger text collapses sooner.
  WindowFixture(
    'air11-landscape-xxxl',
    Size(1180, 820),
    tier: WorkbenchTier.paired,
    textFactor: TextFactor.xxxLarge,
    simulator: _air11,
  ),
  WindowFixture(
    'pro13-landscape-xxxl',
    Size(1376, 1032),
    tier: WorkbenchTier.paired,
    textFactor: TextFactor.xxxLarge,
    simulator: _pro13,
  ),
  WindowFixture(
    'pro13-landscape-ax1',
    Size(1376, 1032),
    tier: WorkbenchTier.paired,
    textFactor: TextFactor.ax1,
    simulator: _pro13,
  ),
  WindowFixture(
    'pro13-landscape-ax3',
    Size(1376, 1032),
    tier: WorkbenchTier.paired,
    textFactor: TextFactor.ax3,
    simulator: _pro13,
  ),
  WindowFixture(
    'air13-portrait-ax3',
    Size(1024, 1366),
    tier: WorkbenchTier.focused,
    textFactor: TextFactor.ax3,
    simulator: _air13,
  ),
  WindowFixture(
    'pro13-landscape-ax5',
    Size(1376, 1032),
    tier: WorkbenchTier.focused,
    textFactor: TextFactor.ax5,
    simulator: _pro13,
  ),
  WindowFixture(
    'pro11-portrait-ax5',
    Size(834, 1210),
    tier: WorkbenchTier.focused,
    textFactor: TextFactor.ax5,
    simulator: _pro11,
  ),
  // An iPhone keeps its routes, upright and on its side.
  WindowFixture(
    'iphone-portrait',
    Size(402, 874),
    tier: WorkbenchTier.compact,
    idiom: Idiom.phone,
    simulator: 'iPhone 16 Pro',
  ),
  WindowFixture(
    'iphone-landscape',
    Size(874, 402),
    tier: WorkbenchTier.compact,
    idiom: Idiom.phone,
    simulator: 'iPhone 16 Pro',
  ),
  WindowFixture(
    'iphone-max-landscape',
    Size(956, 440),
    tier: WorkbenchTier.compact,
    idiom: Idiom.phone,
    simulator: 'iPhone 16 Pro Max',
  ),
];
