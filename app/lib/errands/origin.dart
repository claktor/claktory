/// The host of [origin] ("https://play.google.com" → "play.google.com"),
/// or [origin] itself when it isn't a web address.
String originHost(String origin) {
  final uri = Uri.tryParse(origin);
  return uri == null || uri.host.isEmpty ? origin : uri.host;
}
