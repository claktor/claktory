import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'explorer.dart' show shortPath;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';

/// An agent's database, read-only: where its file is, then its tables and
/// views by name, each with how many rows it holds; a click shows its
/// columns.
class DatabaseView extends StatelessWidget {
  const DatabaseView({super.key, required this.database});

  final Database database;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final tables = [
      for (final table in database.tables)
        if (!table.view) table,
    ];
    final views = [
      for (final table in database.tables)
        if (table.view) table,
    ];
    return ListView(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.l),
      children: [
        if (database.path.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.s,
              Space.xs,
              Space.s,
              Space.xs,
            ),
            child: Text(
              shortPath(database.path),
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
        if (database.tables.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.s,
              vertical: Space.s,
            ),
            child: Text(
              'No tables yet. The agent makes them when it has data to keep.',
              style: TextStyles.body.copyWith(color: palette.textFaint),
            ),
          ),
        if (tables.isNotEmpty) ...[
          const _Heading('Tables'),
          for (final table in tables)
            _Table(key: ValueKey('t/${table.name}'), table: table),
        ],
        if (views.isNotEmpty) ...[
          const _Heading('Views'),
          for (final table in views)
            _Table(key: ValueKey('v/${table.name}'), table: table),
        ],
      ],
    );
  }
}

class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Text(
        text,
        style: TextStyles.caption.copyWith(
          color: palette.textFaint,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }
}

/// How many rows: "1 row", "12,345 rows".
String rowsLabel(Int64 rows) {
  final digits = rows.toString();
  final grouped = StringBuffer();
  for (var i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 == 0) grouped.write(',');
    grouped.write(digits[i]);
  }
  return rows == Int64.ONE ? '1 row' : '$grouped rows';
}

/// One table or view: a chevron, its name and its row count; open, its
/// columns with their types.
class _Table extends StatefulWidget {
  const _Table({super.key, required this.table});

  final DatabaseTable table;

  @override
  State<_Table> createState() => _TableState();
}

class _TableState extends State<_Table> {
  var _open = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final table = widget.table;
    return Pressable(
      onTap: () => setState(() => _open = !_open),
      builder: (context, hovered) => Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Space.s,
          vertical: Space.xs + 1,
        ),
        decoration: BoxDecoration(
          color: hovered ? palette.hover : null,
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                SizedBox(
                  width: 14,
                  child: Center(
                    child: Chevron(
                      color: hovered ? palette.textMuted : palette.textFaint,
                      down: _open,
                    ),
                  ),
                ),
                const SizedBox(width: Space.xs),
                Expanded(
                  child: Text(
                    table.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.code.copyWith(color: palette.text),
                  ),
                ),
                const SizedBox(width: Space.s),
                Text(
                  rowsLabel(table.rows),
                  style: TextStyles.caption.copyWith(
                    color: palette.textFaint,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ],
            ),
            if (_open)
              Padding(
                padding: const EdgeInsets.only(
                  left: 14 + Space.xs,
                  top: Space.xs,
                  bottom: Space.xs,
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (table.columns.isEmpty)
                      Text(
                        'No columns.',
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                    for (final column in table.columns)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 1),
                        child: Row(
                          children: [
                            Flexible(
                              child: Text(
                                column.name,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyles.code.copyWith(
                                  color: palette.textMuted,
                                ),
                              ),
                            ),
                            if (column.type.isNotEmpty) ...[
                              const SizedBox(width: Space.s),
                              Text(
                                column.type,
                                style: TextStyles.caption.copyWith(
                                  color: palette.textFaint,
                                ),
                              ),
                            ],
                          ],
                        ),
                      ),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}
