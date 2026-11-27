import '../user_apps/session.dart';

/// What a device another of the person's devices removed says, in People
/// and its chats: the service's own words when it refuses it.
const deviceRemovedText =
    'This device was removed from your account. Sign in again to start '
    'over as a new device.';

/// What the person reads when a request to People didn't work.
String problemText(UserAppError error) => switch (error.problem) {
  UserAppProblem.notSignedIn =>
    'Sign in to your Claktory account on this device to use People.',
  UserAppProblem.deviceOnly => 'Only your own devices can do this.',
  UserAppProblem.stale =>
    'This changed on another device. What you see is up to date now.',
  UserAppProblem.rateLimited =>
    error.message.isNotEmpty
        ? error.message
        : 'Too many tries for now. Try again later.',
  UserAppProblem.unavailable =>
    "People isn't available right now. Try again in a few minutes.",
  UserAppProblem.unreachable =>
    "Can't reach claktory.com right now. Check your connection.",
  UserAppProblem.notFound => "It isn't there anymore.",
  UserAppProblem.gone =>
    error.message.isNotEmpty
        ? error.message
        : "This request isn't open anymore.",
  UserAppProblem.accepted =>
    "They accepted before you canceled. You're friends now.",
  UserAppProblem.storageFull =>
    "You're out of space for messages. Delete some conversations to make "
        'room.',
  UserAppProblem.invalid =>
    error.message.isNotEmpty ? error.message : "That didn't work. Try again.",
  UserAppProblem.epoch ||
  UserAppProblem.exists ||
  UserAppProblem.tooFarBehind =>
    'This changed on another device. What you see is up to date now.',
  UserAppProblem.notSetUp => "Messages aren't set up on this account yet.",
  UserAppProblem.removedDevice =>
    'This code was already used, or is for a device you removed.',
  UserAppProblem.signInAgain =>
    'For your security, sign in again to start over.',
  UserAppProblem.identityChanged =>
    'Your security key was changed on another of your devices.',
  UserAppProblem.waitingForFriend =>
    "Your friend can't get messages in this chat yet. Try again once they "
        'can.',
};
