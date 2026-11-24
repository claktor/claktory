// This is a generated file - do not edit.
//
// Generated from factory/v1/factory.proto.

// @dart = 3.3

// ignore_for_file: annotate_overrides, camel_case_types, comment_references
// ignore_for_file: constant_identifier_names
// ignore_for_file: curly_braces_in_flow_control_structures
// ignore_for_file: deprecated_member_use_from_same_package, library_prefixes
// ignore_for_file: non_constant_identifier_names, prefer_relative_imports

import 'dart:core' as $core;

import 'package:protobuf/protobuf.dart' as $pb;

class DeviceKind extends $pb.ProtobufEnum {
  static const DeviceKind DEVICE_KIND_UNSPECIFIED =
      DeviceKind._(0, _omitEnumNames ? '' : 'DEVICE_KIND_UNSPECIFIED');

  /// A computer at a desk: an iMac, a Mac mini or Studio, a desktop PC.
  static const DeviceKind DEVICE_KIND_DESKTOP =
      DeviceKind._(1, _omitEnumNames ? '' : 'DEVICE_KIND_DESKTOP');

  /// A laptop: a MacBook.
  static const DeviceKind DEVICE_KIND_LAPTOP =
      DeviceKind._(2, _omitEnumNames ? '' : 'DEVICE_KIND_LAPTOP');
  static const DeviceKind DEVICE_KIND_PHONE =
      DeviceKind._(3, _omitEnumNames ? '' : 'DEVICE_KIND_PHONE');
  static const DeviceKind DEVICE_KIND_TABLET =
      DeviceKind._(4, _omitEnumNames ? '' : 'DEVICE_KIND_TABLET');

  static const $core.List<DeviceKind> values = <DeviceKind>[
    DEVICE_KIND_UNSPECIFIED,
    DEVICE_KIND_DESKTOP,
    DEVICE_KIND_LAPTOP,
    DEVICE_KIND_PHONE,
    DEVICE_KIND_TABLET,
  ];

  static final $core.List<DeviceKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static DeviceKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const DeviceKind._(super.value, super.name);
}

/// How a device's connection travels.
class DevicePath extends $pb.ProtobufEnum {
  /// Not connected.
  static const DevicePath DEVICE_PATH_NONE =
      DevicePath._(0, _omitEnumNames ? '' : 'DEVICE_PATH_NONE');

  /// Straight between the two machines (the same machine, a LAN, or a hole
  /// punched through the NATs).
  static const DevicePath DEVICE_PATH_DIRECT =
      DevicePath._(1, _omitEnumNames ? '' : 'DEVICE_PATH_DIRECT');

  /// Through a relay, end-to-end encrypted.
  static const DevicePath DEVICE_PATH_RELAYED =
      DevicePath._(2, _omitEnumNames ? '' : 'DEVICE_PATH_RELAYED');

  static const $core.List<DevicePath> values = <DevicePath>[
    DEVICE_PATH_NONE,
    DEVICE_PATH_DIRECT,
    DEVICE_PATH_RELAYED,
  ];

  static final $core.List<DevicePath?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static DevicePath? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const DevicePath._(super.value, super.name);
}

/// What an upload is for.
class UploadPurpose extends $pb.ProtobufEnum {
  /// A file attached to a message for `agent_id`, stored in its uploads.
  static const UploadPurpose UPLOAD_PURPOSE_AGENT =
      UploadPurpose._(0, _omitEnumNames ? '' : 'UPLOAD_PURPOSE_AGENT');

  /// An ink blob: an iPad drawing ("drawing.pkdrawing",
  /// "application/x-pencilkit", at most 8 MiB) or a PNG ("image/png": a
  /// preview, at most 4 MiB, or a review item's context image, at most
  /// 3 MiB and 2048 px on its long edge, checked when a thread names it).
  /// `agent_id` is empty. Kept privately, by its SHA-256, where no agent's
  /// tools reach, until the person sends it; FinishUpload must carry the
  /// hash, and answers UploadedFile.blob_sha256.
  static const UploadPurpose UPLOAD_PURPOSE_INK =
      UploadPurpose._(1, _omitEnumNames ? '' : 'UPLOAD_PURPOSE_INK');

  /// A recording to transcribe for the person (TranscribeAudio): AAC in .m4a
  /// ("audio/mp4"), 16 kHz mono, at most 10 minutes and 16 MiB. `agent_id`
  /// is empty. Kept privately, where no agent's tools reach, only until a
  /// TranscribeAudio takes it, or an hour; then it's deleted. FinishUpload
  /// answers UploadedFile with neither an attachment nor a blob hash.
  static const UploadPurpose UPLOAD_PURPOSE_DICTATION =
      UploadPurpose._(2, _omitEnumNames ? '' : 'UPLOAD_PURPOSE_DICTATION');

  static const $core.List<UploadPurpose> values = <UploadPurpose>[
    UPLOAD_PURPOSE_AGENT,
    UPLOAD_PURPOSE_INK,
    UPLOAD_PURPOSE_DICTATION,
  ];

  static final $core.List<UploadPurpose?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static UploadPurpose? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const UploadPurpose._(super.value, super.name);
}

class MonitorAction extends $pb.ProtobufEnum {
  static const MonitorAction MONITOR_ACTION_UNSPECIFIED =
      MonitorAction._(0, _omitEnumNames ? '' : 'MONITOR_ACTION_UNSPECIFIED');
  static const MonitorAction MONITOR_ACTION_PAUSE =
      MonitorAction._(1, _omitEnumNames ? '' : 'MONITOR_ACTION_PAUSE');
  static const MonitorAction MONITOR_ACTION_RESUME =
      MonitorAction._(2, _omitEnumNames ? '' : 'MONITOR_ACTION_RESUME');
  static const MonitorAction MONITOR_ACTION_CANCEL =
      MonitorAction._(3, _omitEnumNames ? '' : 'MONITOR_ACTION_CANCEL');

  static const $core.List<MonitorAction> values = <MonitorAction>[
    MONITOR_ACTION_UNSPECIFIED,
    MONITOR_ACTION_PAUSE,
    MONITOR_ACTION_RESUME,
    MONITOR_ACTION_CANCEL,
  ];

  static final $core.List<MonitorAction?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static MonitorAction? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MonitorAction._(super.value, super.name);
}

/// Whether a FileContent is a preview (#379).
class FilePreview extends $pb.ProtobufEnum {
  /// The original: no preview was asked, or none was needed (small enough,
  /// it fits the screen already, a preview wouldn't be smaller, a GIF, or a
  /// PDF of 5 MB or less, which comes as its first range as before).
  static const FilePreview FILE_PREVIEW_NONE =
      FilePreview._(0, _omitEnumNames ? '' : 'FILE_PREVIEW_NONE');

  /// A preview, smaller than the original: the app says so and offers the
  /// original. Saving and sharing read the original.
  static const FilePreview FILE_PREVIEW_MADE =
      FilePreview._(1, _omitEnumNames ? '' : 'FILE_PREVIEW_MADE');

  /// A preview was asked and would have been made, but this Claktory can't
  /// make one (a video on Linux without ffmpeg, a PDF's page off a Mac): the
  /// original came, and the app says "No preview on this Claktory". For a
  /// PDF's page (ReadFile.preview_page), none of the PDF's bytes come, only
  /// its size and version: the app loads the PDF by its own requests.
  static const FilePreview FILE_PREVIEW_UNAVAILABLE =
      FilePreview._(2, _omitEnumNames ? '' : 'FILE_PREVIEW_UNAVAILABLE');

  static const $core.List<FilePreview> values = <FilePreview>[
    FILE_PREVIEW_NONE,
    FILE_PREVIEW_MADE,
    FILE_PREVIEW_UNAVAILABLE,
  ];

  static final $core.List<FilePreview?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static FilePreview? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const FilePreview._(super.value, super.name);
}

/// ROW: children side by side, left to right; COLUMN: top to bottom.
class ViewAxis extends $pb.ProtobufEnum {
  static const ViewAxis VIEW_AXIS_UNSPECIFIED =
      ViewAxis._(0, _omitEnumNames ? '' : 'VIEW_AXIS_UNSPECIFIED');
  static const ViewAxis VIEW_AXIS_ROW =
      ViewAxis._(1, _omitEnumNames ? '' : 'VIEW_AXIS_ROW');
  static const ViewAxis VIEW_AXIS_COLUMN =
      ViewAxis._(2, _omitEnumNames ? '' : 'VIEW_AXIS_COLUMN');

  static const $core.List<ViewAxis> values = <ViewAxis>[
    VIEW_AXIS_UNSPECIFIED,
    VIEW_AXIS_ROW,
    VIEW_AXIS_COLUMN,
  ];

  static final $core.List<ViewAxis?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static ViewAxis? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ViewAxis._(super.value, super.name);
}

class SuggestionDecision extends $pb.ProtobufEnum {
  /// Not decided yet.
  static const SuggestionDecision SUGGESTION_DECISION_UNSPECIFIED =
      SuggestionDecision._(
          0, _omitEnumNames ? '' : 'SUGGESTION_DECISION_UNSPECIFIED');
  static const SuggestionDecision SUGGESTION_DECISION_ACCEPTED =
      SuggestionDecision._(
          1, _omitEnumNames ? '' : 'SUGGESTION_DECISION_ACCEPTED');
  static const SuggestionDecision SUGGESTION_DECISION_REJECTED =
      SuggestionDecision._(
          2, _omitEnumNames ? '' : 'SUGGESTION_DECISION_REJECTED');

  static const $core.List<SuggestionDecision> values = <SuggestionDecision>[
    SUGGESTION_DECISION_UNSPECIFIED,
    SUGGESTION_DECISION_ACCEPTED,
    SUGGESTION_DECISION_REJECTED,
  ];

  static final $core.List<SuggestionDecision?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static SuggestionDecision? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const SuggestionDecision._(super.value, super.name);
}

class ThreadStatus extends $pb.ProtobufEnum {
  static const ThreadStatus THREAD_STATUS_UNSPECIFIED =
      ThreadStatus._(0, _omitEnumNames ? '' : 'THREAD_STATUS_UNSPECIFIED');
  static const ThreadStatus THREAD_STATUS_OPEN =
      ThreadStatus._(1, _omitEnumNames ? '' : 'THREAD_STATUS_OPEN');
  static const ThreadStatus THREAD_STATUS_RESOLVED =
      ThreadStatus._(2, _omitEnumNames ? '' : 'THREAD_STATUS_RESOLVED');

  static const $core.List<ThreadStatus> values = <ThreadStatus>[
    THREAD_STATUS_UNSPECIFIED,
    THREAD_STATUS_OPEN,
    THREAD_STATUS_RESOLVED,
  ];

  static final $core.List<ThreadStatus?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static ThreadStatus? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ThreadStatus._(super.value, super.name);
}

class AnchorState extends $pb.ProtobufEnum {
  static const AnchorState ANCHOR_STATE_UNSPECIFIED =
      AnchorState._(0, _omitEnumNames ? '' : 'ANCHOR_STATE_UNSPECIFIED');

  /// On the text it was made on.
  static const AnchorState ANCHOR_STATE_ANCHORED =
      AnchorState._(1, _omitEnumNames ? '' : 'ANCHOR_STATE_ANCHORED');

  /// Found again, but the text there changed since (see original_exact).
  static const AnchorState ANCHOR_STATE_MOVED =
      AnchorState._(2, _omitEnumNames ? '' : 'ANCHOR_STATE_MOVED');

  /// Not found in the file any more (or the file is gone); kept, never
  /// dropped. ReattachThread puts it on new text.
  static const AnchorState ANCHOR_STATE_DETACHED =
      AnchorState._(3, _omitEnumNames ? '' : 'ANCHOR_STATE_DETACHED');

  /// An image thread on a file that changed since: it may still be right.
  static const AnchorState ANCHOR_STATE_OUTDATED =
      AnchorState._(4, _omitEnumNames ? '' : 'ANCHOR_STATE_OUTDATED');

  static const $core.List<AnchorState> values = <AnchorState>[
    ANCHOR_STATE_UNSPECIFIED,
    ANCHOR_STATE_ANCHORED,
    ANCHOR_STATE_MOVED,
    ANCHOR_STATE_DETACHED,
    ANCHOR_STATE_OUTDATED,
  ];

  static final $core.List<AnchorState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static AnchorState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AnchorState._(super.value, super.name);
}

class ErrorCode extends $pb.ProtobufEnum {
  static const ErrorCode ERROR_CODE_UNSPECIFIED =
      ErrorCode._(0, _omitEnumNames ? '' : 'ERROR_CODE_UNSPECIFIED');

  /// The client speaks a protocol version the server doesn't. Connection closes.
  static const ErrorCode ERROR_CODE_UNSUPPORTED_PROTOCOL_VERSION = ErrorCode._(
      1, _omitEnumNames ? '' : 'ERROR_CODE_UNSUPPORTED_PROTOCOL_VERSION');

  /// The first message wasn't `hello`. Connection closes.
  static const ErrorCode ERROR_CODE_HANDSHAKE_REQUIRED =
      ErrorCode._(2, _omitEnumNames ? '' : 'ERROR_CODE_HANDSHAKE_REQUIRED');

  /// A frame couldn't be decoded as a ClientMessage.
  static const ErrorCode ERROR_CODE_MALFORMED_MESSAGE =
      ErrorCode._(3, _omitEnumNames ? '' : 'ERROR_CODE_MALFORMED_MESSAGE');

  /// A valid message of a kind this server doesn't know (sent by a newer client).
  static const ErrorCode ERROR_CODE_UNKNOWN_MESSAGE =
      ErrorCode._(4, _omitEnumNames ? '' : 'ERROR_CODE_UNKNOWN_MESSAGE');

  /// The request named a project, agent or provider that doesn't exist.
  static const ErrorCode ERROR_CODE_NOT_FOUND =
      ErrorCode._(5, _omitEnumNames ? '' : 'ERROR_CODE_NOT_FOUND');

  /// The request's fields are invalid (empty name, unknown model, ...).
  static const ErrorCode ERROR_CODE_INVALID_REQUEST =
      ErrorCode._(6, _omitEnumNames ? '' : 'ERROR_CODE_INVALID_REQUEST');

  /// The server failed to carry out a valid request (e.g. couldn't write a file).
  static const ErrorCode ERROR_CODE_INTERNAL =
      ErrorCode._(7, _omitEnumNames ? '' : 'ERROR_CODE_INTERNAL');

  /// On the remote listener: the device's token isn't one of a paired device
  /// (never paired, or unpaired since). Connection closes.
  static const ErrorCode ERROR_CODE_UNAUTHORIZED =
      ErrorCode._(8, _omitEnumNames ? '' : 'ERROR_CODE_UNAUTHORIZED');

  /// The vault is locked (locked mode, after the backend restarted; only a
  /// vault the person gave a key of their own can be): UnlockVault first.
  static const ErrorCode ERROR_CODE_VAULT_LOCKED =
      ErrorCode._(9, _omitEnumNames ? '' : 'ERROR_CODE_VAULT_LOCKED');

  /// No longer sent: requests need no vault passphrase.
  static const ErrorCode ERROR_CODE_PROOF_REQUIRED =
      ErrorCode._(10, _omitEnumNames ? '' : 'ERROR_CODE_PROOF_REQUIRED');

  /// The passphrase (or recovery key) is wrong. Answered after a second's
  /// delay.
  static const ErrorCode ERROR_CODE_WRONG_PROOF =
      ErrorCode._(11, _omitEnumNames ? '' : 'ERROR_CODE_WRONG_PROOF');

  /// The secret isn't granted to that agent (RevokeSecret).
  static const ErrorCode ERROR_CODE_NOT_GRANTED =
      ErrorCode._(12, _omitEnumNames ? '' : 'ERROR_CODE_NOT_GRANTED');

  /// The account service asks to wait (too many codes or tries); the
  /// message says how long.
  static const ErrorCode ERROR_CODE_RATE_LIMITED =
      ErrorCode._(13, _omitEnumNames ? '' : 'ERROR_CODE_RATE_LIMITED');

  /// The account service can't be reached (or answered something Claktory
  /// can't trust); the message says which. Claktory keeps working.
  static const ErrorCode ERROR_CODE_UNREACHABLE =
      ErrorCode._(14, _omitEnumNames ? '' : 'ERROR_CODE_UNREACHABLE');

  /// Hello refused: this device has to show a (fresh) device assertion. A
  /// paired device whose grace to confirm is over, or a device that found
  /// this Claktory through its account with an assertion missing or expired.
  /// The app signs in to the account (or gets a new assertion) and connects
  /// again. Connection closes.
  static const ErrorCode ERROR_CODE_SIGN_IN_REQUIRED =
      ErrorCode._(15, _omitEnumNames ? '' : 'ERROR_CODE_SIGN_IN_REQUIRED');

  /// This Claktory moved to another computer (Error.moved says where); the
  /// copy left here runs nothing.
  static const ErrorCode ERROR_CODE_MOVED =
      ErrorCode._(16, _omitEnumNames ? '' : 'ERROR_CODE_MOVED');

  static const $core.List<ErrorCode> values = <ErrorCode>[
    ERROR_CODE_UNSPECIFIED,
    ERROR_CODE_UNSUPPORTED_PROTOCOL_VERSION,
    ERROR_CODE_HANDSHAKE_REQUIRED,
    ERROR_CODE_MALFORMED_MESSAGE,
    ERROR_CODE_UNKNOWN_MESSAGE,
    ERROR_CODE_NOT_FOUND,
    ERROR_CODE_INVALID_REQUEST,
    ERROR_CODE_INTERNAL,
    ERROR_CODE_UNAUTHORIZED,
    ERROR_CODE_VAULT_LOCKED,
    ERROR_CODE_PROOF_REQUIRED,
    ERROR_CODE_WRONG_PROOF,
    ERROR_CODE_NOT_GRANTED,
    ERROR_CODE_RATE_LIMITED,
    ERROR_CODE_UNREACHABLE,
    ERROR_CODE_SIGN_IN_REQUIRED,
    ERROR_CODE_MOVED,
  ];

  static final $core.List<ErrorCode?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 16);
  static ErrorCode? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ErrorCode._(super.value, super.name);
}

class UpdateCheckState extends $pb.ProtobufEnum {
  /// Nobody has checked since the backend started: show only the button.
  static const UpdateCheckState UPDATE_CHECK_STATE_NONE =
      UpdateCheckState._(0, _omitEnumNames ? '' : 'UPDATE_CHECK_STATE_NONE');
  static const UpdateCheckState UPDATE_CHECK_STATE_CHECKING =
      UpdateCheckState._(
          1, _omitEnumNames ? '' : 'UPDATE_CHECK_STATE_CHECKING');

  /// Nothing newer than this backend is here, and the channel offers
  /// nothing newer either. `version`: this backend's (Workspace.version).
  static const UpdateCheckState UPDATE_CHECK_STATE_UP_TO_DATE =
      UpdateCheckState._(
          2, _omitEnumNames ? '' : 'UPDATE_CHECK_STATE_UP_TO_DATE');

  /// A newer release, `version`, is downloading.
  static const UpdateCheckState UPDATE_CHECK_STATE_DOWNLOADING =
      UpdateCheckState._(
          3, _omitEnumNames ? '' : 'UPDATE_CHECK_STATE_DOWNLOADING');

  /// A release newer than this backend, `version`, is here:
  /// Workspace.latest_release. For UP_TO_DATE and READY alike, word the
  /// answer by the app's own version, as the Update banner decides: an
  /// app-only release this app already runs is "up to date" for it.
  static const UpdateCheckState UPDATE_CHECK_STATE_READY =
      UpdateCheckState._(4, _omitEnumNames ? '' : 'UPDATE_CHECK_STATE_READY');

  /// The check couldn't finish; `error` says why, for the person.
  static const UpdateCheckState UPDATE_CHECK_STATE_FAILED =
      UpdateCheckState._(5, _omitEnumNames ? '' : 'UPDATE_CHECK_STATE_FAILED');

  static const $core.List<UpdateCheckState> values = <UpdateCheckState>[
    UPDATE_CHECK_STATE_NONE,
    UPDATE_CHECK_STATE_CHECKING,
    UPDATE_CHECK_STATE_UP_TO_DATE,
    UPDATE_CHECK_STATE_DOWNLOADING,
    UPDATE_CHECK_STATE_READY,
    UPDATE_CHECK_STATE_FAILED,
  ];

  static final $core.List<UpdateCheckState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static UpdateCheckState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const UpdateCheckState._(super.value, super.name);
}

/// Keeping a laptop's agents working with its lid closed: a helper the Mac
/// app installs (SMAppService, approved with an administrator password in
/// System Settings → Login Items) stops the Mac from sleeping while agents
/// work and it's plugged in, and lets go by itself on low battery, heat or
/// the lid closed on battery.
class LidClosedState extends $pb.ProtobufEnum {
  /// The person hasn't turned it on, or turned it off.
  static const LidClosedState LID_CLOSED_OFF =
      LidClosedState._(0, _omitEnumNames ? '' : 'LID_CLOSED_OFF');

  /// Turned on, and the helper waits for the person's approval in System
  /// Settings → Login Items (the app opens it).
  static const LidClosedState LID_CLOSED_NEEDS_APPROVAL =
      LidClosedState._(1, _omitEnumNames ? '' : 'LID_CLOSED_NEEDS_APPROVAL');

  /// Turned on and the helper answers: it holds the Mac awake while agents
  /// work on power (lid_closed_holding).
  static const LidClosedState LID_CLOSED_ON =
      LidClosedState._(2, _omitEnumNames ? '' : 'LID_CLOSED_ON');

  /// On, and the helper let go: the battery is at 20% or less (it holds again
  /// at 30% or on power).
  static const LidClosedState LID_CLOSED_PAUSED_BATTERY =
      LidClosedState._(3, _omitEnumNames ? '' : 'LID_CLOSED_PAUSED_BATTERY');

  /// On, and the helper let go: the Mac is too hot (it holds again 10 minutes
  /// after it cools down).
  static const LidClosedState LID_CLOSED_PAUSED_HEAT =
      LidClosedState._(4, _omitEnumNames ? '' : 'LID_CLOSED_PAUSED_HEAT');

  /// On, and the helper let go: the lid is closed on battery, as in a bag.
  /// It holds again once plugged in.
  static const LidClosedState LID_CLOSED_PAUSED_LID_ON_BATTERY =
      LidClosedState._(
          5, _omitEnumNames ? '' : 'LID_CLOSED_PAUSED_LID_ON_BATTERY');

  /// On, but no helper answers: it isn't installed, it was turned off in
  /// System Settings, or macOS refused to start it (then Login Items shows it
  /// switched off until the person switches it on again, even after it's
  /// installed again). The Mac app registers it again and offers Login Items;
  /// it never assumes an earlier approval still holds.
  static const LidClosedState LID_CLOSED_MISSING =
      LidClosedState._(6, _omitEnumNames ? '' : 'LID_CLOSED_MISSING');

  /// On, but the helper is from another version of Claktory: the Mac app
  /// installs it again. factoryd asks it to hold nothing meanwhile.
  static const LidClosedState LID_CLOSED_OUTDATED =
      LidClosedState._(7, _omitEnumNames ? '' : 'LID_CLOSED_OUTDATED');

  /// On, but installing or removing the helper failed: lid_closed_error says
  /// why.
  static const LidClosedState LID_CLOSED_FAILED =
      LidClosedState._(8, _omitEnumNames ? '' : 'LID_CLOSED_FAILED');

  static const $core.List<LidClosedState> values = <LidClosedState>[
    LID_CLOSED_OFF,
    LID_CLOSED_NEEDS_APPROVAL,
    LID_CLOSED_ON,
    LID_CLOSED_PAUSED_BATTERY,
    LID_CLOSED_PAUSED_HEAT,
    LID_CLOSED_PAUSED_LID_ON_BATTERY,
    LID_CLOSED_MISSING,
    LID_CLOSED_OUTDATED,
    LID_CLOSED_FAILED,
  ];

  static final $core.List<LidClosedState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 8);
  static LidClosedState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const LidClosedState._(super.value, super.name);
}

/// What macOS answered the Mac app about the helper (SMAppService's status,
/// or the failure of register or unregister), for what the setting shows.
/// Holding comes only from the helper's own answer, never from this.
class LidClosedRegistration extends $pb.ProtobufEnum {
  static const LidClosedRegistration LID_CLOSED_REGISTRATION_UNKNOWN =
      LidClosedRegistration._(
          0, _omitEnumNames ? '' : 'LID_CLOSED_REGISTRATION_UNKNOWN');
  static const LidClosedRegistration LID_CLOSED_REGISTRATION_ENABLED =
      LidClosedRegistration._(
          1, _omitEnumNames ? '' : 'LID_CLOSED_REGISTRATION_ENABLED');
  static const LidClosedRegistration LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL =
      LidClosedRegistration._(
          2, _omitEnumNames ? '' : 'LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL');
  static const LidClosedRegistration LID_CLOSED_REGISTRATION_NOT_REGISTERED =
      LidClosedRegistration._(
          3, _omitEnumNames ? '' : 'LID_CLOSED_REGISTRATION_NOT_REGISTERED');
  static const LidClosedRegistration LID_CLOSED_REGISTRATION_NOT_FOUND =
      LidClosedRegistration._(
          4, _omitEnumNames ? '' : 'LID_CLOSED_REGISTRATION_NOT_FOUND');
  static const LidClosedRegistration LID_CLOSED_REGISTRATION_FAILED =
      LidClosedRegistration._(
          5, _omitEnumNames ? '' : 'LID_CLOSED_REGISTRATION_FAILED');

  static const $core.List<LidClosedRegistration> values =
      <LidClosedRegistration>[
    LID_CLOSED_REGISTRATION_UNKNOWN,
    LID_CLOSED_REGISTRATION_ENABLED,
    LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL,
    LID_CLOSED_REGISTRATION_NOT_REGISTERED,
    LID_CLOSED_REGISTRATION_NOT_FOUND,
    LID_CLOSED_REGISTRATION_FAILED,
  ];

  static final $core.List<LidClosedRegistration?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static LidClosedRegistration? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const LidClosedRegistration._(super.value, super.name);
}

class ChangelogKind extends $pb.ProtobufEnum {
  /// A line with none of the prefixes.
  static const ChangelogKind CHANGELOG_KIND_OTHER =
      ChangelogKind._(0, _omitEnumNames ? '' : 'CHANGELOG_KIND_OTHER');
  static const ChangelogKind CHANGELOG_KIND_ADDED =
      ChangelogKind._(1, _omitEnumNames ? '' : 'CHANGELOG_KIND_ADDED');
  static const ChangelogKind CHANGELOG_KIND_CHANGED =
      ChangelogKind._(2, _omitEnumNames ? '' : 'CHANGELOG_KIND_CHANGED');
  static const ChangelogKind CHANGELOG_KIND_FIXED =
      ChangelogKind._(3, _omitEnumNames ? '' : 'CHANGELOG_KIND_FIXED');

  static const $core.List<ChangelogKind> values = <ChangelogKind>[
    CHANGELOG_KIND_OTHER,
    CHANGELOG_KIND_ADDED,
    CHANGELOG_KIND_CHANGED,
    CHANGELOG_KIND_FIXED,
  ];

  static final $core.List<ChangelogKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static ChangelogKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ChangelogKind._(super.value, super.name);
}

class HighlightKind extends $pb.ProtobufEnum {
  static const HighlightKind HIGHLIGHT_KIND_UNSPECIFIED =
      HighlightKind._(0, _omitEnumNames ? '' : 'HIGHLIGHT_KIND_UNSPECIFIED');

  /// An agent messaged another (agents_send): targets[0] is the sender's
  /// "agent" target, targets[1] the recipient's.
  static const HighlightKind HIGHLIGHT_KIND_MESSAGE =
      HighlightKind._(1, _omitEnumNames ? '' : 'HIGHLIGHT_KIND_MESSAGE');

  /// An agent asked for it with its highlight tool.
  static const HighlightKind HIGHLIGHT_KIND_AGENT =
      HighlightKind._(2, _omitEnumNames ? '' : 'HIGHLIGHT_KIND_AGENT');

  static const $core.List<HighlightKind> values = <HighlightKind>[
    HIGHLIGHT_KIND_UNSPECIFIED,
    HIGHLIGHT_KIND_MESSAGE,
    HIGHLIGHT_KIND_AGENT,
  ];

  static final $core.List<HighlightKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static HighlightKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const HighlightKind._(super.value, super.name);
}

class ExtensionOptionKind extends $pb.ProtobufEnum {
  static const ExtensionOptionKind EXTENSION_OPTION_KIND_UNSPECIFIED =
      ExtensionOptionKind._(
          0, _omitEnumNames ? '' : 'EXTENSION_OPTION_KIND_UNSPECIFIED');

  /// On or off: `OptionValue.on`.
  static const ExtensionOptionKind EXTENSION_OPTION_KIND_SWITCH =
      ExtensionOptionKind._(
          1, _omitEnumNames ? '' : 'EXTENSION_OPTION_KIND_SWITCH');

  /// A list of agents: `OptionValue.agent_ids`.
  static const ExtensionOptionKind EXTENSION_OPTION_KIND_AGENTS =
      ExtensionOptionKind._(
          2, _omitEnumNames ? '' : 'EXTENSION_OPTION_KIND_AGENTS');

  /// One of `ExtensionOption.choices`: `OptionValue.choice`.
  static const ExtensionOptionKind EXTENSION_OPTION_KIND_CHOICE =
      ExtensionOptionKind._(
          3, _omitEnumNames ? '' : 'EXTENSION_OPTION_KIND_CHOICE');

  static const $core.List<ExtensionOptionKind> values = <ExtensionOptionKind>[
    EXTENSION_OPTION_KIND_UNSPECIFIED,
    EXTENSION_OPTION_KIND_SWITCH,
    EXTENSION_OPTION_KIND_AGENTS,
    EXTENSION_OPTION_KIND_CHOICE,
  ];

  static final $core.List<ExtensionOptionKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static ExtensionOptionKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ExtensionOptionKind._(super.value, super.name);
}

/// What a blocked checklist item waits on (#279).
class BlockedKind extends $pb.ProtobufEnum {
  static const BlockedKind BLOCKED_KIND_UNSPECIFIED =
      BlockedKind._(0, _omitEnumNames ? '' : 'BLOCKED_KIND_UNSPECIFIED');

  /// A decision only the person can make.
  static const BlockedKind BLOCKED_KIND_PERSON_DECISION =
      BlockedKind._(1, _omitEnumNames ? '' : 'BLOCKED_KIND_PERSON_DECISION');

  /// Something only the person can do (sign in, pay, plug in).
  static const BlockedKind BLOCKED_KIND_PERSON_ACTION =
      BlockedKind._(2, _omitEnumNames ? '' : 'BLOCKED_KIND_PERSON_ACTION');

  /// Another checklist item (blocked_ref): it opens again by itself when
  /// that one is done or dropped, and its owner is told.
  static const BlockedKind BLOCKED_KIND_ITEM =
      BlockedKind._(3, _omitEnumNames ? '' : 'BLOCKED_KIND_ITEM');

  /// Another agent (blocked_ref).
  static const BlockedKind BLOCKED_KIND_AGENT =
      BlockedKind._(4, _omitEnumNames ? '' : 'BLOCKED_KIND_AGENT');

  /// Something outside: its owner is told to check again at retry_at_ms,
  /// then each time twice as long after.
  static const BlockedKind BLOCKED_KIND_EXTERNAL =
      BlockedKind._(5, _omitEnumNames ? '' : 'BLOCKED_KIND_EXTERNAL');

  static const $core.List<BlockedKind> values = <BlockedKind>[
    BLOCKED_KIND_UNSPECIFIED,
    BLOCKED_KIND_PERSON_DECISION,
    BLOCKED_KIND_PERSON_ACTION,
    BLOCKED_KIND_ITEM,
    BLOCKED_KIND_AGENT,
    BLOCKED_KIND_EXTERNAL,
  ];

  static final $core.List<BlockedKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static BlockedKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const BlockedKind._(super.value, super.name);
}

class ChecklistStatus extends $pb.ProtobufEnum {
  static const ChecklistStatus CHECKLIST_STATUS_UNSPECIFIED = ChecklistStatus._(
      0, _omitEnumNames ? '' : 'CHECKLIST_STATUS_UNSPECIFIED');
  static const ChecklistStatus CHECKLIST_STATUS_OPEN =
      ChecklistStatus._(1, _omitEnumNames ? '' : 'CHECKLIST_STATUS_OPEN');
  static const ChecklistStatus CHECKLIST_STATUS_IN_PROGRESS = ChecklistStatus._(
      2, _omitEnumNames ? '' : 'CHECKLIST_STATUS_IN_PROGRESS');
  static const ChecklistStatus CHECKLIST_STATUS_DONE =
      ChecklistStatus._(3, _omitEnumNames ? '' : 'CHECKLIST_STATUS_DONE');
  static const ChecklistStatus CHECKLIST_STATUS_DROPPED =
      ChecklistStatus._(4, _omitEnumNames ? '' : 'CHECKLIST_STATUS_DROPPED');

  /// Waiting on the person or something outside; still outstanding.
  static const ChecklistStatus CHECKLIST_STATUS_BLOCKED =
      ChecklistStatus._(5, _omitEnumNames ? '' : 'CHECKLIST_STATUS_BLOCKED');

  /// Ongoing or waiting its turn, with nothing to wait on (#279): still
  /// outstanding, never nudged or asked whether it's still blocked; shown
  /// apart from Blocked. It can resume at a set time (retry_at_ms).
  static const ChecklistStatus CHECKLIST_STATUS_PARKED =
      ChecklistStatus._(6, _omitEnumNames ? '' : 'CHECKLIST_STATUS_PARKED');

  static const $core.List<ChecklistStatus> values = <ChecklistStatus>[
    CHECKLIST_STATUS_UNSPECIFIED,
    CHECKLIST_STATUS_OPEN,
    CHECKLIST_STATUS_IN_PROGRESS,
    CHECKLIST_STATUS_DONE,
    CHECKLIST_STATUS_DROPPED,
    CHECKLIST_STATUS_BLOCKED,
    CHECKLIST_STATUS_PARKED,
  ];

  static final $core.List<ChecklistStatus?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 6);
  static ChecklistStatus? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ChecklistStatus._(super.value, super.name);
}

class MemoryChangeKind extends $pb.ProtobufEnum {
  static const MemoryChangeKind MEMORY_CHANGE_KIND_UNSPECIFIED =
      MemoryChangeKind._(
          0, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_UNSPECIFIED');
  static const MemoryChangeKind MEMORY_CHANGE_KIND_PREFERENCE_ADDED =
      MemoryChangeKind._(
          1, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_PREFERENCE_ADDED');
  static const MemoryChangeKind MEMORY_CHANGE_KIND_PREFERENCE_CHANGED =
      MemoryChangeKind._(
          2, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_PREFERENCE_CHANGED');
  static const MemoryChangeKind MEMORY_CHANGE_KIND_PREFERENCE_REMOVED =
      MemoryChangeKind._(
          3, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_PREFERENCE_REMOVED');
  static const MemoryChangeKind MEMORY_CHANGE_KIND_NOTE_WRITTEN =
      MemoryChangeKind._(
          4, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_NOTE_WRITTEN');
  static const MemoryChangeKind MEMORY_CHANGE_KIND_NOTE_EDITED =
      MemoryChangeKind._(
          5, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_NOTE_EDITED');
  static const MemoryChangeKind MEMORY_CHANGE_KIND_NOTE_REMOVED =
      MemoryChangeKind._(
          6, _omitEnumNames ? '' : 'MEMORY_CHANGE_KIND_NOTE_REMOVED');

  static const $core.List<MemoryChangeKind> values = <MemoryChangeKind>[
    MEMORY_CHANGE_KIND_UNSPECIFIED,
    MEMORY_CHANGE_KIND_PREFERENCE_ADDED,
    MEMORY_CHANGE_KIND_PREFERENCE_CHANGED,
    MEMORY_CHANGE_KIND_PREFERENCE_REMOVED,
    MEMORY_CHANGE_KIND_NOTE_WRITTEN,
    MEMORY_CHANGE_KIND_NOTE_EDITED,
    MEMORY_CHANGE_KIND_NOTE_REMOVED,
  ];

  static final $core.List<MemoryChangeKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 6);
  static MemoryChangeKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MemoryChangeKind._(super.value, super.name);
}

class MonitorHeld extends $pb.ProtobufEnum {
  static const MonitorHeld MONITOR_HELD_UNSPECIFIED =
      MonitorHeld._(0, _omitEnumNames ? '' : 'MONITOR_HELD_UNSPECIFIED');

  /// Its agent has the Monitors extension turned off.
  static const MonitorHeld MONITOR_HELD_EXTENSION_OFF =
      MonitorHeld._(1, _omitEnumNames ? '' : 'MONITOR_HELD_EXTENSION_OFF');

  /// Its agent's project is archived.
  static const MonitorHeld MONITOR_HELD_PROJECT_ARCHIVED =
      MonitorHeld._(2, _omitEnumNames ? '' : 'MONITOR_HELD_PROJECT_ARCHIVED');

  static const $core.List<MonitorHeld> values = <MonitorHeld>[
    MONITOR_HELD_UNSPECIFIED,
    MONITOR_HELD_EXTENSION_OFF,
    MONITOR_HELD_PROJECT_ARCHIVED,
  ];

  static final $core.List<MonitorHeld?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static MonitorHeld? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MonitorHeld._(super.value, super.name);
}

class MonitorEventKind extends $pb.ProtobufEnum {
  static const MonitorEventKind MONITOR_EVENT_KIND_UNSPECIFIED =
      MonitorEventKind._(
          0, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_UNSPECIFIED');
  static const MonitorEventKind MONITOR_EVENT_KIND_CREATED =
      MonitorEventKind._(1, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_CREATED');
  static const MonitorEventKind MONITOR_EVENT_KIND_FIRED =
      MonitorEventKind._(2, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_FIRED');
  static const MonitorEventKind MONITOR_EVENT_KIND_PAUSED =
      MonitorEventKind._(3, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_PAUSED');
  static const MonitorEventKind MONITOR_EVENT_KIND_RESUMED =
      MonitorEventKind._(4, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_RESUMED');
  static const MonitorEventKind MONITOR_EVENT_KIND_CANCELLED =
      MonitorEventKind._(
          5, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_CANCELLED');
  static const MonitorEventKind MONITOR_EVENT_KIND_DONE =
      MonitorEventKind._(6, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_DONE');
  static const MonitorEventKind MONITOR_EVENT_KIND_ERROR =
      MonitorEventKind._(7, _omitEnumNames ? '' : 'MONITOR_EVENT_KIND_ERROR');

  static const $core.List<MonitorEventKind> values = <MonitorEventKind>[
    MONITOR_EVENT_KIND_UNSPECIFIED,
    MONITOR_EVENT_KIND_CREATED,
    MONITOR_EVENT_KIND_FIRED,
    MONITOR_EVENT_KIND_PAUSED,
    MONITOR_EVENT_KIND_RESUMED,
    MONITOR_EVENT_KIND_CANCELLED,
    MONITOR_EVENT_KIND_DONE,
    MONITOR_EVENT_KIND_ERROR,
  ];

  static final $core.List<MonitorEventKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static MonitorEventKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MonitorEventKind._(super.value, super.name);
}

class MonitorState extends $pb.ProtobufEnum {
  static const MonitorState MONITOR_STATE_UNSPECIFIED =
      MonitorState._(0, _omitEnumNames ? '' : 'MONITOR_STATE_UNSPECIFIED');
  static const MonitorState MONITOR_STATE_ACTIVE =
      MonitorState._(1, _omitEnumNames ? '' : 'MONITOR_STATE_ACTIVE');
  static const MonitorState MONITOR_STATE_PAUSED =
      MonitorState._(2, _omitEnumNames ? '' : 'MONITOR_STATE_PAUSED');

  /// It ended on its own: once, until or max fires.
  static const MonitorState MONITOR_STATE_DONE =
      MonitorState._(3, _omitEnumNames ? '' : 'MONITOR_STATE_DONE');
  static const MonitorState MONITOR_STATE_CANCELLED =
      MonitorState._(4, _omitEnumNames ? '' : 'MONITOR_STATE_CANCELLED');

  /// It stopped working (its command kept failing or couldn't start, its
  /// watch kept exiting or its log grew too big); `reason` says why. It can
  /// be resumed.
  static const MonitorState MONITOR_STATE_ERROR =
      MonitorState._(5, _omitEnumNames ? '' : 'MONITOR_STATE_ERROR');

  static const $core.List<MonitorState> values = <MonitorState>[
    MONITOR_STATE_UNSPECIFIED,
    MONITOR_STATE_ACTIVE,
    MONITOR_STATE_PAUSED,
    MONITOR_STATE_DONE,
    MONITOR_STATE_CANCELLED,
    MONITOR_STATE_ERROR,
  ];

  static final $core.List<MonitorState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static MonitorState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MonitorState._(super.value, super.name);
}

class OnboardingStep extends $pb.ProtobufEnum {
  static const OnboardingStep ONBOARDING_STEP_UNSPECIFIED =
      OnboardingStep._(0, _omitEnumNames ? '' : 'ONBOARDING_STEP_UNSPECIFIED');
  static const OnboardingStep ONBOARDING_STEP_NEEDED =
      OnboardingStep._(1, _omitEnumNames ? '' : 'ONBOARDING_STEP_NEEDED');
  static const OnboardingStep ONBOARDING_STEP_SKIPPED =
      OnboardingStep._(2, _omitEnumNames ? '' : 'ONBOARDING_STEP_SKIPPED');
  static const OnboardingStep ONBOARDING_STEP_DONE =
      OnboardingStep._(3, _omitEnumNames ? '' : 'ONBOARDING_STEP_DONE');

  static const $core.List<OnboardingStep> values = <OnboardingStep>[
    ONBOARDING_STEP_UNSPECIFIED,
    ONBOARDING_STEP_NEEDED,
    ONBOARDING_STEP_SKIPPED,
    ONBOARDING_STEP_DONE,
  ];

  static final $core.List<OnboardingStep?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static OnboardingStep? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const OnboardingStep._(super.value, super.name);
}

class LoginPromptKind extends $pb.ProtobufEnum {
  static const LoginPromptKind LOGIN_PROMPT_KIND_UNSPECIFIED =
      LoginPromptKind._(
          0, _omitEnumNames ? '' : 'LOGIN_PROMPT_KIND_UNSPECIFIED');
  static const LoginPromptKind LOGIN_PROMPT_KIND_TEXT =
      LoginPromptKind._(1, _omitEnumNames ? '' : 'LOGIN_PROMPT_KIND_TEXT');

  /// Hide what's typed (API keys).
  static const LoginPromptKind LOGIN_PROMPT_KIND_SECRET =
      LoginPromptKind._(2, _omitEnumNames ? '' : 'LOGIN_PROMPT_KIND_SECRET');

  /// Pick one of `options`; answer with its id.
  static const LoginPromptKind LOGIN_PROMPT_KIND_SELECT =
      LoginPromptKind._(3, _omitEnumNames ? '' : 'LOGIN_PROMPT_KIND_SELECT');

  /// Paste the authorization code or the full address the browser was
  /// redirected to. Waits next to `LoginFlow.url`: only needed when the
  /// redirect can't reach the backend.
  static const LoginPromptKind LOGIN_PROMPT_KIND_CODE =
      LoginPromptKind._(4, _omitEnumNames ? '' : 'LOGIN_PROMPT_KIND_CODE');

  static const $core.List<LoginPromptKind> values = <LoginPromptKind>[
    LOGIN_PROMPT_KIND_UNSPECIFIED,
    LOGIN_PROMPT_KIND_TEXT,
    LOGIN_PROMPT_KIND_SECRET,
    LOGIN_PROMPT_KIND_SELECT,
    LOGIN_PROMPT_KIND_CODE,
  ];

  static final $core.List<LoginPromptKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static LoginPromptKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const LoginPromptKind._(super.value, super.name);
}

/// What an agent's character shows. The first that applies wins: working or
/// thinking (it's running), no model, failed, news, asleep, idle.
class CharacterMood extends $pb.ProtobufEnum {
  static const CharacterMood CHARACTER_MOOD_UNSPECIFIED =
      CharacterMood._(0, _omitEnumNames ? '' : 'CHARACTER_MOOD_UNSPECIFIED');

  /// Nothing going on; none of the below.
  static const CharacterMood CHARACTER_MOOD_IDLE =
      CharacterMood._(1, _omitEnumNames ? '' : 'CHARACTER_MOOD_IDLE');

  /// A run is active and no tool is running: a model reply, compaction,
  /// retries, setup.
  static const CharacterMood CHARACTER_MOOD_THINKING =
      CharacterMood._(2, _omitEnumNames ? '' : 'CHARACTER_MOOD_THINKING');

  /// A run is active and a tool (not a background task) is running.
  static const CharacterMood CHARACTER_MOOD_WORKING =
      CharacterMood._(3, _omitEnumNames ? '' : 'CHARACTER_MOOD_WORKING');

  /// Not running, and something new since the person last opened it
  /// (Agent.unseen).
  static const CharacterMood CHARACTER_MOOD_NEWS =
      CharacterMood._(4, _omitEnumNames ? '' : 'CHARACTER_MOOD_NEWS');

  /// Not running, and its last run failed (an error it didn't recover from;
  /// not a stop). Shows even while selected or unseen, until it next runs or
  /// is cleared.
  static const CharacterMood CHARACTER_MOOD_FAILED =
      CharacterMood._(5, _omitEnumNames ? '' : 'CHARACTER_MOOD_FAILED');

  /// Not running, and its model can't run (Agent.model_available is false).
  static const CharacterMood CHARACTER_MOOD_NO_MODEL =
      CharacterMood._(6, _omitEnumNames ? '' : 'CHARACTER_MOOD_NO_MODEL');

  /// Idle for 15 minutes or more, or running but waiting out its accounts'
  /// usage limits (Agent.usage_wait). Only a look: it wakes at once for
  /// anything.
  static const CharacterMood CHARACTER_MOOD_ASLEEP =
      CharacterMood._(7, _omitEnumNames ? '' : 'CHARACTER_MOOD_ASLEEP');

  static const $core.List<CharacterMood> values = <CharacterMood>[
    CHARACTER_MOOD_UNSPECIFIED,
    CHARACTER_MOOD_IDLE,
    CHARACTER_MOOD_THINKING,
    CHARACTER_MOOD_WORKING,
    CHARACTER_MOOD_NEWS,
    CHARACTER_MOOD_FAILED,
    CHARACTER_MOOD_NO_MODEL,
    CHARACTER_MOOD_ASLEEP,
  ];

  static final $core.List<CharacterMood?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static CharacterMood? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const CharacterMood._(super.value, super.name);
}

class AgentStatus extends $pb.ProtobufEnum {
  static const AgentStatus AGENT_STATUS_UNSPECIFIED =
      AgentStatus._(0, _omitEnumNames ? '' : 'AGENT_STATUS_UNSPECIFIED');
  static const AgentStatus AGENT_STATUS_IDLE =
      AgentStatus._(1, _omitEnumNames ? '' : 'AGENT_STATUS_IDLE');
  static const AgentStatus AGENT_STATUS_WORKING =
      AgentStatus._(2, _omitEnumNames ? '' : 'AGENT_STATUS_WORKING');

  static const $core.List<AgentStatus> values = <AgentStatus>[
    AGENT_STATUS_UNSPECIFIED,
    AGENT_STATUS_IDLE,
    AGENT_STATUS_WORKING,
  ];

  static final $core.List<AgentStatus?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static AgentStatus? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AgentStatus._(super.value, super.name);
}

class PipeStartKind extends $pb.ProtobufEnum {
  static const PipeStartKind PIPE_START_KIND_UNSPECIFIED =
      PipeStartKind._(0, _omitEnumNames ? '' : 'PIPE_START_KIND_UNSPECIFIED');

  /// Make an invite for someone else to accept.
  static const PipeStartKind PIPE_START_KIND_INVITE =
      PipeStartKind._(1, _omitEnumNames ? '' : 'PIPE_START_KIND_INVITE');

  /// Accept an invite the person was given.
  static const PipeStartKind PIPE_START_KIND_ACCEPT =
      PipeStartKind._(2, _omitEnumNames ? '' : 'PIPE_START_KIND_ACCEPT');

  static const $core.List<PipeStartKind> values = <PipeStartKind>[
    PIPE_START_KIND_UNSPECIFIED,
    PIPE_START_KIND_INVITE,
    PIPE_START_KIND_ACCEPT,
  ];

  static final $core.List<PipeStartKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static PipeStartKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeStartKind._(super.value, super.name);
}

class PipeStartState extends $pb.ProtobufEnum {
  static const PipeStartState PIPE_START_STATE_UNSPECIFIED =
      PipeStartState._(0, _omitEnumNames ? '' : 'PIPE_START_STATE_UNSPECIFIED');
  static const PipeStartState PIPE_START_STATE_WAITING =
      PipeStartState._(1, _omitEnumNames ? '' : 'PIPE_START_STATE_WAITING');

  /// The person made the invite or connected with the code: pipe_id.
  static const PipeStartState PIPE_START_STATE_DONE =
      PipeStartState._(2, _omitEnumNames ? '' : 'PIPE_START_STATE_DONE');

  /// The person closed it, or it waited 24 hours.
  static const PipeStartState PIPE_START_STATE_DISMISSED =
      PipeStartState._(3, _omitEnumNames ? '' : 'PIPE_START_STATE_DISMISSED');

  static const $core.List<PipeStartState> values = <PipeStartState>[
    PIPE_START_STATE_UNSPECIFIED,
    PIPE_START_STATE_WAITING,
    PIPE_START_STATE_DONE,
    PIPE_START_STATE_DISMISSED,
  ];

  static final $core.List<PipeStartState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static PipeStartState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeStartState._(super.value, super.name);
}

class AppGrantState extends $pb.ProtobufEnum {
  static const AppGrantState APP_GRANT_STATE_UNSPECIFIED =
      AppGrantState._(0, _omitEnumNames ? '' : 'APP_GRANT_STATE_UNSPECIFIED');
  static const AppGrantState APP_GRANT_STATE_PENDING =
      AppGrantState._(1, _omitEnumNames ? '' : 'APP_GRANT_STATE_PENDING');

  /// The person approved it and the account service made the grant.
  static const AppGrantState APP_GRANT_STATE_GRANTED =
      AppGrantState._(2, _omitEnumNames ? '' : 'APP_GRANT_STATE_GRANTED');

  /// The person declined it, or cleared Rivet's conversation while it
  /// waited.
  static const AppGrantState APP_GRANT_STATE_DECLINED =
      AppGrantState._(3, _omitEnumNames ? '' : 'APP_GRANT_STATE_DECLINED');

  /// Approved, but it couldn't be made: AppGrantProposal.error says why.
  static const AppGrantState APP_GRANT_STATE_FAILED =
      AppGrantState._(4, _omitEnumNames ? '' : 'APP_GRANT_STATE_FAILED');

  static const $core.List<AppGrantState> values = <AppGrantState>[
    APP_GRANT_STATE_UNSPECIFIED,
    APP_GRANT_STATE_PENDING,
    APP_GRANT_STATE_GRANTED,
    APP_GRANT_STATE_DECLINED,
    APP_GRANT_STATE_FAILED,
  ];

  static final $core.List<AppGrantState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static AppGrantState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AppGrantState._(super.value, super.name);
}

class DevicePairingState extends $pb.ProtobufEnum {
  static const DevicePairingState DEVICE_PAIRING_STATE_UNSPECIFIED =
      DevicePairingState._(
          0, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_UNSPECIFIED');

  /// Its code waits to be scanned or pasted on the device.
  static const DevicePairingState DEVICE_PAIRING_STATE_WAITING =
      DevicePairingState._(
          1, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_WAITING');

  /// A device used the code and waits for the person to allow it: it's in
  /// RemoteAccess.pending_devices, by device_id.
  static const DevicePairingState DEVICE_PAIRING_STATE_ASKING =
      DevicePairingState._(
          2, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_ASKING');

  /// The person allowed it: it's paired.
  static const DevicePairingState DEVICE_PAIRING_STATE_ADDED =
      DevicePairingState._(
          3, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_ADDED');

  /// The person didn't allow the device that used the code.
  static const DevicePairingState DEVICE_PAIRING_STATE_DENIED =
      DevicePairingState._(
          4, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_DENIED');

  /// Cancelled (from the card, Settings → Devices, or the agent) before a
  /// device was allowed with it.
  static const DevicePairingState DEVICE_PAIRING_STATE_CANCELLED =
      DevicePairingState._(
          5, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_CANCELLED');

  /// Its code ran out before a device was allowed with it.
  static const DevicePairingState DEVICE_PAIRING_STATE_EXPIRED =
      DevicePairingState._(
          6, _omitEnumNames ? '' : 'DEVICE_PAIRING_STATE_EXPIRED');

  static const $core.List<DevicePairingState> values = <DevicePairingState>[
    DEVICE_PAIRING_STATE_UNSPECIFIED,
    DEVICE_PAIRING_STATE_WAITING,
    DEVICE_PAIRING_STATE_ASKING,
    DEVICE_PAIRING_STATE_ADDED,
    DEVICE_PAIRING_STATE_DENIED,
    DEVICE_PAIRING_STATE_CANCELLED,
    DEVICE_PAIRING_STATE_EXPIRED,
  ];

  static final $core.List<DevicePairingState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 6);
  static DevicePairingState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const DevicePairingState._(super.value, super.name);
}

class ProposalState extends $pb.ProtobufEnum {
  static const ProposalState PROPOSAL_STATE_UNSPECIFIED =
      ProposalState._(0, _omitEnumNames ? '' : 'PROPOSAL_STATE_UNSPECIFIED');
  static const ProposalState PROPOSAL_STATE_PENDING =
      ProposalState._(1, _omitEnumNames ? '' : 'PROPOSAL_STATE_PENDING');
  static const ProposalState PROPOSAL_STATE_APPROVED =
      ProposalState._(2, _omitEnumNames ? '' : 'PROPOSAL_STATE_APPROVED');
  static const ProposalState PROPOSAL_STATE_DISMISSED =
      ProposalState._(3, _omitEnumNames ? '' : 'PROPOSAL_STATE_DISMISSED');

  static const $core.List<ProposalState> values = <ProposalState>[
    PROPOSAL_STATE_UNSPECIFIED,
    PROPOSAL_STATE_PENDING,
    PROPOSAL_STATE_APPROVED,
    PROPOSAL_STATE_DISMISSED,
  ];

  static final $core.List<ProposalState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static ProposalState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ProposalState._(super.value, super.name);
}

class EventSource extends $pb.ProtobufEnum {
  static const EventSource EVENT_SOURCE_UNSPECIFIED =
      EventSource._(0, _omitEnumNames ? '' : 'EVENT_SOURCE_UNSPECIFIED');
  static const EventSource EVENT_SOURCE_FACTORY =
      EventSource._(1, _omitEnumNames ? '' : 'EVENT_SOURCE_FACTORY');
  static const EventSource EVENT_SOURCE_AGENT =
      EventSource._(2, _omitEnumNames ? '' : 'EVENT_SOURCE_AGENT');
  static const EventSource EVENT_SOURCE_TASK =
      EventSource._(3, _omitEnumNames ? '' : 'EVENT_SOURCE_TASK');

  /// One of the agent's monitors fired: `from` is its name, `from_id` its id
  /// ("m3").
  static const EventSource EVENT_SOURCE_MONITOR =
      EventSource._(4, _omitEnumNames ? '' : 'EVENT_SOURCE_MONITOR');

  /// An app (see AppInfo): `from` is its name, `from_id` its id.
  static const EventSource EVENT_SOURCE_APP =
      EventSource._(5, _omitEnumNames ? '' : 'EVENT_SOURCE_APP');

  /// A pipe's messages for its lead (see Pipe): `from` is the pipe's name
  /// and the other Claktory's, `from_id` "pipe:<Pipe.id>".
  static const EventSource EVENT_SOURCE_PIPE =
      EventSource._(6, _omitEnumNames ? '' : 'EVENT_SOURCE_PIPE');

  static const $core.List<EventSource> values = <EventSource>[
    EVENT_SOURCE_UNSPECIFIED,
    EVENT_SOURCE_FACTORY,
    EVENT_SOURCE_AGENT,
    EVENT_SOURCE_TASK,
    EVENT_SOURCE_MONITOR,
    EVENT_SOURCE_APP,
    EVENT_SOURCE_PIPE,
  ];

  static final $core.List<EventSource?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 6);
  static EventSource? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const EventSource._(super.value, super.name);
}

class TranscriptionState extends $pb.ProtobufEnum {
  static const TranscriptionState TRANSCRIPTION_STATE_UNSPECIFIED =
      TranscriptionState._(
          0, _omitEnumNames ? '' : 'TRANSCRIPTION_STATE_UNSPECIFIED');

  /// Waiting for the speech model (Workspace.speech_model shows its
  /// download) or being transcribed. The person's messages after it wait
  /// behind it, so the agent reads them in the order they were sent; other
  /// events don't.
  static const TranscriptionState TRANSCRIPTION_STATE_PENDING =
      TranscriptionState._(
          1, _omitEnumNames ? '' : 'TRANSCRIPTION_STATE_PENDING');
  static const TranscriptionState TRANSCRIPTION_STATE_DONE =
      TranscriptionState._(2, _omitEnumNames ? '' : 'TRANSCRIPTION_STATE_DONE');

  /// Not delivered, and not holding up the queue: RetryVoiceMessage or
  /// DiscardVoiceMessage.
  static const TranscriptionState TRANSCRIPTION_STATE_FAILED =
      TranscriptionState._(
          3, _omitEnumNames ? '' : 'TRANSCRIPTION_STATE_FAILED');

  static const $core.List<TranscriptionState> values = <TranscriptionState>[
    TRANSCRIPTION_STATE_UNSPECIFIED,
    TRANSCRIPTION_STATE_PENDING,
    TRANSCRIPTION_STATE_DONE,
    TRANSCRIPTION_STATE_FAILED,
  ];

  static final $core.List<TranscriptionState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static TranscriptionState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const TranscriptionState._(super.value, super.name);
}

class SpeechModelState extends $pb.ProtobufEnum {
  static const SpeechModelState SPEECH_MODEL_STATE_UNSPECIFIED =
      SpeechModelState._(
          0, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_UNSPECIFIED');

  /// Not downloaded: InstallSpeechModel, after asking the person (it's big:
  /// `total`).
  static const SpeechModelState SPEECH_MODEL_STATE_ABSENT =
      SpeechModelState._(1, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_ABSENT');
  static const SpeechModelState SPEECH_MODEL_STATE_DOWNLOADING =
      SpeechModelState._(
          2, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_DOWNLOADING');

  /// Downloaded; its checksum is being checked.
  static const SpeechModelState SPEECH_MODEL_STATE_VERIFYING =
      SpeechModelState._(
          3, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_VERIFYING');
  static const SpeechModelState SPEECH_MODEL_STATE_READY =
      SpeechModelState._(4, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_READY');

  /// The download failed (`error`); InstallSpeechModel tries again, from
  /// where it stopped.
  static const SpeechModelState SPEECH_MODEL_STATE_FAILED =
      SpeechModelState._(5, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_FAILED');

  /// This computer can't run speech recognition (`error`): grey the
  /// microphone out.
  static const SpeechModelState SPEECH_MODEL_STATE_UNSUPPORTED =
      SpeechModelState._(
          6, _omitEnumNames ? '' : 'SPEECH_MODEL_STATE_UNSUPPORTED');

  static const $core.List<SpeechModelState> values = <SpeechModelState>[
    SPEECH_MODEL_STATE_UNSPECIFIED,
    SPEECH_MODEL_STATE_ABSENT,
    SPEECH_MODEL_STATE_DOWNLOADING,
    SPEECH_MODEL_STATE_VERIFYING,
    SPEECH_MODEL_STATE_READY,
    SPEECH_MODEL_STATE_FAILED,
    SPEECH_MODEL_STATE_UNSUPPORTED,
  ];

  static final $core.List<SpeechModelState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 6);
  static SpeechModelState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const SpeechModelState._(super.value, super.name);
}

class ToolCallState extends $pb.ProtobufEnum {
  static const ToolCallState TOOL_CALL_STATE_UNSPECIFIED =
      ToolCallState._(0, _omitEnumNames ? '' : 'TOOL_CALL_STATE_UNSPECIFIED');

  /// The model is still writing the call.
  static const ToolCallState TOOL_CALL_STATE_PENDING =
      ToolCallState._(1, _omitEnumNames ? '' : 'TOOL_CALL_STATE_PENDING');
  static const ToolCallState TOOL_CALL_STATE_RUNNING =
      ToolCallState._(2, _omitEnumNames ? '' : 'TOOL_CALL_STATE_RUNNING');
  static const ToolCallState TOOL_CALL_STATE_DONE =
      ToolCallState._(3, _omitEnumNames ? '' : 'TOOL_CALL_STATE_DONE');
  static const ToolCallState TOOL_CALL_STATE_FAILED =
      ToolCallState._(4, _omitEnumNames ? '' : 'TOOL_CALL_STATE_FAILED');

  static const $core.List<ToolCallState> values = <ToolCallState>[
    TOOL_CALL_STATE_UNSPECIFIED,
    TOOL_CALL_STATE_PENDING,
    TOOL_CALL_STATE_RUNNING,
    TOOL_CALL_STATE_DONE,
    TOOL_CALL_STATE_FAILED,
  ];

  static final $core.List<ToolCallState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static ToolCallState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ToolCallState._(super.value, super.name);
}

class NoticeLevel extends $pb.ProtobufEnum {
  static const NoticeLevel NOTICE_LEVEL_UNSPECIFIED =
      NoticeLevel._(0, _omitEnumNames ? '' : 'NOTICE_LEVEL_UNSPECIFIED');
  static const NoticeLevel NOTICE_LEVEL_INFO =
      NoticeLevel._(1, _omitEnumNames ? '' : 'NOTICE_LEVEL_INFO');
  static const NoticeLevel NOTICE_LEVEL_ERROR =
      NoticeLevel._(2, _omitEnumNames ? '' : 'NOTICE_LEVEL_ERROR');

  static const $core.List<NoticeLevel> values = <NoticeLevel>[
    NOTICE_LEVEL_UNSPECIFIED,
    NOTICE_LEVEL_INFO,
    NOTICE_LEVEL_ERROR,
  ];

  static final $core.List<NoticeLevel?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static NoticeLevel? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const NoticeLevel._(super.value, super.name);
}

class SecretRequestState extends $pb.ProtobufEnum {
  static const SecretRequestState SECRET_REQUEST_STATE_UNSPECIFIED =
      SecretRequestState._(
          0, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_UNSPECIFIED');
  static const SecretRequestState SECRET_REQUEST_STATE_PENDING =
      SecretRequestState._(
          1, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_PENDING');
  static const SecretRequestState SECRET_REQUEST_STATE_GRANTED =
      SecretRequestState._(
          2, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_GRANTED');
  static const SecretRequestState SECRET_REQUEST_STATE_DISMISSED =
      SecretRequestState._(
          3, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_DISMISSED');

  /// Left unanswered 12 hours since it was made or last changed: it can't be
  /// answered any more, and the agent was told. decided_ms says when.
  static const SecretRequestState SECRET_REQUEST_STATE_EXPIRED =
      SecretRequestState._(
          4, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_EXPIRED');

  /// The agent took it back (secrets_request_cancel): it can't be answered
  /// any more. decided_ms says when, and `comment` holds the agent's reason,
  /// if it gave one.
  static const SecretRequestState SECRET_REQUEST_STATE_WITHDRAWN =
      SecretRequestState._(
          5, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_WITHDRAWN');

  /// The person approved the need (DecideSecretRequest.approve_only) and the
  /// admin agent (`admin_agent_id`) finds a matching secret or collects a
  /// new one. The card keeps its direct actions (give a secret, add one,
  /// dismiss), so the person is never stuck on the admin agent. It doesn't
  /// expire: only PENDING requests do.
  static const SecretRequestState SECRET_REQUEST_STATE_APPROVED =
      SecretRequestState._(
          6, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_APPROVED');

  /// A generate request (SecretRequest.generate) decided: the vault made the
  /// key and gave it to no one, as the agent asked. given_secret_id and
  /// given_secret_name name it, public_key is its public half.
  static const SecretRequestState SECRET_REQUEST_STATE_GENERATED =
      SecretRequestState._(
          7, _omitEnumNames ? '' : 'SECRET_REQUEST_STATE_GENERATED');

  static const $core.List<SecretRequestState> values = <SecretRequestState>[
    SECRET_REQUEST_STATE_UNSPECIFIED,
    SECRET_REQUEST_STATE_PENDING,
    SECRET_REQUEST_STATE_GRANTED,
    SECRET_REQUEST_STATE_DISMISSED,
    SECRET_REQUEST_STATE_EXPIRED,
    SECRET_REQUEST_STATE_WITHDRAWN,
    SECRET_REQUEST_STATE_APPROVED,
    SECRET_REQUEST_STATE_GENERATED,
  ];

  static final $core.List<SecretRequestState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static SecretRequestState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const SecretRequestState._(super.value, super.name);
}

class ClaktoryAccountState extends $pb.ProtobufEnum {
  static const ClaktoryAccountState CLAKTORY_ACCOUNT_STATE_UNSPECIFIED =
      ClaktoryAccountState._(
          0, _omitEnumNames ? '' : 'CLAKTORY_ACCOUNT_STATE_UNSPECIFIED');

  /// Not signed in: AccountSignIn.
  static const ClaktoryAccountState CLAKTORY_ACCOUNT_STATE_SIGNED_OUT =
      ClaktoryAccountState._(
          1, _omitEnumNames ? '' : 'CLAKTORY_ACCOUNT_STATE_SIGNED_OUT');

  /// A code was emailed: AccountVerify, or AccountResend.
  static const ClaktoryAccountState CLAKTORY_ACCOUNT_STATE_AWAITING_CODE =
      ClaktoryAccountState._(
          2, _omitEnumNames ? '' : 'CLAKTORY_ACCOUNT_STATE_AWAITING_CODE');
  static const ClaktoryAccountState CLAKTORY_ACCOUNT_STATE_SIGNED_IN =
      ClaktoryAccountState._(
          3, _omitEnumNames ? '' : 'CLAKTORY_ACCOUNT_STATE_SIGNED_IN');

  /// Removed at claktory.com: it keeps running, but after a restart its
  /// secrets open only with the recovery key, until it signs in again.
  static const ClaktoryAccountState CLAKTORY_ACCOUNT_STATE_REVOKED =
      ClaktoryAccountState._(
          4, _omitEnumNames ? '' : 'CLAKTORY_ACCOUNT_STATE_REVOKED');

  /// Not signed in, and the person chose Not now on the sign-in step
  /// (SkipAccountSignIn): the app doesn't ask again. Settings → Account
  /// offers to sign in, as from SIGNED_OUT (AccountSignIn works from here).
  static const ClaktoryAccountState CLAKTORY_ACCOUNT_STATE_SKIPPED =
      ClaktoryAccountState._(
          5, _omitEnumNames ? '' : 'CLAKTORY_ACCOUNT_STATE_SKIPPED');

  static const $core.List<ClaktoryAccountState> values = <ClaktoryAccountState>[
    CLAKTORY_ACCOUNT_STATE_UNSPECIFIED,
    CLAKTORY_ACCOUNT_STATE_SIGNED_OUT,
    CLAKTORY_ACCOUNT_STATE_AWAITING_CODE,
    CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
    CLAKTORY_ACCOUNT_STATE_REVOKED,
    CLAKTORY_ACCOUNT_STATE_SKIPPED,
  ];

  static final $core.List<ClaktoryAccountState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static ClaktoryAccountState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ClaktoryAccountState._(super.value, super.name);
}

class MapNodeKind extends $pb.ProtobufEnum {
  static const MapNodeKind MAP_NODE_KIND_UNSPECIFIED =
      MapNodeKind._(0, _omitEnumNames ? '' : 'MAP_NODE_KIND_UNSPECIFIED');

  /// The workspace folder: agents in no project.
  static const MapNodeKind MAP_NODE_KIND_WORKSPACE =
      MapNodeKind._(1, _omitEnumNames ? '' : 'MAP_NODE_KIND_WORKSPACE');
  static const MapNodeKind MAP_NODE_KIND_PROJECT =
      MapNodeKind._(2, _omitEnumNames ? '' : 'MAP_NODE_KIND_PROJECT');

  /// One of a project's folders.
  static const MapNodeKind MAP_NODE_KIND_FOLDER =
      MapNodeKind._(3, _omitEnumNames ? '' : 'MAP_NODE_KIND_FOLDER');
  static const MapNodeKind MAP_NODE_KIND_AGENT =
      MapNodeKind._(4, _omitEnumNames ? '' : 'MAP_NODE_KIND_AGENT');

  /// A secret in the vault (its name, never values).
  static const MapNodeKind MAP_NODE_KIND_SECRET =
      MapNodeKind._(5, _omitEnumNames ? '' : 'MAP_NODE_KIND_SECRET');

  /// A database: an agent's own, or Claktory's records.
  static const MapNodeKind MAP_NODE_KIND_DATA =
      MapNodeKind._(6, _omitEnumNames ? '' : 'MAP_NODE_KIND_DATA');

  /// An app installed (see AppInfo): label its name, detail its description
  /// (or why it's in trouble), ref_id its id; at the top.
  static const MapNodeKind MAP_NODE_KIND_APP =
      MapNodeKind._(7, _omitEnumNames ? '' : 'MAP_NODE_KIND_APP');

  static const $core.List<MapNodeKind> values = <MapNodeKind>[
    MAP_NODE_KIND_UNSPECIFIED,
    MAP_NODE_KIND_WORKSPACE,
    MAP_NODE_KIND_PROJECT,
    MAP_NODE_KIND_FOLDER,
    MAP_NODE_KIND_AGENT,
    MAP_NODE_KIND_SECRET,
    MAP_NODE_KIND_DATA,
    MAP_NODE_KIND_APP,
  ];

  static final $core.List<MapNodeKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static MapNodeKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MapNodeKind._(super.value, super.name);
}

class MapEdgeKind extends $pb.ProtobufEnum {
  static const MapEdgeKind MAP_EDGE_KIND_UNSPECIFIED =
      MapEdgeKind._(0, _omitEnumNames ? '' : 'MAP_EDGE_KIND_UNSPECIFIED');

  /// An agent can message another (and see it): same project, its "also"
  /// list, or an admin agent.
  static const MapEdgeKind MAP_EDGE_KIND_MESSAGES =
      MapEdgeKind._(1, _omitEnumNames ? '' : 'MAP_EDGE_KIND_MESSAGES');

  /// From a secret to an agent that has it, or to an app.
  static const MapEdgeKind MAP_EDGE_KIND_SECRET =
      MapEdgeKind._(2, _omitEnumNames ? '' : 'MAP_EDGE_KIND_SECRET');

  /// From an agent to a folder of its project it can read but not change
  /// (the project's own folder is the project's node), or to Claktory's
  /// records.
  static const MapEdgeKind MAP_EDGE_KIND_READS =
      MapEdgeKind._(3, _omitEnumNames ? '' : 'MAP_EDGE_KIND_READS');

  /// From an agent to the folder it works in (the workspace's, a project's
  /// or a project folder's node), or to its own database.
  static const MapEdgeKind MAP_EDGE_KIND_WRITES =
      MapEdgeKind._(4, _omitEnumNames ? '' : 'MAP_EDGE_KIND_WRITES');

  /// From an app to an agent given scopes of it.
  static const MapEdgeKind MAP_EDGE_KIND_APP =
      MapEdgeKind._(5, _omitEnumNames ? '' : 'MAP_EDGE_KIND_APP');

  /// From an app to an agent that hears its events.
  static const MapEdgeKind MAP_EDGE_KIND_HEARS =
      MapEdgeKind._(6, _omitEnumNames ? '' : 'MAP_EDGE_KIND_HEARS');

  /// From an agent to its lead (Agent.lead_id). Only leads set in agents'
  /// settings: an admin agent leading everyone isn't drawn.
  static const MapEdgeKind MAP_EDGE_KIND_LEAD =
      MapEdgeKind._(7, _omitEnumNames ? '' : 'MAP_EDGE_KIND_LEAD');

  static const $core.List<MapEdgeKind> values = <MapEdgeKind>[
    MAP_EDGE_KIND_UNSPECIFIED,
    MAP_EDGE_KIND_MESSAGES,
    MAP_EDGE_KIND_SECRET,
    MAP_EDGE_KIND_READS,
    MAP_EDGE_KIND_WRITES,
    MAP_EDGE_KIND_APP,
    MAP_EDGE_KIND_HEARS,
    MAP_EDGE_KIND_LEAD,
  ];

  static final $core.List<MapEdgeKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static MapEdgeKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MapEdgeKind._(super.value, super.name);
}

class ConnectorFlowKind extends $pb.ProtobufEnum {
  static const ConnectorFlowKind CONNECTOR_FLOW_KIND_UNSPECIFIED =
      ConnectorFlowKind._(
          0, _omitEnumNames ? '' : 'CONNECTOR_FLOW_KIND_UNSPECIFIED');

  /// The app opens the service's page and catches where the browser comes
  /// back (FinishConnect).
  static const ConnectorFlowKind CONNECTOR_FLOW_KIND_BROWSER =
      ConnectorFlowKind._(
          1, _omitEnumNames ? '' : 'CONNECTOR_FLOW_KIND_BROWSER');

  /// The app opens a page with a code filled in; the backend asks the service
  /// until the person approves. No FinishConnect.
  static const ConnectorFlowKind CONNECTOR_FLOW_KIND_DEVICE =
      ConnectorFlowKind._(
          2, _omitEnumNames ? '' : 'CONNECTOR_FLOW_KIND_DEVICE');

  /// The app opens the service's page that makes a token (ConnectStarted.url)
  /// and asks the person to paste the token it shows, sent in
  /// FinishConnect.token. The app never shows it again nor keeps it.
  static const ConnectorFlowKind CONNECTOR_FLOW_KIND_PASTE =
      ConnectorFlowKind._(3, _omitEnumNames ? '' : 'CONNECTOR_FLOW_KIND_PASTE');

  static const $core.List<ConnectorFlowKind> values = <ConnectorFlowKind>[
    CONNECTOR_FLOW_KIND_UNSPECIFIED,
    CONNECTOR_FLOW_KIND_BROWSER,
    CONNECTOR_FLOW_KIND_DEVICE,
    CONNECTOR_FLOW_KIND_PASTE,
  ];

  static final $core.List<ConnectorFlowKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static ConnectorFlowKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ConnectorFlowKind._(super.value, super.name);
}

class ConnectProposalState extends $pb.ProtobufEnum {
  static const ConnectProposalState CONNECT_PROPOSAL_STATE_UNSPECIFIED =
      ConnectProposalState._(
          0, _omitEnumNames ? '' : 'CONNECT_PROPOSAL_STATE_UNSPECIFIED');
  static const ConnectProposalState CONNECT_PROPOSAL_STATE_PENDING =
      ConnectProposalState._(
          1, _omitEnumNames ? '' : 'CONNECT_PROPOSAL_STATE_PENDING');
  static const ConnectProposalState CONNECT_PROPOSAL_STATE_CONNECTED =
      ConnectProposalState._(
          2, _omitEnumNames ? '' : 'CONNECT_PROPOSAL_STATE_CONNECTED');
  static const ConnectProposalState CONNECT_PROPOSAL_STATE_DISMISSED =
      ConnectProposalState._(
          3, _omitEnumNames ? '' : 'CONNECT_PROPOSAL_STATE_DISMISSED');

  static const $core.List<ConnectProposalState> values = <ConnectProposalState>[
    CONNECT_PROPOSAL_STATE_UNSPECIFIED,
    CONNECT_PROPOSAL_STATE_PENDING,
    CONNECT_PROPOSAL_STATE_CONNECTED,
    CONNECT_PROPOSAL_STATE_DISMISSED,
  ];

  static final $core.List<ConnectProposalState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static ConnectProposalState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ConnectProposalState._(super.value, super.name);
}

class ConnectFlowState extends $pb.ProtobufEnum {
  static const ConnectFlowState CONNECT_FLOW_STATE_UNSPECIFIED =
      ConnectFlowState._(
          0, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_UNSPECIFIED');

  /// Waiting for the person, in the browser (PASTE: for the token; `error`
  /// says why the last one pasted didn't work).
  static const ConnectFlowState CONNECT_FLOW_STATE_WAITING =
      ConnectFlowState._(1, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_WAITING');

  /// Signing in with what the browser brought back (PASTE: checking the
  /// token with the service).
  static const ConnectFlowState CONNECT_FLOW_STATE_EXCHANGING =
      ConnectFlowState._(
          2, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_EXCHANGING');

  /// Reading the account and what was granted.
  static const ConnectFlowState CONNECT_FLOW_STATE_CHECKING =
      ConnectFlowState._(
          3, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_CHECKING');
  static const ConnectFlowState CONNECT_FLOW_STATE_CONNECTED =
      ConnectFlowState._(
          4, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_CONNECTED');
  static const ConnectFlowState CONNECT_FLOW_STATE_FAILED =
      ConnectFlowState._(5, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_FAILED');
  static const ConnectFlowState CONNECT_FLOW_STATE_EXPIRED =
      ConnectFlowState._(6, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_EXPIRED');
  static const ConnectFlowState CONNECT_FLOW_STATE_CANCELLED =
      ConnectFlowState._(
          7, _omitEnumNames ? '' : 'CONNECT_FLOW_STATE_CANCELLED');

  static const $core.List<ConnectFlowState> values = <ConnectFlowState>[
    CONNECT_FLOW_STATE_UNSPECIFIED,
    CONNECT_FLOW_STATE_WAITING,
    CONNECT_FLOW_STATE_EXCHANGING,
    CONNECT_FLOW_STATE_CHECKING,
    CONNECT_FLOW_STATE_CONNECTED,
    CONNECT_FLOW_STATE_FAILED,
    CONNECT_FLOW_STATE_EXPIRED,
    CONNECT_FLOW_STATE_CANCELLED,
  ];

  static final $core.List<ConnectFlowState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static ConnectFlowState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ConnectFlowState._(super.value, super.name);
}

class ConnectionStatus extends $pb.ProtobufEnum {
  static const ConnectionStatus CONNECTION_STATUS_UNSPECIFIED =
      ConnectionStatus._(
          0, _omitEnumNames ? '' : 'CONNECTION_STATUS_UNSPECIFIED');
  static const ConnectionStatus CONNECTION_STATUS_READY =
      ConnectionStatus._(1, _omitEnumNames ? '' : 'CONNECTION_STATUS_READY');

  /// The service no longer accepts it: connect again (StartConnect.secret_id).
  static const ConnectionStatus CONNECTION_STATUS_NEEDS_RECONNECT =
      ConnectionStatus._(
          2, _omitEnumNames ? '' : 'CONNECTION_STATUS_NEEDS_RECONNECT');

  static const $core.List<ConnectionStatus> values = <ConnectionStatus>[
    CONNECTION_STATUS_UNSPECIFIED,
    CONNECTION_STATUS_READY,
    CONNECTION_STATUS_NEEDS_RECONNECT,
  ];

  static final $core.List<ConnectionStatus?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static ConnectionStatus? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ConnectionStatus._(super.value, super.name);
}

class AppDataKind extends $pb.ProtobufEnum {
  static const AppDataKind APP_DATA_KIND_UNSPECIFIED =
      AppDataKind._(0, _omitEnumNames ? '' : 'APP_DATA_KIND_UNSPECIFIED');

  /// Claktory's records (factory.db): agents, runs, usage, messages, tasks…
  static const AppDataKind APP_DATA_KIND_FACTORY_RECORDS =
      AppDataKind._(1, _omitEnumNames ? '' : 'APP_DATA_KIND_FACTORY_RECORDS');

  /// An agent's own database (its Database extension's).
  static const AppDataKind APP_DATA_KIND_AGENT_DATABASE =
      AppDataKind._(2, _omitEnumNames ? '' : 'APP_DATA_KIND_AGENT_DATABASE');

  /// Activity: runs, background tasks, monitors, checklist items, messages
  /// and token use, as ids, agents, times, states and counts; never their
  /// text.
  static const AppDataKind APP_DATA_KIND_ACTIVITY =
      AppDataKind._(3, _omitEnumNames ? '' : 'APP_DATA_KIND_ACTIVITY');

  /// Conversations: the messages your agents got, from you, other agents
  /// and events, with their text.
  static const AppDataKind APP_DATA_KIND_CONVERSATIONS =
      AppDataKind._(4, _omitEnumNames ? '' : 'APP_DATA_KIND_CONVERSATIONS');

  /// A CSV or TSV file the person (or Rivet, on a card) chose, read as one
  /// table, `rows`, at every query. AppDataSource.id is
  /// "file:<place>:<path>": the place "workspace", a project's id or
  /// "<project id>/<folder id>", the path in that folder with "/" between
  /// names (no name starting with "."), such as
  /// "file:web-site:reports/bugs.csv"; label "reports/bugs.csv in Web Site",
  /// holds the file's full path. Another path is another source. In
  /// AppPermissions, one entry with id "file:*" says the app reads files
  /// the person chooses. Give one with GrantAppSource (source_id the id):
  /// it's refused, saying why, when the file can't be read by an app (a
  /// symbolic link or a hidden name in its path, inside Claktory's own
  /// folder but its workspace, not a regular file, several hard links, over
  /// 10 MB).
  static const AppDataKind APP_DATA_KIND_FILE =
      AppDataKind._(5, _omitEnumNames ? '' : 'APP_DATA_KIND_FILE');

  static const $core.List<AppDataKind> values = <AppDataKind>[
    APP_DATA_KIND_UNSPECIFIED,
    APP_DATA_KIND_FACTORY_RECORDS,
    APP_DATA_KIND_AGENT_DATABASE,
    APP_DATA_KIND_ACTIVITY,
    APP_DATA_KIND_CONVERSATIONS,
    APP_DATA_KIND_FILE,
  ];

  static final $core.List<AppDataKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static AppDataKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AppDataKind._(super.value, super.name);
}

class AppSurfaceState extends $pb.ProtobufEnum {
  static const AppSurfaceState APP_SURFACE_STATE_UNSPECIFIED =
      AppSurfaceState._(
          0, _omitEnumNames ? '' : 'APP_SURFACE_STATE_UNSPECIFIED');

  /// The app is making it.
  static const AppSurfaceState APP_SURFACE_STATE_OPENING =
      AppSurfaceState._(1, _omitEnumNames ? '' : 'APP_SURFACE_STATE_OPENING');
  static const AppSurfaceState APP_SURFACE_STATE_OPEN =
      AppSurfaceState._(2, _omitEnumNames ? '' : 'APP_SURFACE_STATE_OPEN');

  /// The app isn't running (turned off, crashed, updating): it opens again
  /// by itself when the app runs again.
  static const AppSurfaceState APP_SURFACE_STATE_STOPPED =
      AppSurfaceState._(3, _omitEnumNames ? '' : 'APP_SURFACE_STATE_STOPPED');

  /// The app sent something invalid, or couldn't open it (`error` says
  /// what). It stays so: OpenAppView again opens a new surface.
  static const AppSurfaceState APP_SURFACE_STATE_FAILED =
      AppSurfaceState._(4, _omitEnumNames ? '' : 'APP_SURFACE_STATE_FAILED');

  /// The app closed it, or was uninstalled. OpenAppView again for a new one.
  static const AppSurfaceState APP_SURFACE_STATE_CLOSED =
      AppSurfaceState._(5, _omitEnumNames ? '' : 'APP_SURFACE_STATE_CLOSED');

  /// This app doesn't render the surface's catalog: only in the answer to
  /// OpenAppView, which opened nothing.
  static const AppSurfaceState APP_SURFACE_STATE_UNSUPPORTED =
      AppSurfaceState._(
          6, _omitEnumNames ? '' : 'APP_SURFACE_STATE_UNSUPPORTED');

  static const $core.List<AppSurfaceState> values = <AppSurfaceState>[
    APP_SURFACE_STATE_UNSPECIFIED,
    APP_SURFACE_STATE_OPENING,
    APP_SURFACE_STATE_OPEN,
    APP_SURFACE_STATE_STOPPED,
    APP_SURFACE_STATE_FAILED,
    APP_SURFACE_STATE_CLOSED,
    APP_SURFACE_STATE_UNSUPPORTED,
  ];

  static final $core.List<AppSurfaceState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 6);
  static AppSurfaceState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AppSurfaceState._(super.value, super.name);
}

class AppTier extends $pb.ProtobufEnum {
  static const AppTier APP_TIER_UNSPECIFIED =
      AppTier._(0, _omitEnumNames ? '' : 'APP_TIER_UNSPECIFIED');

  /// Made by Claktory: signed with Claktory's key.
  static const AppTier APP_TIER_CLAKTORY =
      AppTier._(1, _omitEnumNames ? '' : 'APP_TIER_CLAKTORY');
  static const AppTier APP_TIER_VERIFIED =
      AppTier._(2, _omitEnumNames ? '' : 'APP_TIER_VERIFIED');
  static const AppTier APP_TIER_COMMUNITY =
      AppTier._(3, _omitEnumNames ? '' : 'APP_TIER_COMMUNITY');

  /// Installed from a file, signed by its own developer's key.
  static const AppTier APP_TIER_LOCAL =
      AppTier._(4, _omitEnumNames ? '' : 'APP_TIER_LOCAL');

  static const $core.List<AppTier> values = <AppTier>[
    APP_TIER_UNSPECIFIED,
    APP_TIER_CLAKTORY,
    APP_TIER_VERIFIED,
    APP_TIER_COMMUNITY,
    APP_TIER_LOCAL,
  ];

  static final $core.List<AppTier?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static AppTier? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AppTier._(super.value, super.name);
}

class AppState extends $pb.ProtobufEnum {
  static const AppState APP_STATE_UNSPECIFIED =
      AppState._(0, _omitEnumNames ? '' : 'APP_STATE_UNSPECIFIED');

  /// Turned off (SetAppRunning).
  static const AppState APP_STATE_STOPPED =
      AppState._(1, _omitEnumNames ? '' : 'APP_STATE_STOPPED');
  static const AppState APP_STATE_STARTING =
      AppState._(2, _omitEnumNames ? '' : 'APP_STATE_STARTING');
  static const AppState APP_STATE_RUNNING =
      AppState._(3, _omitEnumNames ? '' : 'APP_STATE_RUNNING');

  /// Waiting for something in `setup`; it starts once that's done.
  static const AppState APP_STATE_NEEDS_SETUP =
      AppState._(4, _omitEnumNames ? '' : 'APP_STATE_NEEDS_SETUP');

  /// It crashed three times in ten minutes: off until the person starts it
  /// again (`state_detail` says why).
  static const AppState APP_STATE_PAUSED_CRASHED =
      AppState._(5, _omitEnumNames ? '' : 'APP_STATE_PAUSED_CRASHED');

  /// It crashed and starts again shortly.
  static const AppState APP_STATE_RESTARTING =
      AppState._(6, _omitEnumNames ? '' : 'APP_STATE_RESTARTING');

  /// Claktory can't run it here: the apps host is missing or failed
  /// (`state_detail` says why).
  static const AppState APP_STATE_UNAVAILABLE =
      AppState._(7, _omitEnumNames ? '' : 'APP_STATE_UNAVAILABLE');

  static const $core.List<AppState> values = <AppState>[
    APP_STATE_UNSPECIFIED,
    APP_STATE_STOPPED,
    APP_STATE_STARTING,
    APP_STATE_RUNNING,
    APP_STATE_NEEDS_SETUP,
    APP_STATE_PAUSED_CRASHED,
    APP_STATE_RESTARTING,
    APP_STATE_UNAVAILABLE,
  ];

  static final $core.List<AppState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static AppState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AppState._(super.value, super.name);
}

class AppSetupKind extends $pb.ProtobufEnum {
  static const AppSetupKind APP_SETUP_KIND_UNSPECIFIED =
      AppSetupKind._(0, _omitEnumNames ? '' : 'APP_SETUP_KIND_UNSPECIFIED');

  /// A secret to paste: SetAppSecret.
  static const AppSetupKind APP_SETUP_KIND_SECRET =
      AppSetupKind._(1, _omitEnumNames ? '' : 'APP_SETUP_KIND_SECRET');

  static const $core.List<AppSetupKind> values = <AppSetupKind>[
    APP_SETUP_KIND_UNSPECIFIED,
    APP_SETUP_KIND_SECRET,
  ];

  static final $core.List<AppSetupKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 1);
  static AppSetupKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AppSetupKind._(super.value, super.name);
}

class PipeState extends $pb.ProtobufEnum {
  static const PipeState PIPE_STATE_UNSPECIFIED =
      PipeState._(0, _omitEnumNames ? '' : 'PIPE_STATE_UNSPECIFIED');

  /// This side made an invite; nobody has used its code yet.
  static const PipeState PIPE_STATE_INVITED =
      PipeState._(1, _omitEnumNames ? '' : 'PIPE_STATE_INVITED');

  /// This side used a code and is reaching the other Claktory.
  static const PipeState PIPE_STATE_JOINING =
      PipeState._(2, _omitEnumNames ? '' : 'PIPE_STATE_JOINING');

  /// Both Claktories are connected: waiting for this person to approve
  /// (ApprovePipe), checking `digits`.
  static const PipeState PIPE_STATE_APPROVAL =
      PipeState._(3, _omitEnumNames ? '' : 'PIPE_STATE_APPROVAL');

  /// This person approved; waiting for the other's.
  static const PipeState PIPE_STATE_WAITING_PEER =
      PipeState._(4, _omitEnumNames ? '' : 'PIPE_STATE_WAITING_PEER');

  /// Both approved: messages flow.
  static const PipeState PIPE_STATE_OPEN =
      PipeState._(5, _omitEnumNames ? '' : 'PIPE_STATE_OPEN');

  /// Nothing flows until it's resumed; `paused` says why.
  static const PipeState PIPE_STATE_PAUSED =
      PipeState._(6, _omitEnumNames ? '' : 'PIPE_STATE_PAUSED');

  /// Closed by either side, denied, or its invite expired: it keeps its
  /// history until deleted, and can't open again.
  static const PipeState PIPE_STATE_CLOSED =
      PipeState._(7, _omitEnumNames ? '' : 'PIPE_STATE_CLOSED');

  static const $core.List<PipeState> values = <PipeState>[
    PIPE_STATE_UNSPECIFIED,
    PIPE_STATE_INVITED,
    PIPE_STATE_JOINING,
    PIPE_STATE_APPROVAL,
    PIPE_STATE_WAITING_PEER,
    PIPE_STATE_OPEN,
    PIPE_STATE_PAUSED,
    PIPE_STATE_CLOSED,
  ];

  static final $core.List<PipeState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static PipeState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeState._(super.value, super.name);
}

class PipePause extends $pb.ProtobufEnum {
  static const PipePause PIPE_PAUSE_UNSPECIFIED =
      PipePause._(0, _omitEnumNames ? '' : 'PIPE_PAUSE_UNSPECIFIED');

  /// This person paused it.
  static const PipePause PIPE_PAUSE_BY_ME =
      PipePause._(1, _omitEnumNames ? '' : 'PIPE_PAUSE_BY_ME');

  /// The other side paused it: only they can resume.
  static const PipePause PIPE_PAUSE_BY_PEER =
      PipePause._(2, _omitEnumNames ? '' : 'PIPE_PAUSE_BY_PEER');

  /// The loop breaker: agents exchanged 10 messages with no person writing
  /// on either side, or repeated themselves. Resuming is "Continue".
  static const PipePause PIPE_PAUSE_LOOP =
      PipePause._(3, _omitEnumNames ? '' : 'PIPE_PAUSE_LOOP');

  static const $core.List<PipePause> values = <PipePause>[
    PIPE_PAUSE_UNSPECIFIED,
    PIPE_PAUSE_BY_ME,
    PIPE_PAUSE_BY_PEER,
    PIPE_PAUSE_LOOP,
  ];

  static final $core.List<PipePause?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static PipePause? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipePause._(super.value, super.name);
}

/// How a pipe's other side changed from the identity pinned with the pipe.
/// Only a statement checked here raises one: while this Claktory can't
/// check (signed out, or another account service), nothing changes.
class PipeIdentityChange extends $pb.ProtobufEnum {
  static const PipeIdentityChange PIPE_IDENTITY_CHANGE_NONE =
      PipeIdentityChange._(
          0, _omitEnumNames ? '' : 'PIPE_IDENTITY_CHANGE_NONE');

  /// It stopped being vouched for: its statement ran out (a week) or it
  /// signed out. "No longer verified"; nothing is held.
  /// AcceptPipeIdentity dismisses it.
  static const PipeIdentityChange PIPE_IDENTITY_CHANGE_LAPSED =
      PipeIdentityChange._(
          1, _omitEnumNames ? '' : 'PIPE_IDENTITY_CHANGE_LAPSED');

  /// The same account, with another email (verified_email; previous_email
  /// the old one): a notice, nothing held. AcceptPipeIdentity dismisses it.
  static const PipeIdentityChange PIPE_IDENTITY_CHANGE_EMAIL_CHANGED =
      PipeIdentityChange._(
          2, _omitEnumNames ? '' : 'PIPE_IDENTITY_CHANGE_EMAIL_CHANGED');

  /// Another account (verified_email, if it's vouched for; previous_email
  /// the old one): a warning in red. Until AcceptPipeIdentity, what comes
  /// from the other side waits for the person (HELD_INCOMING) and agents
  /// can't send to it; the person still can.
  static const PipeIdentityChange PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED =
      PipeIdentityChange._(
          3, _omitEnumNames ? '' : 'PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED');

  static const $core.List<PipeIdentityChange> values = <PipeIdentityChange>[
    PIPE_IDENTITY_CHANGE_NONE,
    PIPE_IDENTITY_CHANGE_LAPSED,
    PIPE_IDENTITY_CHANGE_EMAIL_CHANGED,
    PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED,
  ];

  static final $core.List<PipeIdentityChange?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static PipeIdentityChange? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeIdentityChange._(super.value, super.name);
}

class PipeAuthorKind extends $pb.ProtobufEnum {
  static const PipeAuthorKind PIPE_AUTHOR_KIND_UNSPECIFIED =
      PipeAuthorKind._(0, _omitEnumNames ? '' : 'PIPE_AUTHOR_KIND_UNSPECIFIED');

  /// An agent on the other side, as it says.
  static const PipeAuthorKind PIPE_AUTHOR_KIND_THEIR_AGENT =
      PipeAuthorKind._(1, _omitEnumNames ? '' : 'PIPE_AUTHOR_KIND_THEIR_AGENT');

  /// The other side says its person wrote it: "says it's from <name>".
  static const PipeAuthorKind PIPE_AUTHOR_KIND_THEIR_PERSON = PipeAuthorKind._(
      2, _omitEnumNames ? '' : 'PIPE_AUTHOR_KIND_THEIR_PERSON');
  static const PipeAuthorKind PIPE_AUTHOR_KIND_MY_AGENT =
      PipeAuthorKind._(3, _omitEnumNames ? '' : 'PIPE_AUTHOR_KIND_MY_AGENT');

  /// This Claktory's person.
  static const PipeAuthorKind PIPE_AUTHOR_KIND_ME =
      PipeAuthorKind._(4, _omitEnumNames ? '' : 'PIPE_AUTHOR_KIND_ME');

  /// Claktory, about the pipe: shown as a line, not a bubble.
  static const PipeAuthorKind PIPE_AUTHOR_KIND_CLAKTORY =
      PipeAuthorKind._(5, _omitEnumNames ? '' : 'PIPE_AUTHOR_KIND_CLAKTORY');

  static const $core.List<PipeAuthorKind> values = <PipeAuthorKind>[
    PIPE_AUTHOR_KIND_UNSPECIFIED,
    PIPE_AUTHOR_KIND_THEIR_AGENT,
    PIPE_AUTHOR_KIND_THEIR_PERSON,
    PIPE_AUTHOR_KIND_MY_AGENT,
    PIPE_AUTHOR_KIND_ME,
    PIPE_AUTHOR_KIND_CLAKTORY,
  ];

  static final $core.List<PipeAuthorKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
  static PipeAuthorKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeAuthorKind._(super.value, super.name);
}

class PipeMessageState extends $pb.ProtobufEnum {
  static const PipeMessageState PIPE_MESSAGE_STATE_UNSPECIFIED =
      PipeMessageState._(
          0, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_UNSPECIFIED');

  /// Outgoing, waiting for the person's Send or Discard (DecidePipeMessage):
  /// Review outgoing is on, or it carries files from outside the outbox.
  static const PipeMessageState PIPE_MESSAGE_STATE_HELD =
      PipeMessageState._(1, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_HELD');

  /// Outgoing, passed: on its way (the other side may be offline).
  static const PipeMessageState PIPE_MESSAGE_STATE_QUEUED =
      PipeMessageState._(2, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_QUEUED');

  /// Outgoing, and the other side has it.
  static const PipeMessageState PIPE_MESSAGE_STATE_DELIVERED =
      PipeMessageState._(
          3, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_DELIVERED');

  /// Outgoing, and stopped: by Claktory (a secret in it, too big, over the
  /// limit) or by the other side. `note` says why. Never sent.
  static const PipeMessageState PIPE_MESSAGE_STATE_REFUSED =
      PipeMessageState._(4, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_REFUSED');

  /// Outgoing, and the person discarded it. Never sent.
  static const PipeMessageState PIPE_MESSAGE_STATE_DISCARDED =
      PipeMessageState._(
          5, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_DISCARDED');

  /// Incoming, and in: the lead was told.
  static const PipeMessageState PIPE_MESSAGE_STATE_RECEIVED =
      PipeMessageState._(
          6, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_RECEIVED');

  /// Incoming over the rate limit: no agent sees it until the person lets
  /// it in (DecidePipeMessage).
  static const PipeMessageState PIPE_MESSAGE_STATE_HELD_INCOMING =
      PipeMessageState._(
          7, _omitEnumNames ? '' : 'PIPE_MESSAGE_STATE_HELD_INCOMING');

  static const $core.List<PipeMessageState> values = <PipeMessageState>[
    PIPE_MESSAGE_STATE_UNSPECIFIED,
    PIPE_MESSAGE_STATE_HELD,
    PIPE_MESSAGE_STATE_QUEUED,
    PIPE_MESSAGE_STATE_DELIVERED,
    PIPE_MESSAGE_STATE_REFUSED,
    PIPE_MESSAGE_STATE_DISCARDED,
    PIPE_MESSAGE_STATE_RECEIVED,
    PIPE_MESSAGE_STATE_HELD_INCOMING,
  ];

  static final $core.List<PipeMessageState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static PipeMessageState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeMessageState._(super.value, super.name);
}

class PipeAction extends $pb.ProtobufEnum {
  static const PipeAction PIPE_ACTION_UNSPECIFIED =
      PipeAction._(0, _omitEnumNames ? '' : 'PIPE_ACTION_UNSPECIFIED');
  static const PipeAction PIPE_ACTION_PAUSE =
      PipeAction._(1, _omitEnumNames ? '' : 'PIPE_ACTION_PAUSE');
  static const PipeAction PIPE_ACTION_RESUME =
      PipeAction._(2, _omitEnumNames ? '' : 'PIPE_ACTION_RESUME');
  static const PipeAction PIPE_ACTION_CLOSE =
      PipeAction._(3, _omitEnumNames ? '' : 'PIPE_ACTION_CLOSE');
  static const PipeAction PIPE_ACTION_DELETE =
      PipeAction._(4, _omitEnumNames ? '' : 'PIPE_ACTION_DELETE');

  static const $core.List<PipeAction> values = <PipeAction>[
    PIPE_ACTION_UNSPECIFIED,
    PIPE_ACTION_PAUSE,
    PIPE_ACTION_RESUME,
    PIPE_ACTION_CLOSE,
    PIPE_ACTION_DELETE,
  ];

  static final $core.List<PipeAction?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static PipeAction? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeAction._(super.value, super.name);
}

class SecretApprovalState extends $pb.ProtobufEnum {
  static const SecretApprovalState SECRET_APPROVAL_STATE_UNSPECIFIED =
      SecretApprovalState._(
          0, _omitEnumNames ? '' : 'SECRET_APPROVAL_STATE_UNSPECIFIED');
  static const SecretApprovalState SECRET_APPROVAL_STATE_PENDING =
      SecretApprovalState._(
          1, _omitEnumNames ? '' : 'SECRET_APPROVAL_STATE_PENDING');
  static const SecretApprovalState SECRET_APPROVAL_STATE_ALLOWED =
      SecretApprovalState._(
          2, _omitEnumNames ? '' : 'SECRET_APPROVAL_STATE_ALLOWED');
  static const SecretApprovalState SECRET_APPROVAL_STATE_DENIED =
      SecretApprovalState._(
          3, _omitEnumNames ? '' : 'SECRET_APPROVAL_STATE_DENIED');

  /// Left unanswered 12 hours.
  static const SecretApprovalState SECRET_APPROVAL_STATE_EXPIRED =
      SecretApprovalState._(
          4, _omitEnumNames ? '' : 'SECRET_APPROVAL_STATE_EXPIRED');

  static const $core.List<SecretApprovalState> values = <SecretApprovalState>[
    SECRET_APPROVAL_STATE_UNSPECIFIED,
    SECRET_APPROVAL_STATE_PENDING,
    SECRET_APPROVAL_STATE_ALLOWED,
    SECRET_APPROVAL_STATE_DENIED,
    SECRET_APPROVAL_STATE_EXPIRED,
  ];

  static final $core.List<SecretApprovalState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static SecretApprovalState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const SecretApprovalState._(super.value, super.name);
}

class AskStepState extends $pb.ProtobufEnum {
  /// Read as PENDING.
  static const AskStepState ASK_STEP_STATE_UNSPECIFIED =
      AskStepState._(0, _omitEnumNames ? '' : 'ASK_STEP_STATE_UNSPECIFIED');
  static const AskStepState ASK_STEP_STATE_PENDING =
      AskStepState._(1, _omitEnumNames ? '' : 'ASK_STEP_STATE_PENDING');
  static const AskStepState ASK_STEP_STATE_DONE =
      AskStepState._(2, _omitEnumNames ? '' : 'ASK_STEP_STATE_DONE');

  /// The person couldn't do it: the procedure stops here.
  static const AskStepState ASK_STEP_STATE_CANT =
      AskStepState._(3, _omitEnumNames ? '' : 'ASK_STEP_STATE_CANT');

  static const $core.List<AskStepState> values = <AskStepState>[
    ASK_STEP_STATE_UNSPECIFIED,
    ASK_STEP_STATE_PENDING,
    ASK_STEP_STATE_DONE,
    ASK_STEP_STATE_CANT,
  ];

  static final $core.List<AskStepState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static AskStepState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AskStepState._(super.value, super.name);
}

class AskPersonState extends $pb.ProtobufEnum {
  static const AskPersonState ASK_PERSON_STATE_UNSPECIFIED =
      AskPersonState._(0, _omitEnumNames ? '' : 'ASK_PERSON_STATE_UNSPECIFIED');
  static const AskPersonState ASK_PERSON_STATE_PENDING =
      AskPersonState._(1, _omitEnumNames ? '' : 'ASK_PERSON_STATE_PENDING');
  static const AskPersonState ASK_PERSON_STATE_ANSWERED =
      AskPersonState._(2, _omitEnumNames ? '' : 'ASK_PERSON_STATE_ANSWERED');
  static const AskPersonState ASK_PERSON_STATE_DISMISSED =
      AskPersonState._(3, _omitEnumNames ? '' : 'ASK_PERSON_STATE_DISMISSED');

  static const $core.List<AskPersonState> values = <AskPersonState>[
    ASK_PERSON_STATE_UNSPECIFIED,
    ASK_PERSON_STATE_PENDING,
    ASK_PERSON_STATE_ANSWERED,
    ASK_PERSON_STATE_DISMISSED,
  ];

  static final $core.List<AskPersonState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static AskPersonState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const AskPersonState._(super.value, super.name);
}

class MovePhase extends $pb.ProtobufEnum {
  static const MovePhase MOVE_PHASE_UNSPECIFIED =
      MovePhase._(0, _omitEnumNames ? '' : 'MOVE_PHASE_UNSPECIFIED');

  /// Dialing the other computer.
  static const MovePhase MOVE_PHASE_CONNECTING =
      MovePhase._(1, _omitEnumNames ? '' : 'MOVE_PHASE_CONNECTING');

  /// Both show `digits`: waiting for ConfirmMove here and the person there.
  static const MovePhase MOVE_PHASE_CONFIRM =
      MovePhase._(2, _omitEnumNames ? '' : 'MOVE_PHASE_CONFIRM');

  /// The other computer checks the plan.
  static const MovePhase MOVE_PHASE_CHECKING =
      MovePhase._(3, _omitEnumNames ? '' : 'MOVE_PHASE_CHECKING');

  /// Copying while agents keep working; FreezeMove ends it now.
  static const MovePhase MOVE_PHASE_COPYING =
      MovePhase._(4, _omitEnumNames ? '' : 'MOVE_PHASE_COPYING');

  /// Agents are stopped and the last round goes; only `move` is served.
  static const MovePhase MOVE_PHASE_FROZEN =
      MovePhase._(5, _omitEnumNames ? '' : 'MOVE_PHASE_FROZEN');

  /// The other computer sets it up and checks it (`step_text` says what).
  static const MovePhase MOVE_PHASE_IMPORTING =
      MovePhase._(6, _omitEnumNames ? '' : 'MOVE_PHASE_IMPORTING');

  /// This copy is put away; telling the other computer to start. No way back.
  static const MovePhase MOVE_PHASE_COMMITTING =
      MovePhase._(7, _omitEnumNames ? '' : 'MOVE_PHASE_COMMITTING');

  /// Moved: the other computer runs this Claktory now.
  static const MovePhase MOVE_PHASE_DONE =
      MovePhase._(8, _omitEnumNames ? '' : 'MOVE_PHASE_DONE');

  /// It didn't go: `error` says why. Claktory runs here as before.
  /// CancelMove dismisses it.
  static const MovePhase MOVE_PHASE_FAILED =
      MovePhase._(9, _omitEnumNames ? '' : 'MOVE_PHASE_FAILED');

  static const $core.List<MovePhase> values = <MovePhase>[
    MOVE_PHASE_UNSPECIFIED,
    MOVE_PHASE_CONNECTING,
    MOVE_PHASE_CONFIRM,
    MOVE_PHASE_CHECKING,
    MOVE_PHASE_COPYING,
    MOVE_PHASE_FROZEN,
    MOVE_PHASE_IMPORTING,
    MOVE_PHASE_COMMITTING,
    MOVE_PHASE_DONE,
    MOVE_PHASE_FAILED,
  ];

  static final $core.List<MovePhase?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 9);
  static MovePhase? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const MovePhase._(super.value, super.name);
}

class NotifyKind extends $pb.ProtobufEnum {
  static const NotifyKind NOTIFY_KIND_UNSPECIFIED =
      NotifyKind._(0, _omitEnumNames ? '' : 'NOTIFY_KIND_UNSPECIFIED');

  /// Something waits for the person: a question, a request for a secret, a
  /// proposal, a held pipe message.
  static const NotifyKind NOTIFY_KIND_NEEDS_YOU =
      NotifyKind._(1, _omitEnumNames ? '' : 'NOTIFY_KIND_NEEDS_YOU');

  /// An agent's notify_person.
  static const NotifyKind NOTIFY_KIND_ALERT =
      NotifyKind._(2, _omitEnumNames ? '' : 'NOTIFY_KIND_ALERT');

  /// An agent answered the person's message.
  static const NotifyKind NOTIFY_KIND_REPLY =
      NotifyKind._(3, _omitEnumNames ? '' : 'NOTIFY_KIND_REPLY');

  /// SendTestNotification's.
  static const NotifyKind NOTIFY_KIND_TEST =
      NotifyKind._(4, _omitEnumNames ? '' : 'NOTIFY_KIND_TEST');

  static const $core.List<NotifyKind> values = <NotifyKind>[
    NOTIFY_KIND_UNSPECIFIED,
    NOTIFY_KIND_NEEDS_YOU,
    NOTIFY_KIND_ALERT,
    NOTIFY_KIND_REPLY,
    NOTIFY_KIND_TEST,
  ];

  static final $core.List<NotifyKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static NotifyKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const NotifyKind._(super.value, super.name);
}

/// What notifications say.
class NotifyPreview extends $pb.ProtobufEnum {
  /// As FULL.
  static const NotifyPreview NOTIFY_PREVIEW_UNSPECIFIED =
      NotifyPreview._(0, _omitEnumNames ? '' : 'NOTIFY_PREVIEW_UNSPECIFIED');

  /// Who, and what it says.
  static const NotifyPreview NOTIFY_PREVIEW_FULL =
      NotifyPreview._(1, _omitEnumNames ? '' : 'NOTIFY_PREVIEW_FULL');

  /// Only who: "Pericolo needs you".
  static const NotifyPreview NOTIFY_PREVIEW_WHO =
      NotifyPreview._(2, _omitEnumNames ? '' : 'NOTIFY_PREVIEW_WHO');

  /// Just "Claktory has something for you".
  static const NotifyPreview NOTIFY_PREVIEW_NONE =
      NotifyPreview._(3, _omitEnumNames ? '' : 'NOTIFY_PREVIEW_NONE');

  static const $core.List<NotifyPreview> values = <NotifyPreview>[
    NOTIFY_PREVIEW_UNSPECIFIED,
    NOTIFY_PREVIEW_FULL,
    NOTIFY_PREVIEW_WHO,
    NOTIFY_PREVIEW_NONE,
  ];

  static final $core.List<NotifyPreview?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static NotifyPreview? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const NotifyPreview._(super.value, super.name);
}

/// Whether an agent notifies the person.
class NotifyMode extends $pb.ProtobufEnum {
  /// As NotificationSettings says for each kind.
  static const NotifyMode NOTIFY_MODE_DEFAULT =
      NotifyMode._(0, _omitEnumNames ? '' : 'NOTIFY_MODE_DEFAULT');

  /// Every kind, even those turned off (still within quiet hours and the
  /// hourly cap).
  static const NotifyMode NOTIFY_MODE_ALWAYS =
      NotifyMode._(1, _omitEnumNames ? '' : 'NOTIFY_MODE_ALWAYS');

  /// Never.
  static const NotifyMode NOTIFY_MODE_NEVER =
      NotifyMode._(2, _omitEnumNames ? '' : 'NOTIFY_MODE_NEVER');

  static const $core.List<NotifyMode> values = <NotifyMode>[
    NOTIFY_MODE_DEFAULT,
    NOTIFY_MODE_ALWAYS,
    NOTIFY_MODE_NEVER,
  ];

  static final $core.List<NotifyMode?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static NotifyMode? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const NotifyMode._(super.value, super.name);
}

class PushPlatform extends $pb.ProtobufEnum {
  static const PushPlatform PUSH_PLATFORM_UNSPECIFIED =
      PushPlatform._(0, _omitEnumNames ? '' : 'PUSH_PLATFORM_UNSPECIFIED');
  static const PushPlatform PUSH_PLATFORM_APNS =
      PushPlatform._(1, _omitEnumNames ? '' : 'PUSH_PLATFORM_APNS');

  /// An app built for development, on APNs's sandbox.
  static const PushPlatform PUSH_PLATFORM_APNS_SANDBOX =
      PushPlatform._(2, _omitEnumNames ? '' : 'PUSH_PLATFORM_APNS_SANDBOX');
  static const PushPlatform PUSH_PLATFORM_FCM =
      PushPlatform._(3, _omitEnumNames ? '' : 'PUSH_PLATFORM_FCM');

  static const $core.List<PushPlatform> values = <PushPlatform>[
    PUSH_PLATFORM_UNSPECIFIED,
    PUSH_PLATFORM_APNS,
    PUSH_PLATFORM_APNS_SANDBOX,
    PUSH_PLATFORM_FCM,
  ];

  static final $core.List<PushPlatform?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 3);
  static PushPlatform? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PushPlatform._(super.value, super.name);
}

class NeedsYouRecordKind extends $pb.ProtobufEnum {
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_UNSPECIFIED =
      NeedsYouRecordKind._(
          0, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_UNSPECIFIED');

  /// An agent's question (ask_person).
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_QUESTION =
      NeedsYouRecordKind._(
          1, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_QUESTION');

  /// A review ask: a file to review.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_REVIEW =
      NeedsYouRecordKind._(
          2, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_REVIEW');

  /// A request for a secret.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_SECRET_REQUEST =
      NeedsYouRecordKind._(
          3, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_SECRET_REQUEST');

  /// A command using secrets, asked for once (SecretApproval).
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_SECRET_USE =
      NeedsYouRecordKind._(
          4, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_SECRET_USE');

  /// A proposal to Rivet.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_PROPOSAL =
      NeedsYouRecordKind._(
          5, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_PROPOSAL');

  /// A proposal to connect a service.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_CONNECT =
      NeedsYouRecordKind._(
          6, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_CONNECT');

  /// An agent's card for starting or accepting a pipe.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_PIPE_START =
      NeedsYouRecordKind._(
          7, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_PIPE_START');

  /// A pipe's held message, let through (APPROVED) or discarded (DENIED):
  /// an agent's to the other side (agent_id set), or the other side's.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_PIPE_HELD =
      NeedsYouRecordKind._(
          8, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_PIPE_HELD');

  /// A pipe's loop pause the person let carry on (APPROVED).
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_PIPE_LOOP =
      NeedsYouRecordKind._(
          9, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_PIPE_LOOP');

  /// Rivet's proposal to give an agent access to an app at the person's
  /// account (AppGrantProposal): APPROVED when the grant was made, DENIED
  /// when the person declined it, FAILED when they approved it but it
  /// couldn't be made. agent_id is Rivet, whose conversation shows the card;
  /// the title names the agent it was for and the app, as it says now.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_APP_GRANT =
      NeedsYouRecordKind._(
          10, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_APP_GRANT');

  /// Where reports go changed: a pipe's forward or a project's default was
  /// set, or an agent was made a report reader or let go (outcome CHANGED).
  /// agent_id is the admin agent that changed it (Rivet), empty when the
  /// person did; pipe_id is set for a pipe's forward; title is Claktory's
  /// sentence ("Rivet set this pipe's reports to go to Ingrid"). A change
  /// of the types a pipe (or a project's new pipes) can forward is one too:
  /// report_types holds those it turned on.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_PIPE_FORWARD =
      NeedsYouRecordKind._(
          11, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_PIPE_FORWARD');

  /// A report type was defined, edited or deleted (outcome CHANGED):
  /// agent_id is the admin agent (Rivet), empty for the person; title is
  /// Claktory's sentence; report_type and report_type_before are the
  /// versions.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_REPORT_TYPE =
      NeedsYouRecordKind._(
          12, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_REPORT_TYPE');

  /// A change to what an app reads, which agents use it and what they hear
  /// of it: CHANGED when Rivet applied it (agent_id Rivet) or the person
  /// did (agent_id empty), from their apps or by undoing another;
  /// APPROVED, DENIED or FAILED for Rivet's card (AppAccessProposal).
  /// title is Claktory's sentence ("Rivet gave Ingrid Change dashboards on
  /// Charts"); app_change_id undoes it; agent_reason is Rivet's why. seq is
  /// the apps_set_access call's entry in Rivet's conversation (or its
  /// card's); 0 for the person's own change or undo: open Marketplace →
  /// Apps at app_id then. One that changed what agents hear counts as a
  /// change of their grant (AppGrant.given).
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_APP_ACCESS =
      NeedsYouRecordKind._(
          13, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_APP_ACCESS');

  /// An agent (Rivet) put an agent on another model or effort with
  /// models_set, which changes what the person spends: outcome CHANGED,
  /// agent_id who did it, subject_agent_id whose model it was, title
  /// Claktory's sentence ("Rivet put Ingrid on Claude Opus 5 at high effort", "Rivet
  /// changed Ingrid's effort to high"), agent_reason its why (models_set's
  /// reason; may be empty), model_change_id to undo it (UndoModelChange),
  /// undone once undone. seq is the models_set call's entry in its
  /// conversation.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_MODEL =
      NeedsYouRecordKind._(
          14, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_MODEL');

  /// An agent (Rivet) took another agent's conversation back to an earlier
  /// message, or switched it to another version, with the Conversations
  /// extension (friend bug #9): outcome CHANGED, agent_id who did it,
  /// subject_agent_id whose conversation it was, title Claktory's sentence
  /// ("Rivet took Mike's conversation back to 14:02", "Rivet switched Mike
  /// to another version"), agent_reason its why (may be empty). No Undo:
  /// what was set aside is in the subject's other versions. seq is the tool
  /// call's entry in the acting agent's conversation. The person's own
  /// changes aren't listed.
  static const NeedsYouRecordKind NEEDS_YOU_RECORD_KIND_CONVERSATION =
      NeedsYouRecordKind._(
          15, _omitEnumNames ? '' : 'NEEDS_YOU_RECORD_KIND_CONVERSATION');

  static const $core.List<NeedsYouRecordKind> values = <NeedsYouRecordKind>[
    NEEDS_YOU_RECORD_KIND_UNSPECIFIED,
    NEEDS_YOU_RECORD_KIND_QUESTION,
    NEEDS_YOU_RECORD_KIND_REVIEW,
    NEEDS_YOU_RECORD_KIND_SECRET_REQUEST,
    NEEDS_YOU_RECORD_KIND_SECRET_USE,
    NEEDS_YOU_RECORD_KIND_PROPOSAL,
    NEEDS_YOU_RECORD_KIND_CONNECT,
    NEEDS_YOU_RECORD_KIND_PIPE_START,
    NEEDS_YOU_RECORD_KIND_PIPE_HELD,
    NEEDS_YOU_RECORD_KIND_PIPE_LOOP,
    NEEDS_YOU_RECORD_KIND_APP_GRANT,
    NEEDS_YOU_RECORD_KIND_PIPE_FORWARD,
    NEEDS_YOU_RECORD_KIND_REPORT_TYPE,
    NEEDS_YOU_RECORD_KIND_APP_ACCESS,
    NEEDS_YOU_RECORD_KIND_MODEL,
    NEEDS_YOU_RECORD_KIND_CONVERSATION,
  ];

  static final $core.List<NeedsYouRecordKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 15);
  static NeedsYouRecordKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const NeedsYouRecordKind._(super.value, super.name);
}

class NeedsYouOutcome extends $pb.ProtobufEnum {
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_UNSPECIFIED =
      NeedsYouOutcome._(
          0, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_UNSPECIFIED');
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_ANSWERED =
      NeedsYouOutcome._(1, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_ANSWERED');
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_DISMISSED =
      NeedsYouOutcome._(2, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_DISMISSED');
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_APPROVED =
      NeedsYouOutcome._(3, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_APPROVED');
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_DENIED =
      NeedsYouOutcome._(4, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_DENIED');

  /// Approved, but what it asked couldn't be done (APP_GRANT: the account
  /// service refused the grant, or the agent was deleted since).
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_FAILED =
      NeedsYouOutcome._(5, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_FAILED');

  /// A setting changed, rather than a card answered (PIPE_FORWARD).
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_CHANGED =
      NeedsYouOutcome._(6, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_CHANGED');

  /// The person replied with a comment rather than approving or dismissing
  /// (#370 Submit): `reply` holds it.
  static const NeedsYouOutcome NEEDS_YOU_OUTCOME_REPLIED =
      NeedsYouOutcome._(7, _omitEnumNames ? '' : 'NEEDS_YOU_OUTCOME_REPLIED');

  static const $core.List<NeedsYouOutcome> values = <NeedsYouOutcome>[
    NEEDS_YOU_OUTCOME_UNSPECIFIED,
    NEEDS_YOU_OUTCOME_ANSWERED,
    NEEDS_YOU_OUTCOME_DISMISSED,
    NEEDS_YOU_OUTCOME_APPROVED,
    NEEDS_YOU_OUTCOME_DENIED,
    NEEDS_YOU_OUTCOME_FAILED,
    NEEDS_YOU_OUTCOME_CHANGED,
    NEEDS_YOU_OUTCOME_REPLIED,
  ];

  static final $core.List<NeedsYouOutcome?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 7);
  static NeedsYouOutcome? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const NeedsYouOutcome._(super.value, super.name);
}

class ReportFieldType extends $pb.ProtobufEnum {
  static const ReportFieldType REPORT_FIELD_TYPE_UNSPECIFIED =
      ReportFieldType._(
          0, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_UNSPECIFIED');
  static const ReportFieldType REPORT_FIELD_TYPE_TEXT =
      ReportFieldType._(1, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_TEXT');
  static const ReportFieldType REPORT_FIELD_TYPE_TEXT_LIST =
      ReportFieldType._(2, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_TEXT_LIST');
  static const ReportFieldType REPORT_FIELD_TYPE_ENUM =
      ReportFieldType._(3, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_ENUM');
  static const ReportFieldType REPORT_FIELD_TYPE_BOOL =
      ReportFieldType._(4, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_BOOL');

  /// A known release ("v0.1.16"), or "unknown".
  static const ReportFieldType REPORT_FIELD_TYPE_VERSION =
      ReportFieldType._(5, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_VERSION');

  /// Files from the pipe's inbox, copied into the report.
  static const ReportFieldType REPORT_FIELD_TYPE_ATTACHMENTS =
      ReportFieldType._(
          6, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_ATTACHMENTS');

  /// A whole number within the field's min and max.
  static const ReportFieldType REPORT_FIELD_TYPE_NUMBER =
      ReportFieldType._(7, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_NUMBER');

  /// A day, "2026-11-22", from 1900-01-01 to 2999-12-31.
  static const ReportFieldType REPORT_FIELD_TYPE_DATE =
      ReportFieldType._(8, _omitEnumNames ? '' : 'REPORT_FIELD_TYPE_DATE');

  static const $core.List<ReportFieldType> values = <ReportFieldType>[
    REPORT_FIELD_TYPE_UNSPECIFIED,
    REPORT_FIELD_TYPE_TEXT,
    REPORT_FIELD_TYPE_TEXT_LIST,
    REPORT_FIELD_TYPE_ENUM,
    REPORT_FIELD_TYPE_BOOL,
    REPORT_FIELD_TYPE_VERSION,
    REPORT_FIELD_TYPE_ATTACHMENTS,
    REPORT_FIELD_TYPE_NUMBER,
    REPORT_FIELD_TYPE_DATE,
  ];

  static final $core.List<ReportFieldType?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 8);
  static ReportFieldType? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ReportFieldType._(super.value, super.name);
}

class PipeReportState extends $pb.ProtobufEnum {
  static const PipeReportState PIPE_REPORT_STATE_UNSPECIFIED =
      PipeReportState._(
          0, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_UNSPECIFIED');

  /// Forwarded; its reader hasn't marked it.
  static const PipeReportState PIPE_REPORT_STATE_NEW =
      PipeReportState._(1, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_NEW');
  static const PipeReportState PIPE_REPORT_STATE_ACCEPTED =
      PipeReportState._(2, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_ACCEPTED');

  /// "Already known as A3".
  static const PipeReportState PIPE_REPORT_STATE_KNOWN =
      PipeReportState._(3, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_KNOWN');

  /// "Fixed in v0.1.16".
  static const PipeReportState PIPE_REPORT_STATE_FIXED =
      PipeReportState._(4, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_FIXED');
  static const PipeReportState PIPE_REPORT_STATE_WONT_FIX =
      PipeReportState._(5, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_WONT_FIX');

  /// "Needs more: steps".
  static const PipeReportState PIPE_REPORT_STATE_NEEDS_INFO = PipeReportState._(
      6, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_NEEDS_INFO');

  /// Not forwarded.
  static const PipeReportState PIPE_REPORT_STATE_REFUSED =
      PipeReportState._(7, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_REFUSED');

  /// Where it goes changed before it was read: the person sends or discards
  /// it (DecidePipeReports).
  static const PipeReportState PIPE_REPORT_STATE_WAITING =
      PipeReportState._(8, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_WAITING');

  /// The person discarded it while it waited.
  static const PipeReportState PIPE_REPORT_STATE_DISCARDED =
      PipeReportState._(9, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_DISCARDED');

  /// Its reader did what it asked.
  static const PipeReportState PIPE_REPORT_STATE_DONE =
      PipeReportState._(10, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_DONE');

  /// Its reader won't do what it asked.
  static const PipeReportState PIPE_REPORT_STATE_DECLINED =
      PipeReportState._(11, _omitEnumNames ? '' : 'PIPE_REPORT_STATE_DECLINED');

  static const $core.List<PipeReportState> values = <PipeReportState>[
    PIPE_REPORT_STATE_UNSPECIFIED,
    PIPE_REPORT_STATE_NEW,
    PIPE_REPORT_STATE_ACCEPTED,
    PIPE_REPORT_STATE_KNOWN,
    PIPE_REPORT_STATE_FIXED,
    PIPE_REPORT_STATE_WONT_FIX,
    PIPE_REPORT_STATE_NEEDS_INFO,
    PIPE_REPORT_STATE_REFUSED,
    PIPE_REPORT_STATE_WAITING,
    PIPE_REPORT_STATE_DISCARDED,
    PIPE_REPORT_STATE_DONE,
    PIPE_REPORT_STATE_DECLINED,
  ];

  static final $core.List<PipeReportState?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 11);
  static PipeReportState? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const PipeReportState._(super.value, super.name);
}

const $core.bool _omitEnumNames =
    $core.bool.fromEnvironment('protobuf.omit_enum_names');
