import 'package:flutter/widgets.dart';

import '../rail.dart' show ClaktoriesScope;
import '../scope.dart';
import '../state.dart' show WorkspaceStore;
import 'e2ee.dart' show e2eeOf;
import 'people_view.dart' show peopleAvailable;

/// Starts this device's messages engine as soon as the Claktory the app
/// shows offers People, and points it at whichever Claktory that is (row
/// 569): every group is followed while the app runs, not only once People
/// opens, so a friend's moved home is routed to without the person looking.
/// Draws nothing.
class StartMessages extends StatefulWidget {
  const StartMessages({super.key});

  @override
  State<StartMessages> createState() => _StartMessagesState();
}

class _StartMessagesState extends State<StartMessages> {
  WorkspaceStore? _store;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Depended on: another Claktory shown comes here again.
    final scope = context.dependOnInheritedWidgetOfExactType<AppScope>()!;
    final store = scope.backend.workspace;
    if (!identical(store, _store)) {
      _store?.removeListener(_check);
      _store = store..addListener(_check);
    }
    // Depended on too: another Claktory chosen comes here again.
    ClaktoriesScope.of(context);
    _check();
  }

  @override
  void dispose() {
    _store?.removeListener(_check);
    super.dispose();
  }

  void _check() {
    final store = _store;
    if (!mounted || store == null || !peopleAvailable(store.workspace)) return;
    e2eeOf(context);
  }

  @override
  Widget build(BuildContext context) => const SizedBox.shrink();
}
