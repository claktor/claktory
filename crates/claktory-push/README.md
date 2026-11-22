# claktory-push

Claktory's push relay. Claktories send notifications to their people's phones through it, because only the holder of Claktory's Apple key can push to `com.claktory.app` (and of the Firebase service account, for Android). The plan is notifications.md §3 in Research.

It learns as little as it can:

- A phone registers its push token and gets back a random handle. Its Claktory pushes with that handle. The relay never learns which person, account or Claktory a handle belongs to.
- It keeps each handle's SHA-256, never the handle itself, so a copy of its database lets nobody push.
- Every push is ciphertext sealed by the Claktory with a key only the phone has. The relay sends it under a fixed alert, "Claktory has something for you". The phone's notification extension replaces that text with what it decrypts.
- Callers can't set any text: the API refuses fields it doesn't take.
- What it does see: each phone's token, the address a phone registers from and the address each Claktory pushes from, timing, urgency and the size bucket.

## API

JSON over HTTPS. TLS is handled by a proxy in front (Caddy, `--trust-proxy`).

| | |
|---|---|
| `POST /v1/register {platform, token}` → `{handle}` | platform: `apns`, `apns_sandbox` (development builds) or `fcm`. At most 20 an hour per IP, 10 an hour per token and 2,000 an hour in all. A token keeps its 20 newest handles. |
| `POST /v1/push {handle, ciphertext, urgency?, collapse?, thread?, ttl?}` → `{}` | ciphertext: base64, at most 2,200 bytes decoded. urgency: `normal`, `urgent` (time-sensitive) or `low`. collapse and thread: opaque base64url, at most 64 characters. ttl: seconds, at most a week (one day if left out). Limits: 10 a minute and 60 an hour per handle, 20 a minute and 120 an hour per device (its token, through all its handles), 1,000 an hour per IP, 50,000 an hour in all. |
| `POST /v1/unregister {handle}` → `{}` | |
| `GET /v1/health` → `{ok, apns, fcm}` | for watchdogs |

Errors are `{error, message, retry_after?}`:

- 400 `invalid_request`.
- 404 `unknown_handle`: forget the handle.
- 410 `gone`: Apple or Google retired the token. Every handle of that token is deleted.
- 429 `rate_limited`.
- 502 `unavailable` (try again later) or `refused`.

Handles that no push has used for 180 days are forgotten.

## Running it

```sh
claktory-push --db /var/lib/claktory-push/push.db --listen 127.0.0.1:8090 --trust-proxy \
  --apns-key-id KEYID --apns-team TEAMID [--apns-topic com.claktory.app]
```

The keys are read once, at start. Under systemd, give them as encrypted credentials (`LoadCredentialEncrypted=apns.p8:…`, `fcm.json:…`). With no `--apns-key` or `--fcm-service-account`, the relay reads `apns.p8` and `fcm.json` from `$CREDENTIALS_DIRECTORY`, so they never sit on disk in the clear.

## Running it on this computer, for tests

`--local FILE` runs a relay that needs no keys and never calls Apple or Google. It takes registrations and pushes as the real one does, for every platform, with the same checks and limits. Each push it would have sent is logged and appended to FILE as one JSON line: `at`, `platform`, `token`, `urgency`, `collapse`, `thread`, `ttl`, and `payload`, which is what Apple (or Google) would have been sent.

```sh
cargo run -p claktory-push -- --db /tmp/push.db --listen 127.0.0.1:8090 --local /tmp/pushes.jsonl
```

Point a Claktory at it with its notifications setting `relay` (http is allowed only on this computer), for example in a `scripts/qa.sh` sandbox:

```sh
scripts/qa.sh send qa1 UpdateSettings '{"notification_change":{"relay":"http://127.0.0.1:8090"}}'
scripts/qa.sh send qa1 SendTestNotification '{}'   # one line per registered device in /tmp/pushes.jsonl
```

A phone registers with the relay its Claktory names. The iPhone app takes only an https relay today (`HttpPushRelay` in `app/lib/mobile/push.dart`), so a dev build on a Simulator needs `http://127.0.0.1` allowed there first. A held push reaches a Simulator as Apple would deliver it: `tail -1 /tmp/pushes.jsonl | jq .payload | xcrun simctl push booted com.claktory.app.dev -`.

Its handling is the crown jewel's (Fugo's required change A):

- It runs as a Unix user of its own, apart from `iroh-relay`, or on a box of its own.
- It doesn't depend on the account service. An alert that the account service is down must still go out.
- Deploys use forced commands, and Matt is sent a notice.
- If Apple's developer portal offers it, restrict the key to `com.claktory.app`.
