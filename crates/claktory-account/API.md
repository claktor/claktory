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
- Limits a minute: a signed request from a Claktory or a signed-in device
  counts against its key (240 a minute, from any address), never its
  address, since a person's Claktory and devices, or a household, often
  share one. Everything else (sign-in, codes, web pages, public answers)
  counts against the address, 30 a minute; so do signed requests that
  don't verify (no key, an unknown one, a bad signature), and once an
  address has 30 of those in a minute its signed requests are refused
  unchecked until the next, except those naming a Claktory (its token) or
  a signed-in device (its account key) the service knows: they're checked
  and limited by their key as ever, so others' bad requests on a shared
  address (a carrier's NAT, an office) don't lock out a person's devices.
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
  the endpoint key `device_key` (an iroh endpoint id) is signed in to the
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
Errors: `400 invalid_email` (not an address, or one with an invisible
character that changes how it reads, such as U+202E or a zero-width space),
`400 invalid_request`, `429 rate_limited`
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

### The review account (`--review-account`)
For the stores' reviewers, who need sign-in details that work at any time,
from anywhere, without a one-time code. Off unless the service is started
with `--review-account FILE` and FILE is there (the operator's
`review-account off` removes it). FILE (a systemd credential, `chmod 600`:
one others may read stops the service) is one line,
`<email> <salt> <sha256(salt || code), hex>`, with the salt (16 or more
letters and digits) taken as the text it is:
`printf '%s%s' "$salt" "$code" | sha256sum`. The service never holds the
code, and the email is a secret too: no mailbox, never logged (log lines say
"review account"), and never a `--vouch-claktory-email` (the service won't
start with both).

For that email (after the usual normalisation), every sign-in that takes a
code (a Claktory's, a device's, the web pages', Delete account's among them)
works as above, with the same request, 15 minutes, 5 tries, limits and
answers, except:
- no code is emailed, and nothing else is sent to it (unlock and move
  notices included); the code is the fixed one;
- 5 wrong codes in an hour, counted by the email and by the address they
  come from, across sign-ins, lock it for the rest of the hour: every code,
  the right one too, is `400 wrong_code`, and a new start is `429
  rate_limited`;
- each sign-in with it logs one line, `review sign-in: <route>, from
  <region, country>` (`claktory`, `device` or `web`; the place from the
  GeoIP file, never the IP).

It's an ordinary account otherwise. Every other email is unchanged, and the
fixed code opens no other email's sign-in.

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
  "counts": {"agents": 12, "projects": 3, "devices": 2, "pipes": 1, "secrets": 4, "monitors": 2},
  "setup": {"model_connected": true, "first_run": true},
  "devices": [
    {"platform": "macos", "kind": "laptop", "app_version": "v0.1.16", "count": 1},
    {"platform": "ios", "kind": "phone", "app_version": "v0.1.16", "count": 1}
  ],
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
      ],
      "devices_connected": 2,
      "health": [
        {"version": "v0.1.16", "rolled_back": 1, "crashes": 0}
      ]
    }
  ]
}
```
- `counts`: how many agents, projects and paired devices the Claktory has now;
  newer Claktories also its pipes, secrets and active monitors (0 when left
  out). Claktory sends `secrets` only roughly, as 0, 1, 10 or 50 (at least
  that many).
- `setup`: the steps of setup it has reached, each true for good once
  reached: `model_connected` (a model provider signed in), `first_run` (an
  agent ran).
- `devices`: its paired devices that have connected (this computer's own app
  among them), counted by `platform` ("macos", "ios", "android"…), `kind`
  ("desktop", "laptop", "phone", "tablet") and `app_version` ("v0.1.16",
  "dev"), each a label (1–40 of `A-Z a-z 0-9 . _ + -`, "unknown" when the app
  didn't say), at most 100 entries; never a device's name. Replaces the
  previous check-in's (an empty list: none now); left out, the previous
  list stays.
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
  - `devices_connected` (optional): how many of its paired devices connected
    in the period.
  - `health` (optional): per backend `version` (a label; "unknown" when the
    supervisor couldn't tell), at most 20: `rolled_back`, how often an update
    to it didn't start and the supervisor went back, and `crashes`, how often
    it stopped without stopping cleanly (a crash, or the computer losing
    power). Added up across periods.
  - A period that's out of range (ends in the future, starts more than 90
    days ago, a malformed id, an hour outside it, more `agent_minutes` than
    `agents_working` × 60, a value no Claktory could have) is left out and
    counted in `periods_ignored`; the rest are recorded. Don't send it again.

`200 {"relays": ["https://relay-eu.claktory.com"], "server_time": 1763647200, "epoch": 4, "periods_recorded": 1, "periods_ignored": 0}`
(relays empty until Claktory runs its own; `epoch` is the current one, so a
backend notices a new one it missed; `periods_recorded` counts the periods
new to the service). Updates the Claktory's last contact, version,
platform, counts, setup and devices. The service also notes where the request came from:
country, region and city, from its IP with an offline GeoIP file, keeping
only the place, never the IP.
Errors: `401 unauthorized`, `403 revoked`, `400 invalid_request` (malformed,
a count above 1,000,000, a label out of form, more than 100 kinds of
device, or more than 400 periods).

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

### `POST /v1/pipe-identity` (signed)
Verified pipes: the service's word, for the Claktories this one has pipes
with, that whoever holds its pipe key is a Claktory of its account.
```json
{"endpoint_id": "<64 hex>", "time": 1795183320, "proof": b64}
```
`endpoint_id` is the Claktory's tunnel key (its iroh endpoint id, the key
its pipes pin), and must be the `backend` it registered with `PUT
/v1/endpoints`. `proof` is that key's own Ed25519 signature over
`"claktory-pipe-peer/1 proof\n" claktory_id "\n" endpoint_id "\n" time`
(`endpoint_id` in lower case; `time` within 300 s of the service's clock),
so the service vouches only for a key the asking Claktory holds, and a
proof serves no other Claktory or later.
`200 {"statement", "expires_in": 604800}`. The statement is
`b64(payload) "." b64(signature)`, the service key's signature over
`"claktory-pipe-peer/1\n" ‖ payload`, the payload JSON:
```json
{"v": 1, "label": "claktory-pipe-peer/1", "service": "https://account.claktory.com", "service_key": b64,
 "endpoint_id": "<64 hex>", "account_id": "…", "email": "ana@example.com", "iat": 1795183320, "exp": 1795788120}
```
Nothing in it is what the Claktory says of itself (no names), and it
vouches for the Claktory's account and its email (confirmed with a code at
sign-in), not for the person. It lasts a week; the Claktory asks again
about once a day, and a signed-out or removed one, or one whose account is
deleted, gets none, so its peers' verification ends by expiry.
A peer checks it with `verify_pipe_peer` (`protocol.rs`): the signature
with the service key it pinned at sign-in (never the payload's
`service_key`, which must name that same key), the label, `v`, that
`endpoint_id` is the pipe's pinned peer key, `exp - iat` at most a week,
the time (60 s of skew) and an email in printable ASCII, 4096 bytes at most.
Errors: `400 invalid_request` (a malformed endpoint id, a proof that
doesn't verify, or a time more than 300 s off), `401`, `403 revoked`, `403
wrong_endpoint` (not this Claktory's registered backend endpoint, or none
registered yet), `403 unsupported_email` (the account's email isn't
printable ASCII, so it can't be shown to others without look-alikes, or
it's at claktory.com or a subdomain, which would read as Claktory's staff,
unless the operator allowed that address with `--vouch-claktory-email`;
that account stays unverified), `429 rate_limited` (48 a day).

### `POST /v1/reports/ai` (signed, or not)
Report this reply: a person sends one of their agents' (or chats') replies
to Claktory's team, as their app's sheet showed it before they sent it.
Signed by a signed-in Claktory, unsigned by one that isn't.
```json
{"text": "…", "model": "Claude Opus 4.5", "version": "v0.1.30", "platform": "macos-arm64", "note": "…"}
```
- `text`: the reply, the vault's values hidden, at most 16384 bytes (cut on
  a character's boundary by the Claktory, `ai_report_text`); not blank.
- `model`: the model's name, or its id when it's no longer listed; up to 100
  characters. `version`, `platform`: up to 40. All three are one line,
  with no control or format characters, and not blank.
- `note`: optional, as `ai_report_note` leaves it (no control characters
  but line breaks and tabs, no format characters, trimmed), at most 1000
  characters.
- No other field: an unknown one is refused, and so is a body over 40 KB
  (a reply full of line breaks and quotes is twice its size in JSON).

`200 {"keep_days": 30}`. The service keeps exactly those fields and the hour
the report arrived in (never the second), for 30 days (the hourly purge), for Claktory's team to read
on the server; no route serves them. Nothing of who sent it is kept with it:
not the address, account, Claktory or key, signed or not. Who sent it
counts only toward the limits, by a hash, which the purge forgets after
two days. It sends no email and reaches no one.
Limits: unsigned, 10 an hour and 50 a day per address; signed, 20 a day per
Claktory (its address's other limits don't count); 2000 a day from
everyone together. Errors: `400 invalid_request`, `401` (a signature that
doesn't verify), `403 revoked`, `429 rate_limited`.

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
Errors: `400 invalid_email`, `400 invalid_request`, `403 device_removed`
(this key was removed from that account; no code is sent),
`429 rate_limited`, `503 email_unavailable`.

#### `POST /v1/devices/sign-in/verify` (device signed, by the key that started it)
`{"request_id", "code"}` → `200 {"account_id", "email", "name"}`. Makes the
account if it's new and binds the account key to it (from another account,
it moves, and the old account's assertions for it leave the allowlist).
Errors: `400 wrong_code`, `410 code_expired`, `401 unauthorized` (another
key, which spends none of the code's tries), `403 too_many_devices` (30 per
account), `403 device_removed` (the key was removed after the code was sent;
the code is spent).

#### `GET /v1/devices/me` (device signed)
`200 {"account_id", "email", "name"}`, or `401 unauthorized` when the key isn't
bound: never signed in, signed out, or removed (at claktory.com/account or
by another device).

#### `POST /v1/devices/sign-out` (device signed)
`{}` → `200 {}`: the key is unbound (also when it wasn't bound), and the
device keys of its assertions leave the allowlist.

#### `POST /v1/devices/remove` (device signed)
`{"account_key": b64}` → `200 {"removed": true}`: another device of the
same account (lost, stolen, given away) is signed out, as removing it at
claktory.com/account does: its key is unbound, so this service and the
apps behind it (DMs' worker) answer it `401 unauthorized` from then on, and
the device keys of its assertions leave the allowlist. A DM device's leaf id
is its account key. The key is also remembered for good, as when it's
removed at claktory.com/account (never when a device signs itself out): it
never signs in to this account again, even with a right code, and is
answered `403 device_removed` (message: "This device was removed from your
account. Sign in again to start over as a new device."). The app then
deletes what it kept for that account (its DM store) and makes a new account
key, which signs in as a new device and joins DMs only through a hand-off
from a device already in. Only this account is closed to it; other accounts
aren't. The removed keys are in Download My Data, and go when the account is
deleted. `{"removed": false}` when that key isn't (or is no longer) one of
this account's devices, so a retry is no error, and another account's
devices are never touched; the key is remembered all the same (a device
that signed itself out first stays out). Errors: `400 invalid_request` (not a
key, or the caller's own: that's sign-out), `401 unauthorized`, `429
rate_limited` (10 an hour per device).

#### `GET /v1/claktories` (device signed)
`200 {"claktories": [{"claktory_id", "name", "color", "machine", "platform", "version", "endpoint_id", "relays": [], "last_contact_ms"}]}`:
the account's Claktories that aren't removed, newest contact first. `name`
and `color` are the Claktory's own (until it sends them, its computer's name
and ""); `machine` is its computer's name; `endpoint_id` and `relays` are what
its pairing codes give (`e=`, `r=`), empty if it never registered.

#### `POST /v1/devices/assertion` (device signed)
`{"device_key": "<64 hex>"}` → `200 {"assertion", "expires_in": 3600}`, for
any endpoint key the signed-in device uses: the tunnel key it connects to
one Claktory with (a device may use one per Claktory), or the one it uses
for a transfer, such as a DM history transfer's (a fresh one each time).
60 an hour per account key. Its `device_key` goes on the
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

### User-level apps: `/v1/apps/…` (signed, or device signed)
Apps a person keeps at the account rather than in a Claktory (DMs first) run
in the user-apps worker, a process of its own with its own database and none
of the service's keys (`crates/claktory-user-apps`, whose README has the
contract). The service authenticates each call and forwards it there, with who
it's from:
- a Claktory's signed request (`Bearer` token), for itself or for one of its
  agents: `?agent=<id>&incarnation=<incarnation>` on the call, which its
  signature covers, both once;
- a person's device-signed request (`Device`), with the account's Claktories
  signed in now and their epochs. A device names no agent.

What the worker answers is passed on only as JSON with an app's status, and
signed like every answer. Without a worker (`--user-apps SOCKET
--user-apps-uid UID`), while it's down, or when what listens there doesn't
run as the worker's user: `503 app_unavailable`.

Grants (which agent of which Claktory may use what in an app) live with the
app's data:
- `PUT /v1/apps/grants` (signed) `{"app", "agent": {"id", "incarnation"},
  "scopes", "resources": [{"kind", "id"}], "expires_ms"}` → the grant;
- `GET /v1/apps/grants` → `{"grants": […]}`, the Claktory's own (signed) or
  the account's (device signed), live ones only;
- `DELETE /v1/apps/grants/<id>` → `204`, one of the account's (device
  signed) or the Claktory's own (signed).

A grant holds only under the epoch it was made in: signing in again,
signing out or being removed ends it. Refusals: `400 invalid_request`,
`403 not_granted`, `404 not_found`, `429 rate_limited`. `GET
/v1/apps/whoami` answers with the caller as the worker got it.

A person's own DM identity (device signed only; anyone else `403
device_only`), whose shapes and limits are in the worker's README:
- `GET /v1/apps/dms/me` → profile (name, avatar, revision), privacy
  (`requests_on`, revision) and the friend ID's generation, never the code;
- `POST /v1/apps/dms/me/friend-id/reveal` → `{"code", "generation"}`;
- `POST /v1/apps/dms/me/friend-id/reset` `{"generation"}` → the new code;
- `PUT /v1/apps/dms/me/profile` `{"name"?, "new_avatar"?, "revision"}` and
  `PUT /v1/apps/dms/me/privacy` `{"requests_on", "revision"}` → as `GET me`.
A revision or generation another device changed meanwhile is `409 stale`.

Requests, friends and blocks, device signed only too:
- `POST /v1/apps/dms/requests` `{"code", "action_id"}` → `202` and a
  receipt, the same for every well-formed code, whoever's it is or isn't;
- `GET /v1/apps/dms/requests` → incoming (who asked) and outgoing (receipts,
  `pending` or `accepted`);
- `POST /v1/apps/dms/requests/<id>/accept`, `/decline`, `/cancel`;
- `GET /v1/apps/dms/contacts` (each friend with their `account_id`, whose
  keys the person's devices fetch; requests and blocks never show it),
  `DELETE /v1/apps/dms/contacts/<id>`;
- `POST /v1/apps/dms/blocks` `{"request_id"}` or `{"contact_id"}`, `GET
  /v1/apps/dms/blocks`, `DELETE /v1/apps/dms/blocks/<id>`;
- `GET /v1/apps/dms/declines` (whom the person declined in the last 30
  days), `DELETE /v1/apps/dms/declines/<id>` (reopen: their requests reach
  the person again, and the declined one waits again if it still could);
- `POST /v1/apps/dms/reports` `{"action_id", "request_id"|"contact_id"|"block_id",
  "reason", "note"?, "include_profile"?, "messages"?}` and `GET
  /v1/apps/dms/reports`: reports to Claktory's operators, keeping only what
  the person chose; `messages` (only with `contact_id`, at most 50, oldest
  first) are `{"seq", "text", "from": "you"|"them", "sent_ms"}` as the
  reporter's device shows them, labelled not verifiable for operators;
- `GET /v1/apps/dms/conversations` (the friend in `with`, with their
  `account_id`); `POST /v1/apps/dms/conversations/<id>/messages`
  `{"client_message_id", "contact_generation", "envelope"}`: an MLS
  application message (`{"format": 2, "bytes"}`) in the conversation's
  current group, at most 5 epochs old → `{"message_id", "seq",
  "accepted_ms"}` once stored (a retry answers the same); `409 epoch`
  (`{"epoch"}`, the group's now) for an older one, `409 not_set_up` before
  the group exists; `GET
  /v1/apps/dms/conversations/<id>/messages?after=|before=&limit=` → a page
  of history, each message's `envelope` and `epoch`;
- end to end (MLS, the worker a delivery service that can't read what it
  relays): the account's keys and device list (`GET /v1/apps/dms/keys/<account>`,
  `PUT /v1/apps/dms/me/keys`, `POST /v1/apps/dms/me/keys/rotate`),
  KeyPackages (`POST`/`GET /v1/apps/dms/key-packages`, `POST
  /v1/apps/dms/key-packages/claim`), groups and their mailboxes (`POST`/`GET
  /v1/apps/dms/groups`, `GET /v1/apps/dms/groups/<id>/messages`, `POST …/commits`,
  `…/proposals`, `…/messages`, `GET …/group-info`), Welcomes (`GET
  /v1/apps/dms/welcomes`, `DELETE …/<id>`), a Claktory's feed of them (`GET
  /v1/apps/dms/claktory/events?since=&wait=`, signed) and hand-offs between
  the person's devices (`POST`/`GET /v1/apps/dms/me/handoffs`, `DELETE …/<id>`);
  shapes in the worker's README, "End to end";
- `GET /v1/apps/dms/sync` → a snapshot and a cursor; `GET
  /v1/apps/dms/sync?since=<cursor>&wait=` → what happened after it, waiting
  up to 25 seconds for something (`410 resync_required`: start again);
  `POST /v1/apps/dms/conversations/<id>/delete`: what the person keeps of
  a conversation, on all their devices (read positions and drafts travel
  end to end, in the person's own group);
- an agent's message is sent only once the person approves exactly it:
  its Claktory proposes it for the agent (`POST
  /v1/apps/dms/conversations/<id>/proposals` `{"client_message_id",
  "contact_generation", "envelope", "agent_name"?}`, signed, with a `dm.send`
  grant, the envelope sealed in the Claktory's own channel group), and only
  the person's device approves (`POST /v1/apps/dms/proposals/<id>/approve`
  `{"revision", "proposal_digest", "envelope"}`: the digest it was shown,
  SHA-256 of the proposal's bytes, and what it showed sealed again in the
  conversation's group; `409 epoch` or `409 not_set_up` as for a message)
  or declines; the backend never can;
- `GET /v1/apps/dms/conversations/<id>/agent-drafts` and `DELETE …/<id>`:
  the drafts agents left the person, and closing one.

A Claktory for one of its agents, within the agent's grant (signed, with
`?agent=&incarnation=`), and only these:
- `GET /v1/apps/dms/agent/conversations` (with each one's `group_id` and the
  friend's `account_id`) and `GET
  /v1/apps/dms/agent/conversations/<id>/messages?after=|before=&limit=`
  (`dm.read`): the conversations the grant names, history from the grant on;
- `PUT /v1/apps/dms/conversations/<id>/agent-drafts/<client id>`
  `{"contact_generation", "envelope", "agent_name"?}` (`dm.draft`, the
  envelope sealed in the Claktory's channel): a draft for the person, never
  a send;
- `PUT`/`DELETE /v1/apps/dms/agent/subscriptions/<id>` (`dm.subscribe`).
And for itself, `GET /v1/apps/dms/agent/events?since=<cursor>&wait=`: which of
its agents a new message from the other person should wake (ids only).

### Web: `/account`
Sign in with an emailed code; lists the account's Claktories (name, version,
platform, last contact, recent unlocks with their IP, and what their
check-ins sent: place, counts, agent hours in the last 7 days, tokens per
model in the last 30); **Remove** a Claktory (what `revoked` means above); the devices signed in
(name, platform, last seen), each with **Remove** (its key is unbound, and never signs in to the account again: `403 device_removed`);
**Agents' access**, the grants agents on its Claktories hold in the user-level
apps (agent, Claktory, app, what it may do, how many conversations, since
when), each with **Remove**, asked of the user-apps worker, so it works with
every Claktory off;
**Delete account** (its Claktories' usage and backend versions' health stay only in anonymous totals; their devices are deleted; its user-level apps' data is purged by the user-apps worker, retried until it answers if it's down; `GET /account/delete` opens its confirm step directly, after the emailed-code sign-in if need be, for apps to link to);
**Download my data** (JSON, everything kept about the account). Whoever runs
the service (`--admin-email`) also gets **Usage counts** at `/account/admin`,
the same report as `claktory-account stats` (daily and monthly active
Claktories and 30-day retention, from the days each checked in; totals of
their counts, setup reached, each backend version's rollbacks and crashes,
and their devices by system, kind and app version: totals only, never one
Claktory's), including the emails the mail
provider refused today and yesterday, by why. The first refusal of a kind
each day (the account isn't entitled to send, sending is turned off, the
token is refused, the sending is throttled…) is also emailed to them, and
logged.

### `GET /mark.png`
Claktory's mark, as sign-in emails sent before v0.1.16 showed it from
`--public-url`: a PNG, cached a week, and not counted against the IP's
requests a minute, since mail providers fetch it through a few image proxies.
Emails now draw the mark in HTML and load nothing (Gmail hides remote images).

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
| 403 | `device_removed` | this device's key was removed from the account (by another of its devices, or at claktory.com/account); it never signs in to that account again |
| 403 | `wrong_endpoint` | not this Claktory's registered backend endpoint |
| 403 | `unsupported_email` | an email a pipe statement can't carry (not printable ASCII, or claktory.com's own) |
| 404 | `not_found` | |
| 409 | `claktory_taken` | |
| 410 | `code_expired` | |
| 429 | `rate_limited` | `retry_after` |
| 500 | `internal` | the service's fault; try again later |
| 403 | `not_granted` | an app call no grant allows |
| 503 | `email_unavailable` | |
| 503 | `app_unavailable` | the user-apps worker isn't configured, running or answering rightly |
