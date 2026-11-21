import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoAdaptiveTextSelectionToolbar,
        CupertinoTextMagnifier,
        cupertinoTextSelectionHandleControls;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'platform_policy.dart';

/// What a text field is for, so the phone brings the right keyboard and
/// offers the right autofill: an email's @ keyboard, a code from Messages,
/// a name capitalized. On a Mac only [secret] changes anything (it hides
/// what's typed).
enum InputKind {
  /// One line of words: a name of something, a search.
  text,

  /// Several lines, Return starting a new one.
  multiline,

  /// A message to an agent: several lines, sentences capitalized, Return
  /// starting a new line (Send sends).
  message,

  /// An email address.
  email,

  /// A code sent by email or text: digits, offered by the keyboard as it
  /// arrives.
  oneTimeCode,

  /// A person's name.
  name,

  /// A web address.
  url,

  /// A number.
  number,

  /// A password, key or token: hidden, never suggested or corrected.
  secret,
}

/// How [InputKind] sets up the keyboard.
class InputSetup {
  const InputSetup({
    required this.keyboardType,
    required this.textInputAction,
    this.autofillHints = const [],
    this.textCapitalization = TextCapitalization.none,
    this.autocorrect = true,
    this.enableSuggestions = true,
    this.obscureText = false,
    this.inputFormatters = const [],
  });

  final TextInputType keyboardType;
  final TextInputAction textInputAction;
  final List<String> autofillHints;
  final TextCapitalization textCapitalization;
  final bool autocorrect;
  final bool enableSuggestions;
  final bool obscureText;
  final List<TextInputFormatter> inputFormatters;
}

extension InputKindSetup on InputKind {
  InputSetup get setup => switch (this) {
    InputKind.text => const InputSetup(
      keyboardType: TextInputType.text,
      textInputAction: TextInputAction.done,
      textCapitalization: TextCapitalization.sentences,
    ),
    InputKind.multiline => const InputSetup(
      keyboardType: TextInputType.multiline,
      textInputAction: TextInputAction.newline,
      textCapitalization: TextCapitalization.sentences,
    ),
    InputKind.message => const InputSetup(
      keyboardType: TextInputType.multiline,
      textInputAction: TextInputAction.newline,
      textCapitalization: TextCapitalization.sentences,
    ),
    InputKind.email => const InputSetup(
      keyboardType: TextInputType.emailAddress,
      textInputAction: TextInputAction.done,
      autofillHints: [AutofillHints.email],
      autocorrect: false,
      enableSuggestions: false,
    ),
    InputKind.oneTimeCode => InputSetup(
      keyboardType: TextInputType.number,
      textInputAction: TextInputAction.done,
      autofillHints: const [AutofillHints.oneTimeCode],
      autocorrect: false,
      enableSuggestions: false,
      inputFormatters: [FilteringTextInputFormatter.digitsOnly],
    ),
    InputKind.name => const InputSetup(
      keyboardType: TextInputType.name,
      textInputAction: TextInputAction.done,
      autofillHints: [AutofillHints.givenName],
      textCapitalization: TextCapitalization.words,
      autocorrect: false,
    ),
    InputKind.url => const InputSetup(
      keyboardType: TextInputType.url,
      textInputAction: TextInputAction.done,
      autofillHints: [AutofillHints.url],
      autocorrect: false,
      enableSuggestions: false,
    ),
    InputKind.number => const InputSetup(
      keyboardType: TextInputType.number,
      textInputAction: TextInputAction.done,
      autocorrect: false,
      enableSuggestions: false,
    ),
    InputKind.secret => const InputSetup(
      keyboardType: TextInputType.visiblePassword,
      textInputAction: TextInputAction.done,
      autocorrect: false,
      enableSuggestions: false,
      obscureText: true,
    ),
  };
}

/// Whether text is selected the phone's own way here: handles to drag, the
/// magnifier, and the system's edit menu (on an iPhone, or Android).
bool get nativeSelectionHere => PhonePlatformPolicy.current.nativeSelection;

/// More items for an edit menu, beside Copy and Select All: Comment,
/// Suggest, Share. Each needs a label and an onPressed.
typedef ExtraEditItems<S> = List<ContextMenuButtonItem> Function(S state);

/// How an [EditableText] selects here, as the platform's
/// [PhonePlatformPolicy] says: on an iPhone [iPhoneTextSelection]; on a
/// desktop all null, the mouse's selection with no handles and no menu.
({
  TextSelectionControls? controls,
  EditableTextContextMenuBuilder? contextMenuBuilder,
  TextMagnifierConfiguration magnifier,
})
nativeTextSelection(
  BuildContext context, {
  ExtraEditItems<EditableTextState>? extra,
}) => PhonePlatformPolicy.current.selection(context, extra: extra);

/// How an [EditableText] selects on an iPhone: Cupertino handles, the
/// magnifier, and iOS's own edit menu (Copy, Paste, Select All, Look Up…)
/// with [extra] items after them.
({
  TextSelectionControls? controls,
  EditableTextContextMenuBuilder? contextMenuBuilder,
  TextMagnifierConfiguration magnifier,
})
iPhoneTextSelection({ExtraEditItems<EditableTextState>? extra}) {
  return (
    controls: cupertinoTextSelectionHandleControls,
    contextMenuBuilder: (context, state) {
      final more = extra?.call(state) ?? const <ContextMenuButtonItem>[];
      if (SystemContextMenu.isSupportedByField(state)) {
        return SystemContextMenu.editableText(
          editableTextState: state,
          items: [
            ...SystemContextMenu.getDefaultItems(state),
            for (final item in more)
              if (item.label != null && item.onPressed != null)
                IOSSystemContextMenuItemCustom(
                  title: item.label!,
                  onPressed: item.onPressed!,
                ),
          ],
        );
      }
      return CupertinoAdaptiveTextSelectionToolbar.buttonItems(
        anchors: state.contextMenuAnchors,
        buttonItems: [...state.contextMenuButtonItems, ...more],
      );
    },
    magnifier: nativeMagnifier,
  );
}

/// The edit menu for text that can be selected but not edited (a
/// [SelectableRegion]: the transcript, a viewer), as the platform's
/// [PhonePlatformPolicy] says: on an iPhone [iPhoneSelectableMenu]. Null
/// on a desktop, which keeps its own.
SelectableRegionContextMenuBuilder? nativeSelectableMenu({
  ExtraEditItems<SelectableRegionState>? extra,
}) => PhonePlatformPolicy.current.selectableMenu(extra: extra);

/// The iPhone's edit menu for text that selects but doesn't edit: Copy and
/// Select All, then [extra].
SelectableRegionContextMenuBuilder iPhoneSelectableMenu({
  ExtraEditItems<SelectableRegionState>? extra,
}) =>
    (context, state) => CupertinoAdaptiveTextSelectionToolbar.buttonItems(
      anchors: state.contextMenuAnchors,
      buttonItems: [...state.contextMenuButtonItems, ...?extra?.call(state)],
    );

/// The iPhone's loupe over text while a handle or the caret is dragged.
final nativeMagnifier = TextMagnifierConfiguration(
  magnifierBuilder: (context, controller, info) =>
      CupertinoTextMagnifier(controller: controller, magnifierInfo: info),
);
