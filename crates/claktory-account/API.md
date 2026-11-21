# claktory-account HTTP API, v1

The contract between a Claktory's backend (factoryd), the app on a person's
devices (Devices, below) and the account service (`claktory-account`). The design is in Opus Researcher's
`claktory-account.md`; this is what the service implements. Only add to it:
never change what a field means.

## Conventions

- JSON in and out, UTF-8. Bodies up to 1 MiB.
- Binary values are **base64url without padding** ("b64"). Times are unix
  seconds (integers).
- Errors: an HTTP status and `{"error": "<code>", "message": "<for people>", …}`.
  Codes below; a client acts on `error`, never on `message`.
- `429 rate_limited` carries `retry_after` (seconds) and a `Retry-After` header.
- The service's address is configurable (`account.service` in a Claktory's
  settings); `https://account.claktory.com` is Claktory's own.

## Keys and signatures

- **The service key**: Ed25519. `GET /.well-known/claktory-account` gives it;
  a Claktory pins it at its first sign-in and checks every answer with it.
- **The account root's statement** for the service key, beside it at
  well-known: an offline key of `ACCOUNT_ROOTS` (`protocol.rs`) signs exactly
  ```
  "claktory-account-service-key/1\n" "service: " URL "\n" "key: " b64(service key) "\n" "from: " UNIX_SECONDS "\n"
  ```
  (no other bytes: no CR, no spaces but those, `from` digits with no sign or
  leading zero), and the signature is `ROOT_ID ":" base64(ed25519 sig)`
  (standard base64, as releases' `latest.json.sig`). For claktory.com, once a
  Claktory release anchors it, a key comes only with one: at sign-in, at or
  after that release's floor; a key the service changes to, with a `from`
  later than the pinned key's, which a Claktory then takes by itself; and a
  pinned key from before a release's floor no longer checks device
  assertions. Services run by others aren't anchored.
- **Every answer** (errors too) carries `Claktory-Signature: b64(sig)`, the
  service key's signature over
  ```
  "claktory-account response v1\n" STATUS "\n" METHOD "\n" PATH_AND_QUERY "\n" hex(sha256(request body)) "\n" hex(sha256(response body))
  ```
  with STATUS the HTTP status in decimal (`200`), so an answer can't pass
  for another status's.
- **The backend key**: Ed25519, made by the Claktory, given at sign-in
  (`backend_key`). Requests marked *signed* carry
  - `Authorization: Bearer <machine_token>`
  - `Claktory-Time: <unix seconds>`
  - `Claktory-Signature: b64(sig)`, the backend key's signature over
    ```
    "claktory-account request v1\n" METHOD "\n" PATH_AND_QUERY "\n" TIME "\n" hex(sha256(request body))
    ```
  A time more than 300 s from the service's is refused (`401 clock_skew`, with
  `server_time`), and so is a signature already seen (`401 replayed`).
  A token without its key is useless. Signatures are deterministic, so the
  same request twice in one second is refused as `replayed`: a retry signs
  again with the time then.
- **A device's account key**: Ed25519, made by the app on a device (a phone,
  a laptop), given at its sign-in (`account_key`). Requests marked *device
  signed* carry `Authorization: Device <b64(account key)>` with the same
  `Claktory-Time` and `Claktory-Signature` as signed requests, by the
  account key, over the same bytes; the same `clock_skew` and `replayed`.
- **A device assertion**: `b64(payload) "." b64(sig)`, where payload is the
  JSON `{"v": 1, "account_id", "account_key", "device_key": "<64 hex>",
  "iat", "exp"}` (exp = iat + 3600) and sig the service key's signature over
  `"claktory device assertion v1\n" ‖ payload`. It says that whoever holds
  the tunnel key `device_key` (an iroh endpoint id) is signed in to the
  account, through the device with `account_key`. A Claktory checks it
  offline with its pinned service key, that the account is its own and that
  `device_key` is the key the device connected with, with 60 s of leeway on
  the times (`protocol::verify_device_assertion`).
- **The service share** `S` (32 bytes) = HMAC-SHA256(K_share,
  `"claktory share v1" 0x00 claktory_id 0x00 u64be(epoch)`). Never stored;
  only ever sent sealed.
- **Sealing** to the Claktory's one-time X25519 key `eph_pub`: the service
  makes a one-time X25519 key `epk`, then
  - `key = HKDF-SHA256(ikm = X25519(e, eph_pub), salt = epk ‖ eph_pub, info = "claktory share seal v1")`
  - `sealed_share = b64(epk(32) ‖ nonce(24) ‖ XChaCha20-Poly1305(key, nonce, S, aad))`
  - `aad = "claktory share v1" 0x00 claktory_id 0x00 u64be(epoch)`

  An `eph_pub` that gives an all-zero shared secret is refused.

## Identifiers

- `claktory_id`: made by the Claktory, random, 16–64 of `A-Z a-z 0-9 _ -`.
- `name`: the Claktory's name, shown on claktory.com/account and in emails;
  control characters and anything that looks like a link are removed, then
  it's cut to 40 characters.
- `version`, `platform`: up to 40 characters; platform like `macos-arm64`,
  `linux-x86_64`.
- Endpoint ids: iroh endpoint ids, 64 hex characters.
- A device's `name`: what it's called ("Matt's iPhone"), tidied as a
  Claktory's name.

## Endpoints

### `GET /.well-known/claktory-account`
`{"service": "claktory-account", "api": 1, "version": "0.1.0", "service_key": b64, "service_key_statement": {"text": "…", "signature": "ROOT_ID:…"}}`
(`service_key_statement` is newer, and left out by a service without one.)

### `POST /v1/sign-in/start`
```json
{"email": "…", "claktory_id": "…", "name": "Mac mini", "backend_key": b64, "version": "v0.1.9", "platform": "macos-arm64"}
```
Emails a 6-digit code (it works for 15 minutes, 5 tries), from
`code@<mail domain>` (`code@auth.claktory.com`). Answers the same whether or
not the account exists: `200 {"request_id": "…", "expires_in": 900, "code_from": "code@auth.claktory.com"}`
(`code_from`, the address the code comes from, is newer: a client reads its
absence as unknown).
Errors: `400 invalid_email`, `400 invalid_request`, `429 rate_limited`
(per email 5 an hour and 20 a day, per IP 30 sign-ins a day),
`503 email_unavailable` (the day's email cap is reached, or the mail
provider didn't send it: it failed or refused, or the address is on its
suppression list or bounced).

### `POST /v1/sign-in/verify`
```json
{"request_id": "…", "code": "482913", "eph_pub": b64}
```
Makes the account if it's new, and registers the Claktory (or, signing in
again, gives it a new token, backend key and epoch, and undoes a removal).
It may carry `Claktory-Time` and `Claktory-Signature` (no token), signed as
a signed request by the backend key the Claktory has now: that proves it's
the same Claktory, which then moves from another account it signed out of
rather than `409 claktory_taken`. Signed with another key it proves nothing;
a time that's off is `401 clock_skew` (with `server_time`) and a signature
seen before `401 replayed`, each leaving the code to use again.
`200 {"account_id", "email", "claktory_id", "machine_token", "epoch", "sealed_share", "service_key"}`.
Errors: `400 wrong_code` (`attempts_left`), `410 code_expired` (expired, or
out of tries: start again), `409 claktory_taken` (that id belongs to another
account), `403 too_many_claktories` (10 per account), `400 invalid_request`.

### `POST /v1/share` (signed)
`{"eph_pub": b64, "epoch": 4}` → `200 {"epoch", "sealed_share"}`.
After a restart. `epoch` is optional: without it, the current epoch's share;
with it, that epoch's, which may be the current one or the one before it
only (else `400 invalid_request`): a backend signed in again while its vault
was locked needs the old share once, to reseal its vault under the new one. Each release is recorded with the IP and emails the
account ("Mac mini unlocked its secrets"). Errors: `401 unauthorized`,
`403 revoked` (removed at claktory.com/account), `429 rate_limited` (one a
minute).

### `POST /v1/check-in` (signed)
At start, then hourly while running. Everything but `version` and
`platform` is optional (a check-in without it still works):
```json
{
  "version": "v0.1.9",
  "platform": "macos-arm64",
  "counts": {"agents": 12, "projects": 3, "devices": 2},
  "periods": [
    {
      "id": "p-01HZX…",
      "from": 1763643600,
      "to": 1763647200,
      "usage": [
        {"provider": "anthropic", "model": "claude-opus-4-8", "model_name": "Claude Opus 4.8", "requests": 41,
         "input_tokens": 52000, "output_tokens": 9100, "cache_read_tokens": 1830000, "cache_write_tokens": 64000}
      ],
      "activity": [
        {"hour": 1763643600, "agent_minutes": 95, "agents_working": 3, "runs": 7}
      ]
    }
  ]
}
```
- `counts`: how many agents, projects and paired devices the Claktory has now.
- `periods`: what happened since the last accepted check-in. The Claktory
  makes a period when it checks in, keeps the ones not yet accepted (offline,
  say) and sends them all with its next check-in, oldest first, 400 at most
  per check-in. A `200` means every period sent is recorded: drop them. A
  period sent again (its answer was lost) is recorded once, by its `id`.
  - `id`: made by the Claktory, unique among its periods, 16–64 of
    `A-Z a-z 0-9 _ -`; `from` < `to`, unix seconds, at most 31 days apart.
  - `usage`: model calls in the period, one entry per provider and model
    (1–100 of `A-Z a-z 0-9 . _ : / @ + -` each), at most 200; token counts as
    the provider reported them. `model_name` (optional) is the model's name as
    the app shows it, 1–100 characters, for the account page.
  - `activity`: per UTC hour the period touches (`hour` a multiple of 3600,
    from the hour `from` is in up to `to`): `agent_minutes`, the minutes agents
    spent working, summed over agents; `agents_working`, the most working at
    once; `runs`, the runs started (a prompt or event through to the agent's
    reply).
  - A period that's out of range (ends in the future, starts more than 90
    days ago, a malformed id, an hour outside it, more `agent_minutes` than
    `agents_working` × 60, a value no Claktory could have) is left out and
    counted in `periods_ignored`; the rest are recorded. Don't send it again.

`200 {"relays": ["https://relay-eu.claktory.com"], "server_time": 1763647200, "epoch": 4, "periods_recorded": 1, "periods_ignored": 0}`
(relays empty until Claktory runs its own; `epoch` is the current one, so a
backend notices a new one it missed; `periods_recorded` counts the periods
new to the service). Updates the Claktory's last contact, version,
platform and counts. The service also notes where the request came from:
country, region and city, from its IP with an offline GeoIP file, keeping
only the place, never the IP.
Errors: `401 unauthorized`, `403 revoked`, `400 invalid_request` (malformed,
a count above 1,000,000, or more than 400 periods).

### `GET /v1/stats/public`
For anyone, no sign-in:
`200 {"claktories": 1234, "tokens_total": 1000000000000, "input_tokens": …, "output_tokens": …, "cache_read_tokens": …, "cache_write_tokens": …, "since": 1763600000}`:
Claktories signed in; tokens all Claktories have used, as their check-ins
reported them (kept when an account is deleted): `tokens_total` is the sum
of the four kinds after it; and since when (the first sign-in). Counted at most
once a minute; `Cache-Control: public, max-age=300` and
`Access-Control-Allow-Origin: *`, for a counter on claktory.com.

### `PUT /v1/endpoints` (signed)
`{"backend": "<64 hex>", "devices": ["<64 hex>", …], "name": "Mac mini", "color": "blue", "relays": ["https://…"]}` → `200 {"count": 3}`.
Replaces this Claktory's iroh endpoint ids for the relay allowlist (20 at
most: `400 too_many_endpoints`). `name`, `color` and `relays` (newer, each
optional) are the Claktory's own name and color and the relays its pairing
codes give, for devices that find it through the account: kept until it
sends them again, the name tidied as at sign-in, a color that isn't a
label (up to 40 of `A-Z a-z 0-9 . _ + -`) and relays that aren't http(s)
URLs (200 characters at most, 8 kept) left out rather than refused.
Errors: `401`, `403 revoked`.

### `POST /v1/move` (signed)
A Claktory moved to another computer (#217): its new copy changes its
credentials, so the old copy's no longer work.
```json
{"backend_key": b64, "machine_token": b64, "name": "cloud-box", "version": "v0.1.15", "platform": "linux-x86_64", "from": "Mac mini"}
```
Signed with the token and backend key it has now; `backend_key` and
`machine_token` (32 random bytes, which the Claktory chooses) replace them
at once. The epoch, and so the share, stays the same. `name` is the new
computer's name (tidied as at sign-in), `from` the old one's, for the email.
`200 {"epoch"}`. The account is emailed ("Mac mini moved to cloud-box").
A Claktory whose answer was lost tries again with the old pair, gets `401
unauthorized`, and checks whether its new pair works with a check-in.
Errors: `400 invalid_request` (a key or token that isn't 32 bytes, or a
token already in use), `401`, `403 revoked`, `429 rate_limited` (10 a day).

### Devices

A person's devices sign in to the account themselves, to find its
Claktories and connect to them without a pairing code. A Claktory still
decides who gets in: the first device of a Claktory signed in less than 30
minutes ago at once, every other one once the person allows it on a device
already in (or with a pairing code too).

#### `POST /v1/devices/sign-in/start`
```json
{"email": "…", "account_key": b64, "name": "Matt's iPhone", "platform": "ios", "version": "v0.1.13"}
```
Emails a 6-digit code (15 minutes, 5 tries) for signing in on that device,
which works only with that account key. Limits are a Claktory sign-in's,
counted together with them. `200 {"request_id", "expires_in": 900, "code_from"}`.
Errors: `400 invalid_email`, `400 invalid_request`, `429 rate_limited`,
`503 email_unavailable`.

#### `POST /v1/devices/sign-in/verify` (device signed, by the key that started it)
`{"request_id", "code"}` → `200 {"account_id", "email", "name"}`. Makes the
account if it's new and binds the account key to it (from another account,
it moves, and the old account's assertions for it leave the allowlist).
Errors: `400 wrong_code`, `410 code_expired`, `401 unauthorized` (another
key, which spends none of the code's tries), `403 too_many_devices` (30 per
account).

#### `GET /v1/devices/me` (device signed)
`200 {"account_id", "email", "name"}`, or `401 unauthorized` when the key isn't
bound: never signed in, signed out, or removed at claktory.com/account.

#### `POST /v1/devices/sign-out` (device signed)
`{}` → `200 {}`: the key is unbound (also when it wasn't bound), and the
device keys of its assertions leave the allowlist.

#### `GET /v1/claktories` (device signed)
`200 {"claktories": [{"claktory_id", "name", "color", "machine", "platform", "version", "endpoint_id", "relays": [], "last_contact_ms"}]}`:
the account's Claktories that aren't removed, newest contact first. `name`
and `color` are the Claktory's own (until it sends them, its computer's name
and ""); `machine` is its computer's name; `endpoint_id` and `relays` are what
its pairing codes give (`e=`, `r=`), empty if it never registered.

#### `POST /v1/devices/assertion` (device signed)
`{"device_key": "<64 hex>"}` → `200 {"assertion", "expires_in": 3600}`, for
the tunnel key the device connects to one Claktory with (a device may use
one per Claktory). 60 an hour per account key. Its `device_key` goes on the
relay allowlist for a day after its last assertion. Errors: `400
invalid_request`, `401 unauthorized`, `429 rate_limited`.

### `GET /v1/relay/allowlist?since=<version>` (relays only)
`Authorization: Bearer <relay bearer>` →
`200 {"version": 7, "endpoints": ["<64 hex>", …]}`, or
`200 {"version": 7, "unchanged": true}` when `since` is the current version.
Endpoints of Claktories that aren't removed, and the device keys
assertions were made for in the last day (dropped within the hour after),
of devices still signed in. For Claktory's own relays,
later: `claktory-account relay-access` runs beside a relay and answers its
`access.http` checks from this list.

### Web: `/account`
Sign in with an emailed code; lists the account's Claktories (name, version,
platform, last contact, recent unlocks with their IP, and what their
check-ins sent: place, counts, agent hours in the last 7 days, tokens per
model in the last 30); **Remove** a Claktory (what `revoked` means above); the devices signed in
(name, platform, last seen), each with **Remove** (its key is unbound);
**Delete account** (its Claktories' usage stays only in anonymous totals; `GET /account/delete` opens its confirm step directly, after the emailed-code sign-in if need be, for apps to link to);
**Download my data** (JSON, everything kept about the account). Whoever runs
the service (`--admin-email`) also gets **Usage counts** at `/account/admin`,
the same report as `claktory-account stats`, including the emails the mail
provider refused today and yesterday, by why. The first refusal of a kind
each day (the account isn't entitled to send, sending is turned off, the
token is refused, the sending is throttled…) is also emailed to them, and
logged.

### `GET /mark.png`
Claktory's mark, as the sign-in emails show it from `--public-url`: a PNG,
cached a week, and not counted against the IP's requests a minute, since mail
providers fetch it through a few image proxies.

## Error codes

| HTTP | `error` | |
|---|---|---|
| 400 | `invalid_request` | malformed JSON or a field out of range |
| 400 | `invalid_email` | |
| 400 | `wrong_code` | `attempts_left` |
| 400 | `too_many_endpoints` | |
| 401 | `unauthorized` | unknown token, or a bad signature |
| 401 | `clock_skew` | `server_time` |
| 401 | `replayed` | |
| 403 | `revoked` | the Claktory was removed from its account |
| 403 | `too_many_claktories` | |
| 403 | `too_many_devices` | |
| 404 | `not_found` | |
| 409 | `claktory_taken` | |
| 410 | `code_expired` | |
| 429 | `rate_limited` | `retry_after` |
| 500 | `internal` | the service's fault; try again later |
| 503 | `email_unavailable` | |
