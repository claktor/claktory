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
  ];

  static final $core.List<ErrorCode?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 8);
  static ErrorCode? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ErrorCode._(super.value, super.name);
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

  static const $core.List<ExtensionOptionKind> values = <ExtensionOptionKind>[
    EXTENSION_OPTION_KIND_UNSPECIFIED,
    EXTENSION_OPTION_KIND_SWITCH,
    EXTENSION_OPTION_KIND_AGENTS,
  ];

  static final $core.List<ExtensionOptionKind?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 2);
  static ExtensionOptionKind? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const ExtensionOptionKind._(super.value, super.name);
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

  static const $core.List<ChecklistStatus> values = <ChecklistStatus>[
    CHECKLIST_STATUS_UNSPECIFIED,
    CHECKLIST_STATUS_OPEN,
    CHECKLIST_STATUS_IN_PROGRESS,
    CHECKLIST_STATUS_DONE,
    CHECKLIST_STATUS_DROPPED,
    CHECKLIST_STATUS_BLOCKED,
  ];

  static final $core.List<ChecklistStatus?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 5);
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

  /// Idle for 15 minutes or more. Only a look: it wakes at once for anything.
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

  static const $core.List<EventSource> values = <EventSource>[
    EVENT_SOURCE_UNSPECIFIED,
    EVENT_SOURCE_FACTORY,
    EVENT_SOURCE_AGENT,
    EVENT_SOURCE_TASK,
    EVENT_SOURCE_MONITOR,
  ];

  static final $core.List<EventSource?> _byValue =
      $pb.ProtobufEnum.$_initByValueList(values, 4);
  static EventSource? valueOf($core.int value) =>
      value < 0 || value >= _byValue.length ? null : _byValue[value];

  const EventSource._(super.value, super.name);
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

const $core.bool _omitEnumNames =
    $core.bool.fromEnvironment('protobuf.omit_enum_names');
