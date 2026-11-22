import 'dart:async';
import 'dart:developer' as developer;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'state.dart';

/// What the person has typed in each agent's message box and not sent yet,
/// by agent id. Kept on this device rather than in the backend: the person's
/// decision, the one exception besides the window's frame (AGENTS.md,
/// "Product rules"). So a draft survives switching agents, opening Settings
/// or the Marketplace, and the app restarting or updating.
class Drafts extends ChangeNotifier {
  Drafts(this._storage, [Map<String, String> saved = const {}])
    : _drafts = {...saved};

  /// The drafts this device kept; none if they can't be read.
  static Future<Drafts> load(DraftStorage storage) async {
    try {
      return Drafts(storage, await storage.read());
    } catch (error) {
      developer.log('could not read drafts: $error', name: 'factory.drafts');
      return Drafts(storage);
    }
  }

  final DraftStorage _storage;
  final Map<String, String> _drafts;

  /// The agent's draft, or '' when there's none.
  String of(String agentId) => _drafts[agentId] ?? '';

  /// Keeps [text] as the agent's draft; '' drops it.
  void set(String agentId, String text) {
    if (of(agentId) == text) return;
    if (text.isEmpty) {
      _drafts.remove(agentId);
    } else {
      _drafts[agentId] = text;
    }
    _save();
    notifyListeners();
  }

  /// Starts a message to the agent for the person to finish: [text] becomes
  /// its draft, or goes on a new line after the draft it has, and its
  /// message box takes the focus with the cursor at the end.
  void compose(String agentId, String text) {
    final draft = of(agentId);
    _focusFor = agentId;
    set(agentId, draft.isEmpty ? text : '$draft\n$text');
    notifyListeners();
  }

  /// Starts a reply to a message in the box keyed [key] (an agent's id, or
  /// a chat's `chat:<id>`): the message quoted ([replyQuote]) above what the
  /// draft has, and the box takes the focus with the cursor at the end, to
  /// write the reply under it.
  void reply(String key, String message, {String? from}) {
    final quote = replyQuote(message, from: from);
    if (quote.isEmpty) return;
    final draft = of(key).trim();
    _focusFor = key;
    set(key, draft.isEmpty ? '$quote\n\n' : '$quote\n\n$draft');
    notifyListeners();
  }

  /// The agent whose message box should take the focus, once.
  String? _focusFor;

  /// Whether [agentId]'s message box should take the focus now; true once
  /// per [compose].
  bool takeFocus(String agentId) {
    if (_focusFor != agentId) return false;
    _focusFor = null;
    return true;
  }

  /// Drops the drafts of agents, chats and pipes that no longer exist, each
  /// time the backend sends the workspace. A chat's draft is keyed
  /// `chat:<id>`, a pipe's `pipe:<id>`.
  void watch(WorkspaceStore workspace) {
    void prune() {
      if (!workspace.received) return;
      final kept = {
        for (final agent in workspace.workspace.agents) agent.id,
        for (final chat in workspace.workspace.chats) 'chat:${chat.id}',
        for (final pipe in workspace.workspace.pipes) 'pipe:${pipe.id}',
      };
      final before = _drafts.length;
      _drafts.removeWhere((key, _) => !kept.contains(key));
      if (_drafts.length != before) _save();
    }

    workspace.addListener(prune);
    prune();
  }

  void _save() {
    unawaited(
      _storage.write(Map.of(_drafts)).catchError((Object error) {
        developer.log('could not keep drafts: $error', name: 'factory.drafts');
      }),
    );
  }
}

/// [message] as a reply quotes it: Markdown's quote, `> ` before each
/// line, after a line naming who wrote it when that's not the agent or the
/// person whose conversation it is ([from]). A long message is quoted up to
/// [replyQuoteLength] characters, cut between words, with `…`: enough to
/// say which one, as the agent has the rest.
String replyQuote(String message, {String? from}) {
  var text = message.trim();
  if (text.isEmpty) return '';
  if (text.length > replyQuoteLength) {
    final cut = text.lastIndexOf(RegExp(r'\s'), replyQuoteLength);
    text =
        '${text.substring(0, cut > replyQuoteLength ~/ 2 ? cut : replyQuoteLength).trimRight()}…';
  }
  return [
    if (from != null && from.isNotEmpty) '> $from wrote:',
    for (final line in text.split('\n'))
      line.trimRight().isEmpty ? '>' : '> ${line.trimRight()}',
  ].join('\n');
}

/// How much of a message [replyQuote] quotes.
const replyQuoteLength = 500;

/// Where [Drafts] are kept.
abstract interface class DraftStorage {
  Future<Map<String, String>> read();
  Future<void> write(Map<String, String> drafts);
}

/// This app's user defaults on this device, through the native side
/// (`MainFlutterWindow.swift`), next to the window's frame.
class DeviceDraftStorage implements DraftStorage {
  static const _channel = MethodChannel('factory/device');

  @override
  Future<Map<String, String>> read() async =>
      await _channel.invokeMapMethod<String, String>('drafts') ?? {};

  @override
  Future<void> write(Map<String, String> drafts) =>
      _channel.invokeMethod<void>('setDrafts', drafts);
}

/// Drafts kept in memory, for tests: [saved] is what a restart would read.
class MemoryDraftStorage implements DraftStorage {
  Map<String, String> saved = {};

  @override
  Future<Map<String, String>> read() async => Map.of(saved);

  @override
  Future<void> write(Map<String, String> drafts) async =>
      saved = Map.of(drafts);
}
