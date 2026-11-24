import 'package:flutter/widgets.dart';

import '../ipad/pair_step.dart';

/// A wide Android window connects as a store device, not as a Mac that can
/// run its own Claktory, even if its phone idiom was loaded before unfolding.
/// Reuse the shared PairScreen and its existing safe-area
/// and keyboard owner unchanged; do not fork the screen or its wording.
class AndroidPairStep extends StatelessWidget {
  const AndroidPairStep({super.key});

  @override
  Widget build(BuildContext context) => const IPadPairStep();
}
