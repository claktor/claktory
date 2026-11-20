import 'protocol/factory/v1/factory.pb.dart';
import 'usage.dart' show formatUntil;

/// Whether [agent] is at work now: its run is open and not held up by its
/// accounts' usage limits (then it sleeps, and says what it waits for).
bool atWork(Agent agent) =>
    agent.status == AgentStatus.AGENT_STATUS_WORKING && agent.usageWait.isEmpty;

/// What [agent] waits for while usage limits hold it up (Agent.usage_wait),
/// with the time it asks again in this device's own time zone: "Waiting for
/// your Claude usage to reset at 18:57" today, "… at Fri 18:57" later in the
/// week. The backend writes its own local time, which may be another zone's.
/// As the backend says it when there's no time (it waits for the person);
/// empty when it isn't held up.
String usageWaitLine(Agent agent, {DateTime? now}) {
  final text = agent.usageWait;
  final ms = agent.usageWaitUntilMs.toInt();
  final at = text.lastIndexOf(' at ');
  if (text.isEmpty || ms <= 0 || at < 0) return text;
  return '${text.substring(0, at)} at '
      '${formatUntil(ms, now ?? DateTime.now())}';
}
