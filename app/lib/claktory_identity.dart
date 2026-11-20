import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart' show thisMacName;
import 'controls.dart';
import 'forms.dart' show ColorPicker;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show SettingRequests, SettingRow;

/// A Claktory's name, the same on every device (`Settings.name`): as it
/// said on connecting until its Settings come, and before that as its
/// pairing code says ("This Mac" for this Mac's own).
String claktoryName(Backend backend) {
  final name = backend.workspace.workspace.settings.name;
  if (name.isNotEmpty) return name;
  if (backend.welcomeName.isNotEmpty) return backend.welcomeName;
  return backend.target?.label ?? thisMacName;
}

/// A Claktory's color (`Settings.color`, one of the agent colors' names),
/// found the same way as [claktoryName]; empty until one is known.
String claktoryColorName(Backend backend) {
  final color = backend.workspace.workspace.settings.color;
  if (color.isNotEmpty) return color;
  if (backend.welcomeColor.isNotEmpty) return backend.welcomeColor;
  return backend.target?.color ?? '';
}

/// [claktoryColorName] as a color; null until one is known.
Color? claktoryColor(Backend backend) {
  final name = claktoryColorName(backend);
  return agentColors[name];
}

/// The Claktory's name and color in Settings → General: every device that
/// connects to it shows them.
class ClaktoryIdentity extends StatefulWidget {
  const ClaktoryIdentity({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<ClaktoryIdentity> createState() => _ClaktoryIdentityState();
}

class _ClaktoryIdentityState extends State<ClaktoryIdentity>
    with SettingRequests {
  late final _name = TextEditingController(
    text: widget.workspace.settings.name,
  );
  late var _shown = widget.workspace.settings.name;

  @override
  void didUpdateWidget(ClaktoryIdentity old) {
    super.didUpdateWidget(old);
    // The backend's name, unless the person is editing it.
    final name = widget.workspace.settings.name;
    if (name == _shown) return;
    if (_name.text.trim() == _shown.trim()) _name.text = name;
    _shown = name;
  }

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _save() => run(
    () =>
        AppScope.of(context)
            .updateSettings(UpdateSettings(name: _name.text.trim())),
  );

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final settings = widget.workspace.settings;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Claktory name',
          child: Row(
            children: [
              SizedBox(
                width: 200,
                child: TextField(
                  controller: _name,
                  // What an empty name goes back to.
                  placeholder: widget.workspace.remote.machine,
                  onSubmitted: (_) => _save(),
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: ListenableBuilder(
                  listenable: _name,
                  builder: (context, _) => _name.text.trim() == settings.name
                      ? Text(
                          'Shown on every device connected to this Claktory.',
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        )
                      : Align(
                          alignment: Alignment.centerLeft,
                          child: Button(label: 'Save', onPressed: _save),
                        ),
                ),
              ),
            ],
          ),
        ),
        SettingRow(
          label: 'Color',
          child: Align(
            alignment: Alignment.centerLeft,
            child: ColorPicker(
              value: settings.color,
              onChanged: (color) => run(
                () =>
                    AppScope.of(context)
                        .updateSettings(UpdateSettings(color: color)),
              ),
            ),
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}
