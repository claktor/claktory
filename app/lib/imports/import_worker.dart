import 'dart:async';
import 'dart:io';
import 'dart:isolate';

import 'claude_export.dart';

/// The person stopped reading an export.
class ExportCancelled implements Exception {
  const ExportCancelled();
}

/// An export being read: its preview, or why not, and a way to stop it.
abstract class ExportJob {
  /// The preview; throws [ExportFailure], or [ExportCancelled] once
  /// [cancel]led.
  Future<ExportPreview> get result;

  /// Stops reading it at once.
  void cancel();
}

/// Reads the export at [path] in an isolate of its own, so a file built to
/// take long or take memory never holds up the app: checked for size
/// before it's read, stopped past [deadline], and stopped at once by
/// [ExportJob.cancel]. Nothing leaves this device.
ExportJob readExportFile(
  String path, {
  ExportLimits limits = const ExportLimits(),
  Duration deadline = const Duration(seconds: 30),
}) => _IsolateJob(path, limits, deadline);

class _IsolateJob implements ExportJob {
  _IsolateJob(String path, ExportLimits limits, Duration deadline) {
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
      _read,
      (_port.sendPort, path, limits),
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
  final _done = Completer<ExportPreview>();
  Isolate? _isolate;
  late final Timer _timer;

  @override
  Future<ExportPreview> get result => _done.future;

  @override
  void cancel() => _end(const ExportCancelled());

  void _receive(Object? message) {
    switch (message) {
      case ExportPreview preview:
        _end(preview);
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

  /// [outcome] is the preview, or what to throw.
  void _end(Object outcome) {
    if (_done.isCompleted) return;
    _timer.cancel();
    _port.close();
    _isolate?.kill(priority: Isolate.immediate);
    if (outcome is ExportPreview) {
      _done.complete(outcome);
    } else {
      _done.completeError(outcome);
    }
  }
}

void _read((SendPort, String, ExportLimits) job) {
  final (reply, path, limits) = job;
  Object outcome;
  try {
    final file = File(path);
    if (file.lengthSync() > limits.maxInput) {
      throw const ExportFailure(ExportProblem.tooBig);
    }
    outcome = readClaudeExport(file.readAsBytesSync(), limits: limits);
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
