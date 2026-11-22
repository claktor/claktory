import 'dart:async';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'claude_export.dart';

/// The person stopped reading an export.
class ExportCancelled implements Exception {
  const ExportCancelled();
}

/// Work on an export in an isolate of its own: its result, or why not,
/// and a way to stop it.
abstract class ImportJob<T> {
  /// The result; throws [ExportFailure], or [ExportCancelled] once
  /// [cancel]led.
  Future<T> get result;

  /// Stops it at once.
  void cancel();
}

/// An export being read for its preview.
typedef ExportJob = ImportJob<ExportPreview>;

/// Reads the export at [path] in an isolate of its own, so a file built to
/// take long or take memory never holds up the app: checked for size
/// before it's read, stopped past [deadline], and stopped at once by
/// [ImportJob.cancel]. Nothing leaves this device.
ExportJob readExportFile(
  String path, {
  ExportLimits limits = const ExportLimits(),
  Duration deadline = const Duration(seconds: 30),
}) => runOnExport(
  path,
  (bytes) => readClaudeExport(bytes, limits: limits),
  limits: limits,
  deadline: deadline,
);

/// Runs [work] on the bytes of the export at [path] in an isolate of its
/// own, as [readExportFile] reads it: the file's size checked first,
/// stopped past [deadline] or when cancelled. [work] goes to the isolate,
/// so it may only hold what can be sent there.
ImportJob<T> runOnExport<T>(
  String path,
  T Function(Uint8List bytes) work, {
  ExportLimits limits = const ExportLimits(),
  Duration deadline = const Duration(seconds: 30),
}) => _IsolateJob<T>(path, work, limits, deadline);

/// What the isolate answers when [work] worked.
class _Done<T> {
  const _Done(this.value);

  final T value;
}

class _IsolateJob<T> implements ImportJob<T> {
  _IsolateJob(
    String path,
    T Function(Uint8List bytes) work,
    ExportLimits limits,
    Duration deadline,
  ) {
    _port.listen(_receive);
    _timer = Timer(
      deadline,
      () => _end(
        ExportFailure(
          ExportProblem.unsafe,
          'still reading after ${deadline.inSeconds} s',
        ),
      ),
    );
    Isolate.spawn(
      _run<T>,
      (_port.sendPort, path, limits, work),
      onError: _port.sendPort,
      onExit: _port.sendPort,
      errorsAreFatal: true,
      debugName: 'export reader',
    ).then(
      (isolate) {
        if (_done.isCompleted) {
          isolate.kill(priority: Isolate.immediate);
        } else {
          _isolate = isolate;
        }
      },
      onError: (Object error) =>
          _end(ExportFailure(ExportProblem.unsafe, '$error')),
    );
  }

  final _port = ReceivePort();
  final _done = Completer<T>();
  Isolate? _isolate;
  late final Timer _timer;

  @override
  Future<T> get result => _done.future;

  @override
  void cancel() => _end(const ExportCancelled());

  void _receive(Object? message) {
    switch (message) {
      case _Done<T> done:
        _end(done);
      case ExportFailure failure:
        _end(failure);
      // An error it didn't expect ([error, stack] as text), or its end
      // with nothing said (out of memory): either way, not read.
      case List<Object?> error:
        _end(ExportFailure(ExportProblem.unsafe, '${error.first}'));
      default:
        _end(const ExportFailure(ExportProblem.unsafe, 'it stopped'));
    }
  }

  /// [outcome] is the result ([_Done]), or what to throw.
  void _end(Object outcome) {
    if (_done.isCompleted) return;
    _timer.cancel();
    _port.close();
    _isolate?.kill(priority: Isolate.immediate);
    if (outcome is _Done<T>) {
      _done.complete(outcome.value);
    } else {
      _done.completeError(outcome);
    }
  }
}

void _run<T>(
  (SendPort, String, ExportLimits, T Function(Uint8List bytes)) job,
) {
  final (reply, path, limits, work) = job;
  Object outcome;
  try {
    final file = File(path);
    if (file.lengthSync() > limits.maxInput) {
      throw const ExportFailure(ExportProblem.tooBig);
    }
    outcome = _Done<T>(work(file.readAsBytesSync()));
  } on ExportFailure catch (failure) {
    outcome = failure;
  } on FileSystemException catch (error) {
    outcome = ExportFailure(ExportProblem.unsupported, error.message);
  } on Object catch (error) {
    // A file built to trip the reader up (offsets past its end, say).
    outcome = ExportFailure(ExportProblem.unsafe, '$error');
  }
  Isolate.exit(reply, outcome);
}
