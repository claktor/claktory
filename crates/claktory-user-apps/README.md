# claktory-user-apps

Claktory's user-level apps are what a person keeps at their account rather than in any one Claktory: DMs first. This crate is their worker, a process of its own beside the account service (the root, `claktory-account`), and the narrow channel between the two (`ipc`). The design is in the research folder's `apps-levels.md` §4 and `build-plan-apps-dms-browser.md` §5 (U1a, U1b).

## Why a process of its own

The root holds the keys that matter most: the service key, which signs device assertions and every answer, and `k_share`, which releases vault shares. An app is the first place another person's content (DM texts, profiles) is parsed inside that trust zone. So an app runs in the worker, with:

- its own OS user, database and data folder (the deploy's);
- none of the root's keys, flags or files: it never sees `secrets.json`;
- no network: it listens on one local socket and answers only the root's user (checked on every connection with the peer's credentials, besides the socket's permissions). The root checks the same way that what it connects to runs as the worker's user (`--user-apps-uid`), so a socket someone else put there while the worker was down gets nothing;
- its own limits: 32 requests at once, then a wait of up to 5 seconds, then `503 busy`;
- no way to speak as the root: the root passes on only a JSON body with an app's statuses ([`ipc::acceptable`]), and signs it as it signs every answer, bound to the request it answers (`claktory-account response v1`). That signature can't be taken for a device assertion or a statement, which have labels of their own.

When the worker is down, or isn't configured, every app route answers `503 app_unavailable`. The root has no fallback.

## The channel

There's one request per connection to the worker's socket. A frame is a 4-byte big-endian length and that many bytes of JSON (the head), then a 4-byte length and the body's bytes. A head is at most 64 KiB and a body at most 1 MiB. The root waits up to 30 seconds for an answer.

The root's request head is `{"v": 1, "request": …}`, with `request` one of:

- `{"type": "call", "method", "path", "query", "time", "caller"}`: an HTTP call under `/v1/apps/`, which the root authenticated; its body follows as it came. `time` is the root's (unix seconds). `query` leaves out `agent` and `incarnation`, which are in the caller.
- `{"type": "health"}`: 200 when the worker is up.
- `{"type": "purge_account", "account_id", "time"}`: the account was deleted (its calls already fail), so its apps forget it. Answered with 204.

The worker's answer head is `{"v": 1, "status": N}`, followed by the body (JSON; empty with 204).

`caller` is what the root verified, never what the caller said:

- `{"kind": "device", "account_id", "device_key", "registrations": [{"claktory_id", "epoch"}]}`: a person's device signed in to the account (`Authorization: Device …`, signed). `registrations` are the account's Claktories signed in now.
- `{"kind": "backend", "account_id", "claktory_id", "epoch", "agent": {"id", "incarnation"} | null}`: a Claktory's backend (`Authorization: Bearer <machine token>`, signed with its backend key). It acts for one of its agents when the call's query has `agent=<id>&incarnation=<incarnation>`, which its signature covers. Both are needed, once each: letters, digits, `- _ . :`, 64 at most. A device names no agent.

## Grants (`grants`)

A grant lets one agent of one Claktory use scopes of one app:

- It binds the agent's id and incarnation (factoryd mints a new incarnation for a new agent of the same id).
- It binds the epoch of the Claktory's registration it was made under. Signing out, being removed, or signing in again ends that registration, so the grant never matches again, and it's purged when next listed.
- It binds the resources it names, when the app declares them. DMs: `conversation` resources, and a DM grant must name at least one.

Grants live in the worker's database with the app's own data, so an app checks one inside its own write transaction (`grants::check`, which also counts the use against the scope's per-minute and per-hour quota). A revoke can't land between the check and the write. Anything unknown is refused: an app, a scope, a resource kind, or a caller that isn't an agent.

| Route | Caller | What it does |
|---|---|---|
| `PUT /v1/apps/grants` | backend | `{"app", "agent": {"id", "incarnation"}, "scopes": […], "resources": [{"kind", "id"}], "expires_ms": 0}` makes the agent's grant in that app, replacing the one it had. Answered with the grant. factoryd offers this only to its Rivet, after the person approves. The service can tell only that the backend asked. |
| `GET /v1/apps/grants` | backend | `{"grants": […]}`: this Claktory's live grants, for its mirror. |
| `GET /v1/apps/grants` | device | `{"grants": […]}`: the account's live grants, across its Claktories. |
| `DELETE /v1/apps/grants/<id>` | device, backend | Revokes one: a device any of its account's, a backend any of its own. 204, or 404 for one that isn't theirs. |

A grant reads `{"id", "app", "claktory_id", "agent": {"id", "incarnation"}, "scopes", "resources", "granted_ms", "expires_ms"}`. Refusals are `{"error", "message"}`:

- `400 invalid_request`
- `403 not_granted`
- `404 not_found`
- `429 rate_limited`, with `retry_after`
- `503`

An account holds 200 grants at most. A grant has up to 16 scopes and 64 resources.

A call from a person's device carries the account's live registrations, so an app can check a deferred actor's registration (a person approving an agent's stored proposal, say) against them in the same request. A delivery the worker starts by itself (an agent's subscription firing because someone else wrote) has no such caller. That needs a narrow, read-only query from the worker to the root for one registration's current epoch, which comes with the DM work that first needs it (D4a), and is never cached beyond one operation.

`GET /v1/apps/whoami` answers with the caller as the worker got it, so a device or backend can check what reaches the apps.

## Running it

```sh
claktory-user-apps serve --db /var/lib/claktory-user-apps/apps.db \
  --socket /run/claktory-user-apps/root.sock --root-uid <claktory-account's uid>
claktory-account serve … --user-apps /run/claktory-user-apps/root.sock --user-apps-uid <claktory-user-apps's uid>
```

`claktory-user-apps backup --db FILE --to DIR [--keep N]` writes a consistent copy while it runs (`apps-<UTC time>.db`), keeping the newest N (7).

The socket is made mode 0660. The deploy gives its folder to the worker's user, with the account service's group able to reach it.
