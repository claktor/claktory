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
- `{"type": "purge_account", "account_id", "time"}`: the account was deleted (its calls already fail), so its apps forget it. Answered with 204. The service queues it in the same transaction as the deletion (`apps_purges` in its database) and asks until it gets that 204: at once, then a minute later, waiting twice as long each time up to an hour, so an account deleted while the worker is down is purged once it's back. A purge has to be safe to repeat.
- `{"type": "signed_out", "account_id", "account_key", "time"}`: a device of the account signed out at `time`, or was removed (row 671), so its DM leaf's KeyPackages put up until then go and nobody adds it to a group meanwhile; any it put up later (signed back in before this came) stay. Answered with 204. The service queues it with the sign-out (`apps_sign_outs` in its database) and asks until it gets that 204, as with a purge, so it has to be safe to repeat. Meanwhile an account's own device drops any left when it next reads its keys (`signed_out` below).

The worker's answer head is `{"v": 1, "status": N}`, followed by the body (JSON; empty with 204).

`caller` is what the root verified, never what the caller said:

- `{"kind": "device", "account_id", "device_key", "registrations": [{"claktory_id", "epoch"}]}`: a person's device signed in to the account (`Authorization: Device …`, signed). `registrations` are the account's Claktories signed in now; `bound_devices` (absent from an older service) the account's device keys signed in now, the caller's included.
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

## DM keys at rest (`dms::keys`)

DMs are service-readable, not end-to-end encrypted (dms.md §5). What keeps a copy of the worker's database, or a backup of it, from yielding any DM text on its own:

- **The wrapping key.** It's 32 random bytes in a file of its own (`--dm-key`, made by `init-key`, 0600, refused if anyone else can read it; or a systemd credential, `LoadCredentialEncrypted`, given as a file directly in the service's `$CREDENTIALS_DIRECTORY`, which is root's and read through an ACL, so there and only there its group may read it (0440), never write it, and others have no access). It's kept apart from the database and its backups, and backed up offline by itself. It's none of the account service's keys, and nothing else derives from it.
- **Data keys.** There's one per purpose: `account:<id>` for an account's profile and codes, one per conversation. Each is kept in `dm_keys`, sealed with the wrapping key (XChaCha20-Poly1305) and bound to its id, its purpose and the wrapping key's id. Forgetting a data key forgets everything sealed with it: purging an account forgets `account:<id>` and `account:<id>:…`.
- **Fields.** A body, a profile or a code's reveal value is sealed with its purpose's data key and bound to its table, record and column, so a value moved elsewhere doesn't open. The format is a format byte (1), a cipher byte (1), the data key's id (16 bytes), a fresh nonce (24 bytes), then the ciphertext and tag. Any failure (tampered, moved, cut, another format, a key gone) is the same refusal, and it never says what the field holds.
- **Refusals.** The database records its wrapping key's id. A worker given another key refuses to start, so a restore with the wrong key fails loudly and writes nothing. A database whose schema is newer than the worker's (`user_version`) is refused too.

Restoring takes the database copy and the same key file. After a wrapping key may have been exposed, `rewrap --db FILE --from OLD --to NEW` (worker stopped) seals every data key with a new one, in one transaction or not at all; backups made before still need the old key.

Rewrapping changes only the wrapping key; the data keys stay the same. It fully answers a leaked key file when no copy of the database leaked with it. Someone holding the old key and any copy of the database sealed with it (an older backup) recovers the data keys, and those open what's written after the rewrap too: that needs a full rekey (new data keys, every field sealed again), which isn't built yet and is needed only once DMs carry real traffic. So the old wrapped keys don't linger in the live file, the worker zeroes what it deletes (`secure_delete`), and a rewrap ends by rebuilding the file (`VACUUM`) and emptying its WAL.

## Your own DM identity (`dms::identity`, D1a)

A person's identity in DMs: a profile (a name they choose and a generated avatar), their privacy (whether anyone may send them a friend request), and their secret friend ID. Only the person's own signed-in devices reach these routes. A Claktory, an agent or the account page gets `403 device_only`. Nothing here looks up anyone else: there's no search, directory or profile-by-ID. Everything is made the first time a device asks: an empty name, a generated avatar, requests on, and a friend ID.

- **Friend ID.** 20 random bytes (160 bits), shown as `cf1_` and 32 base32 letters (RFC 4648 alphabet). When typed, case, spaces and hyphens don't matter, and the prefix is optional; anything else isn't one. It's kept two ways. One is an HMAC-SHA256 digest keyed by a data key of its own (`lookup:friend-codes`), which is how a request finds its owner. The other is the owner's sealed copy (`account:<id>`), so any of their devices can show it again. The code itself is never in a path, a log or an answer other than reveal and reset.
- **Reset** makes a new generation and retires the old code for good. Its digest stays as a tombstone, its sealed copy goes, and it's never reassigned. The device sends the generation it saw, so a reset from another device in between gets `409 stale`. Requests still waiting through the old generation close.
- **Profile and privacy** each have a revision, and a change names the revision it was made from (`409 stale` otherwise). The profile is sealed. A name is trimmed, runs of spaces become one, and it holds 1 to 80 characters with no control, direction-changing or invisible ones. Nor may it say Claktory, even spaced out, with `-`, `.` or `_` between letters, or with letters that read alike (`1`, `I` or `|` for l, `0` for o, Cyrillic, Greek or fullwidth lookalikes), so no one can message as "Claktory Support".
- **Limits:** 30 reveals an hour, 5 resets a day, and 60 profile or privacy changes an hour, per account (`429 rate_limited`).
- **Purging an account** deletes its identity. Its friend IDs are left retired and owned by no one, and its data keys are forgotten.

| Route (device signed) | Body | Answer |
|---|---|---|
| `GET /v1/apps/dms/me` | | `{"profile": {"name", "avatar_seed", "revision"}, "privacy": {"requests_on", "revision"}, "friend_id": {"generation", "created_ms"}}`. Never the code. |
| `POST /v1/apps/dms/me/friend-id/reveal` | `{"request_nonce"?}` | `{"code", "generation"}` |
| `POST /v1/apps/dms/me/friend-id/reset` | `{"generation", "request_nonce"?}` | `{"code", "generation"}`: the new one |
| `PUT /v1/apps/dms/me/profile` | `{"name"?, "new_avatar"?, "revision", "request_nonce"?}` | as `GET me` |
| `PUT /v1/apps/dms/me/privacy` | `{"requests_on", "revision", "request_nonce"?}` | as `GET me` |

`request_nonce` (any string, in the body or a GET's query) only makes otherwise identical signed requests differ, since the service refuses a replayed signature; the worker ignores it. Unknown fields are `400 invalid_request`. Refusals are `{"error", "message"}`: `400 invalid_request`, `403 device_only`, `409 stale`, `429 rate_limited`, `503 unavailable`.

## Requests, friends and blocks (`dms::contacts`, D1b)

Two people become friends only when one sends a request with the other's friend ID and the other accepts it, each from their own signed-in device. No agent or Claktory can send, accept or block (`403 device_only`). A request carries no words: only the sender's own name and avatar, shown to the person it reached, and when it was sent.

- **No oracle.** Every well-formed request gets the same answer: `202` with a receipt and "Request sent. If that's their friend ID, they'll see it." That holds whether the code is unknown, retired or the sender's own, or its owner has requests off, blocked the sender (or was blocked by them), declined them in the last 30 days, already has a request from them, is already their friend, sent them one first (crossing), or has 100 waiting. Every request runs the same lookups, uses up the same quota and writes one receipt. A request that reached no one keeps no target and is never delivered later. Only a code that doesn't parse is `400`. The receipt reads `pending` until it's accepted (`accepted`, with the contact) or cancelled, or it expires 30 days on. Declined, closed and undelivered receipts read `pending` too.
- **Accept** reads the request, blocks and friendships inside one IMMEDIATE transaction. It makes a contact with a new id, a new `conversation_id` and the pair's next `generation`. Any other request between the two closes; a crossing one still reads `pending` to its sender. Accepting again answers the same contact. A request that's cancelled, declined, closed (its target reset their friend ID, turned requests off, or blocked) or expired is `410 gone`.
- **Decline** tells the sender nothing. That sender's requests reach the person again only after 30 days, unless the person **reopens** to them first (`declines`): then their requests reach the person again, and the request declined comes back to wait for an answer, if it hasn't expired and would still reach them (sent to the current friend ID, requests on, no block, not friends, nothing else waiting between them). The sender is told nothing either way. A decline's `id` is a keyed hash of who declined whom, since nobody sees an account id.
- **Cancel** works on a request that wasn't accepted first; if it was, it's `409 accepted`, and undoing it means removing the friend.
- **Block** names a request that reached the person, or one of their contacts, since nobody ever sees an account id. Requests either way stop reaching, what's pending between them closes, their friendship ends, and every grant either has on their conversation loses it (`grants::drop_resource`). **Unblock** restores nothing. **Remove** ends a friendship the same way, without the block. Reconnecting takes a new request and acceptance, and makes a new conversation.
- **Reset** of a friend ID, and turning requests off, close every request waiting for the person. Their own requests out can still be accepted.
- **Tag:** each person a viewer sees (a requester, a contact, someone blocked) has a `tag`: the first 6 base32 letters of HMAC(lookup key, viewer, them). It tells two people with the same name apart. It stays the same through resets and renames, differs for every viewer, and can't be used for anything else.
- **Limits:** 5 requests a day in someone's first week, 20 after (counted from their DM identity's creation), and 120 accepts, declines, reopens, blocks, unblocks, removals and cancels an hour.
- **Deleting an account** ends its friendships (and their agents' access), drops its requests, and closes the ones sent to it. Their senders' receipts read as before.

| Route (device signed) | Body | Answer |
|---|---|---|
| `POST /v1/apps/dms/requests` | `{"code", "action_id", "request_nonce"?}` | `202 {"receipt": {"id", "state": "pending", "created_ms", "expires_ms"}, "message"}`. `action_id` is the device's id for this request, the same on a retry (22–64 of `A–Z a–z 0–9 - _`, 128 random bits or more); a retry answers the same receipt. |
| `GET /v1/apps/dms/requests` | | `{"incoming": [{"id", "from": {"name", "avatar_seed", "tag"}, "created_ms", "expires_ms"}], "outgoing": [{"id", "state", "contact_id"?, "created_ms", "expires_ms"}]}` |
| `POST /v1/apps/dms/requests/<id>/accept` | `{}` | the contact |
| `POST /v1/apps/dms/requests/<id>/decline` | `{}` | `204` |
| `POST /v1/apps/dms/requests/<id>/cancel` | `{}` | `204`; `409 accepted` |
| `GET /v1/apps/dms/contacts` | | `{"contacts": [{"id", "name", "avatar_seed", "tag", "conversation_id", "generation", "since_ms"}]}` |
| `DELETE /v1/apps/dms/contacts/<id>` | | `204` |
| `POST /v1/apps/dms/blocks` | `{"request_id"}` or `{"contact_id"}` | `{"id", "name", "avatar_seed", "tag", "created_ms"}` |
| `GET /v1/apps/dms/blocks` | | `{"blocks": [as above]}` |
| `DELETE /v1/apps/dms/blocks/<id>` | | `204` |
| `GET /v1/apps/dms/declines` | | `{"declines": [{"id", "from": {"name", "avatar_seed", "tag"}, "until_ms"}]}`: whom the person declined in the last 30 days |
| `DELETE /v1/apps/dms/declines/<id>` | | `{"request": {"id", "from", "created_ms", "expires_ms"} or null}`: reopened, with the request that waits again, if any |

Others' requests, contacts, blocks and declines are `404 not_found`. Each change tells the person's devices through their outbox (Sync, below): a request that reached someone tells theirs too, one that reached no one tells nobody (in the same statement, run either way).

## Messages (`dms::messages`, `dms::bodies`, D2a)

Two friends write to each other in their conversation (a friendship's `conversation_id`; reconnecting starts a new one), from their own devices, or through an agent's proposal they approved (below).

- **Durable send.** A message is appended in one IMMEDIATE transaction that, inside it, checks the sender may (one of the two people; an agent's comes only from a proposal its person approved, and needs a live `dm.send` grant on the conversation, checked first, every time, retries included), that the friendship is on (not removed, nobody blocked) and has the generation the sender saw (`409 stale` otherwise), looks the message up by the sender's id for it, takes the conversation's next `seq`, stores the body and adds a `message` event to both people's outboxes (`dm_account_events`, each account's own order, for sync: D2b). The receipt (`message_id`, `seq`, `accepted_ms`) comes only after it commits: that's when the app shows Sent.
- **Ids and retries.** `client_message_id` is the sender's id for the message, the same on every retry (22–64 of `A–Z a–z 0–9 - _`, 128 random bits or more), unique per author, actor (the person, or the agent with its Claktory, registration and incarnation), and conversation. A retry with the same body (and generation and actor) answers the first receipt, even once the friendship has ended; other text is `409 conflict`, never a second message. `request_nonce` only makes the signed requests differ.
- **Block or removal first** (either person, either order of commit): the send is `410 closed`, the same words whatever ended it. A send that commits first is an earlier message and stays. History stays readable to both after a friendship ends; an ended conversation is listed while it has messages, showing the other person as they were when it ended (each side's view sealed with the conversation's key, `dm_ended_views`), never their name or avatar as they change after.
- **Bodies are opaque** to everything but `dms::bodies`: a format number and bytes, plus a keyed digest retries are compared by. Format 1 (now) is the text sealed with the conversation's data key (`conversation:<id>`, D5a) and bound to its message; format 2 is reserved for the sender's own end-to-end ciphertext, which would change only that module. Text is 1 to 16,384 bytes of UTF-8.
- **Limits:** 60 messages a minute and 1,000 an hour from a person's devices; an agent's are its grant's. A history page is at most 100 messages and 256 KiB of bodies. A sender whose space is full (Sync) is `413 storage_full`.
- **Deleting an account** drops its outbox; its messages stay in its friends' histories, theirs to keep.

| Route (device signed) | Body | Answer |
|---|---|---|
| `GET /v1/apps/dms/conversations` | | `{"conversations": [{"id", "contact_id", "contact_generation", "with": {"name", "avatar_seed", "tag"}, "active", "since_ms", "ended_ms", "last_seq", "view"}]}`: `view` as in Sync |
| `POST /v1/apps/dms/conversations/<id>/messages` | `{"client_message_id", "contact_generation", "text", "request_nonce"?}` | `{"message_id", "seq", "accepted_ms"}`; `409 conflict`, `409 stale`, `410 closed` |
| `GET /v1/apps/dms/conversations/<id>/messages?after=N` or `?before=N`, `&limit=` (50, up to 100) | | `{"messages": [{"id", "seq", "from": "you"\|"them", "by_agent", "accepted_ms", "body": {"text"}, "client_message_id"?}], "has_more"}`, oldest first: after N, or the newest before N (the latest without either), none the person deleted; `client_message_id` on the person's own (from any of their devices), to match a send whose answer was lost |

Someone else's conversation is `404 not_found`. Agents reach conversations only through their own routes (Agents, below).

## Sync (`dms::sync`, D2b)

How every device of a person keeps up, with nothing queued or kept on a device that it can't fetch again.

- **Outbox.** Every change a person's devices need adds an event to that account's own outbox (`dm_account_events`, numbered in its own order) in the same transaction as the change. Fetching uses nothing up: every device reads the same outbox from its own cursor. Events are kept 30 days (pruned hourly); the numbering carries on past what was pruned.
- **Snapshot, then changes.** A device starts with `GET sync`: everything it shows, and a cursor. Then `GET sync?since=<cursor>` answers up to 100 events (or 256 KiB) after it, with the next cursor and `has_more`. `wait=1..25` holds the answer until something happens (only an event in that account's own outbox wakes it), or the wait is over and it answers no events. A device (or Claktory) may have 4 waiting at once and an account 8; more are answered at once, without waiting. A cursor that was never handed out, or one from before what was pruned, is `410 resync_required`: start again from a snapshot, and page back through history for older messages.
- **What each person keeps of a conversation** (`dm_views`), never seen by the other: how far they've read (only ever forward, at most the last message; agents reading don't move it), how far they've deleted their copy (only forward; deleting reads too), and their draft, kept sealed like a message, replaced only from the revision the device saw (`409 stale` with the draft as it is otherwise, so two devices never silently overwrite each other), refused once the conversation ended (`410 closed`). Empty text clears it.
- **Deleting** a conversation's history through a message hides it on all the person's devices for good; the other person's copy stays. A message both people deleted (or whose other person's account is gone) leaves the store. An ended conversation is listed while the person has something left of it.
- **Space.** Each account keeps at most 100 MiB of messages (its conversations' after its deletion cutoff, its own and the other's, sealed size): past it, sending is `413 storage_full` until it deletes some. The other person's sends still reach it.

| Route (device signed) | Body | Answer |
|---|---|---|
| `GET /v1/apps/dms/sync` | | `{"conversations": [as listed], "requests": {as GET requests}, "contacts": [as GET contacts], "proposals": [as GET proposals], "cursor", "usage": {"bytes", "quota"}}` |
| `GET /v1/apps/dms/sync?since=<cursor>&limit=` (100) `&wait=` (0–25 s) | | `{"events": [{"kind", "conversation_id"?, …}], "next_cursor", "has_more"}`; `410 resync_required` |
| `PUT /v1/apps/dms/conversations/<id>/read` | `{"seq", "request_nonce"?}` | `{"read_seq"}` |
| `PUT /v1/apps/dms/conversations/<id>/draft` | `{"text", "revision", "request_nonce"?}` | `{"draft": {"text", "revision"}}`; `409 stale` with `"draft"`; `410 closed` |
| `POST /v1/apps/dms/conversations/<id>/delete` | `{"through_seq", "request_nonce"?}` | `{"deleted_seq"}` |

A view is `{"read_seq", "deleted_seq", "draft": {"text", "revision"}}`. Events, by `kind`:
- `message`: `"message"`, as in history, or `{"id", "seq", "deleted": true}` once the person deleted it;
- `read`, `draft`, `deleted`: `"view"`, the conversation's as it is now;
- `conversation` (one began or ended), `requests`, `contacts`, `blocks`, `me`: fetch that again;
- `agent_draft` (D4a): `{"id", "revision"}` of an agent's suggested draft;
- `proposal` (D4b): `{"id", "revision"}` of an agent's proposed message, proposed, revised or decided: fetch `GET proposals` again.

## Agents (`dms::agents`, D4a)

An agent of one of the person's Claktories reaches their DMs only through a grant the person approved (Grants), and only these routes: a Claktory's backend calls them for one of its agents (`?agent=&incarnation=`), and each checks the agent's grant, and counts it against the scope's quota, inside the same IMMEDIATE transaction as what it reads or writes. Device-only routes stay device-only, and these refuse every other caller.

- **Only what was granted.** The conversations the grant names; history from the grant on (a message in the grant's own second isn't shown), at most 100 messages and 64 KiB a page, without what the person deleted. Reading never moves the person's read position. No friends, requests, blocks, profile or friend ID.
- **Drafts, never sends.** An agent's draft is a suggestion for the person, apart from their own composer draft, sealed like a message (`dm_agent_drafts`), for an active friendship of the generation the agent saw (`409 stale`; ended: `410 closed`). The same id and text answers the same; other text revises it while it's open. At most 3 open in a conversation per agent, and the grant's quota (dm.draft: 5 a minute, 20 an hour). Each change adds an `agent_draft` event to the person's outbox. Their devices list them (also in the sync snapshot's `agent_drafts`: who left one in which conversation, never the text) and close one when the person uses or dismisses it; the agent can't change a closed one. Sending stays the person's.
- **Following.** An agent with dm.subscribe and dm.read follows a conversation; its Claktory long-polls `agent/events` (no agent named), which answers only messages from the other person, in conversations one of that Claktory's agents follows with a live grant naming it, sent after the grant began: ids, never text, each counted against that grant's dm.subscribe quota. Each poll is the backend's own call, authenticated by the root then, so a delivery is checked against its registration as it is.
- **Deleting an account** drops its agents' drafts and follows.

| Route | Caller | Body | Answer |
|---|---|---|---|
| `GET /v1/apps/dms/agent/conversations` | agent, any dms scope | | `{"conversations": [{"id", "contact_generation", "with": {"name", "tag"}, "active", "last_seq", "from_ms", "subscribed"}]}` |
| `GET /v1/apps/dms/agent/conversations/<id>/messages?after=N` or `?before=N`, `&limit=` | agent, dm.read | | as a device's history page |
| `PUT /v1/apps/dms/conversations/<id>/agent-drafts/<client id>` | agent, dm.draft | `{"contact_generation", "text", "agent_name"?, "request_nonce"?}` | `{"draft_id", "revision", "accepted_ms"}`; `409 stale`, `410 closed`, `429 too_many_drafts` |
| `GET /v1/apps/dms/conversations/<id>/agent-drafts` | device | | `{"drafts": [{"id", "agent": {"name", "claktory_id"}, "revision", "body": {"text"}, "created_ms", "updated_ms"}]}`, open ones, newest first |
| `DELETE /v1/apps/dms/conversations/<id>/agent-drafts/<id>` | device | | `204` |
| `PUT`, `DELETE /v1/apps/dms/agent/subscriptions/<id>` | agent, dm.subscribe and dm.read to follow | | `204` |
| `GET /v1/apps/dms/agent/events` or `?since=<cursor>&wait=` (1–25 s) | the backend | | `{"events": [{"agent": {"id", "incarnation"}, "conversation_id", "seq"}], "next_cursor"}`; without `since`, none and the cursor now; `410 resync_required` |

`agent_name` is what the Claktory calls its agent, shown on the draft as it says (one line, at most 64 characters); the service can't check it.

## Agents' messages, reviewed (`dms::review`, D4b)

An agent sends a message only once the person approved exactly it, from one of their own devices; its Claktory never says "approved".

- **Proposal.** The agent's Claktory proposes, for the agent (`?agent=&incarnation=`), with a live `dm.send` grant on the conversation: the text, sealed like a message, bound to the proposal's revision, the conversation and its generation, the agent (Claktory, registration epoch, incarnation, the name shown) and the grant. Each proposal or revision counts as one use of the grant's `dm.send`; a retry (same id, same text) doesn't. The same id with other text, while it waits, is the next revision; once decided, `409 conflict`. At most 5 wait per grant and conversation (`429 too_many_waiting`); each waits 3 days.
- **Review.** The person's devices list what waits (`GET proposals`, also in the sync snapshot, with a `proposal` event at each step) with everything the content hash covers. The device computes the hash itself from what it shows, and approves with it and the revision: a hash or revision that isn't the current one is `409 changed`. The approval's transaction checks the agent's Claktory is still registered under that epoch (from the person's own sign-in, not the backend), the same grant still holds `dm.send` on the conversation, and the friendship is on at that generation, then appends the text once as the agent's message (`by_agent`). Approving again answers the same. It works with the agent's Claktory off.
- **Can't be sent any more** (the grant revoked or replaced, the Claktory signed in again or out, the friendship ended or blocked): approving is `410 unsendable` and it's `closed` for good; it's no longer listed. Declined, withdrawn or expired ones are `410 unsendable` too.
- **Use draft** is the device copying the text into the person's own composer: their Send is their own message, nothing here.
- **What the agent learns:** only what became of its own proposal (`state`, and `message_id`/`seq` once sent), never the person's words; another incarnation of it, nothing. `GET proposals/<id>?wait=1..25` waits while it's `pending`. Decided and expired proposals are deleted a week later.

**Content hash** (v1): SHA-256, lowercase hex, of `"claktory-dm-review/1\n"` then, for each of `proposal`, `revision`, `conversation`, `generation`, `claktory`, `agent`, `incarnation`, `agent_name`, `text` in that order, `name ":" <length in bytes> ":" <UTF-8 value> "\n"` (numbers in decimal). Golden vector: proposal `0123456789abcdef0123456789abcdef`, revision 2, conversation `c0ffee`, generation 1, claktory `mac-mini`, agent `june`, incarnation `inc-1`, agent_name `June`, text `Sounds good \u2014 see you at 6 \u202Eok` → `a93bb7251af328211f8bee5cb78eed29f2a66553887be93dfa295c474da0aea6`.

| Route | Caller | Body | Answer |
|---|---|---|---|
| `POST /v1/apps/dms/conversations/<id>/proposals` | the agent's Claktory, for it | `{"client_message_id", "contact_generation", "text", "agent_name", "request_nonce"?}` | status: `{"proposal_id", "revision", "state", "expires_ms", "waiting", "message_id"?, "seq"?}` |
| `GET /v1/apps/dms/proposals/<id>?wait=` | that agent | | status |
| `DELETE /v1/apps/dms/proposals/<id>` | that agent | | status (`withdrawn`, if it waited) |
| `GET /v1/apps/dms/proposals` | the person's device | | `{"proposals": [{"id", "revision", "state", "conversation_id", "contact_generation", "from": {"claktory_id", "agent_id", "agent_incarnation", "agent_name"}, "text", "content_hash", "created_ms", "expires_ms"}]}` |
| `GET /v1/apps/dms/proposals/<id>` | the person's device | | as listed |
| `POST /v1/apps/dms/proposals/<id>/approve` | the person's device | `{"revision", "content_hash", "request_nonce"?}` | status (`sent`); `409 changed`, `410 unsendable`, `413 storage_full` |
| `POST /v1/apps/dms/proposals/<id>/decline` | the person's device | `{"request_nonce"?}` | status (`declined`); `409` once sent |

`state` is `pending`, `sent`, `declined`, `withdrawn`, `expired` or `closed`.

## Reports (`dms::reports`, D-S5)

A person reports someone who sent them a request, a friend (now or before) or someone they blocked, to Claktory's operators, from their own devices only (`403 device_only` otherwise). Nobody else is told, the person reported least of all, and reporting neither declines nor blocks.

- **Only what they chose.** A report keeps why (`reason`: `spam`, `harassment`, `impersonation` or `other`) and what it's about (a request, with when it was sent; a friendship, with when it began and ended; a block, with when), plus, only if the person adds them, their own words (`note`, at most 1,000 characters, no hidden or direction-changing ones) and the name and avatar the person reported shows them (`include_profile`); about a contact, messages of their conversation the person ticked (`messages`: at most 50, oldest first, each `{"seq", "text", "from": "you"|"them", "sent_ms"}` with up to 16 KiB of text, 64 KiB in all). Never anything else of theirs. Messages are end-to-end encrypted, so those are the text the reporter's device shows, which nobody here can check (no franking in v1): operators see them labelled so (`messages_label`), kept as sent, hidden and direction-changing characters too (they may be what's reported), each named by its code in the message's `hidden`.
- **Sealed:** that evidence is sealed with a data key of the report's own (`report:<id>`), forgotten with the report 30 days after it was made (the worker deletes them at start and every hour), so it opens to nothing in backups after that too. The reporter's list shows whom it's about as they showed when it was made (sealed the same way), so it follows none of their later changes.
- **Operators** read open reports with `claktory-user-apps reports --db FILE --dm-key FILE --operator NAME` (JSON lines: reporter and reported account ids, subject, reason, evidence), which records each one as read by NAME, with the time (`dm_report_views`), and close one with `--close ID`, recorded the same way; its reporter then sees it `reviewed`. Suspension and appeal are still to come.
- **Deleting an account** keeps the reports it made, as it chose to send them, from no one; reports about it stay until they expire.
- **Limits:** 20 reports a day. `action_id` makes a retry the same report.

| Route (device signed) | Body | Answer |
|---|---|---|
| `POST /v1/apps/dms/reports` | `{"action_id", "request_id"` or `"contact_id"` or `"block_id", "reason", "note"?, "include_profile"?, "messages"?}` (`messages` only with `contact_id`) | `{"id", "about": {"name", "avatar_seed", "tag"}, "subject", "reason", "state": "received", "created_ms", "message"}` |
| `GET /v1/apps/dms/reports` | | `{"reports": [as above, without "message"; "state" "received" or "reviewed"]}` |

Reporting someone else's request, contact or block is `404 not_found`.

## End to end (E2)

DMs become end-to-end encrypted with MLS (RFC 9420) through `claktory-dm-crypto` (dms-e2ee.md in the research folder is the design). The worker becomes a delivery service: it stores and relays MLS messages it can't read, and checks what it can from their framing. Where this section and the routes above differ, this section holds. The parts: leaves, identities and KeyPackages (`dms::leaves`); groups, mailboxes, commits, Welcomes and the Claktory's feed (`dms::groups`); device hand-offs (`dms::handoffs`); conversation messages, agent drafts and proposals as envelopes (`dms::messages`, `dms::agents`, `dms::review`, `dms::bodies`). Each time the worker starts it deletes whatever format-1 (worker-sealed) body, draft and agent draft is left, all test data, with the conversations that leaves empty and their keys; format 1 is refused.

**Leaves.** Every member of a group is a leaf, with a certificate the person's identity key signed: a **device** (its leaf id is the device's account key, the `device_key` the root verified) or a **Claktory** (its leaf id is `claktory_id` and registration epoch, `certificate::claktory_leaf_id`). The worker checks each leaf it sees against its caller: a device uploads and acts only as its own leaf, a Claktory only as its leaf of the current registration epoch. It can't check a leaf's identity signature against what friends pinned; every client does.

**Envelopes.** Every MLS message travels as `{"format": 2, "bytes": "<base64url, no padding>"}` (`envelope::Envelope`). The worker reads the kind, group and epoch from the bytes, never from anything declared: application messages are PrivateMessages, commits and proposals PublicMessages. Caps: application 20 KiB; commit, proposal, Welcome and GroupInfo 128 KiB; KeyPackage 4 KiB. A retry is compared by SHA-256 of the format-2 bytes: resend the exact bytes.

**Groups** (`dm_groups`). Three kinds:

| Kind | Subject | Members | Who fetches |
|---|---|---|---|
| `conversation` | a conversation and its generation | both people's devices, plus the Claktories granted on it | both people's devices; a Claktory while one of its agents has a live grant naming the conversation, from the grant's start |
| `self` | an account | that account's devices | that account's devices |
| `channel` | an account and one of its Claktories | that account's devices and that Claktory | those devices; that Claktory while its registration epoch holds |

A group's id is 16 to 32 random bytes its creator chose (base64url in paths and answers). One current group per subject; making another for a subject that has one is `409 exists`. Only a device makes one, of a subject that's its account's: a conversation it's one of the two people in, at its current generation; its account's self group; a channel with one of its account's Claktories. The GroupInfo's only member must be the caller's own leaf. The worker keeps each group's public state (OpenMLS `PublicGroup`, `storage::SqlStorage::public`), its epoch and its last `seq`, written in the same transaction as the message that moves them.

**Mailbox.** Each group has one sequence (`seq`) for everything in it: application messages and commits, in the order the worker accepted them, which is the order every member processes them. Kept 30 days (the self group's read positions, 7); a member further behind rejoins (below). Proposals and agent drafts are kept with their own routes, not in the channel's mailbox, so no device spends its one decryption on them twice.

**Commits.** One per epoch per group, first come: a commit for any epoch but the current one is `409 epoch` with `{"epoch"}`; fetch, process, and make it again. A commit comes with its Welcome (when it adds anyone) and the new epoch's GroupInfo, in one request, stored in one transaction with the new public state: the GroupInfo's epoch must be the commit's new one, and the Welcome's recipients exactly the KeyPackages the commit adds. Budget: 120 commits an hour per leaf, 600 per account; a new security key's moves, and Removes of the account's own leaves its current key and list no longer vouch for (a removed or lost device's), have a budget of their own, 1000 an hour per leaf and 5000 per account, so a person with many chats isn't held up. A Claktory commits only its own Update. **Revoking a grant** from a device set up for messages commits the Claktory leaf's Remove from that conversation in the same action (Fugo L6); only when the revoke came from somewhere that can't commit (the account page) does the Claktory propose its own Remove, which the next device commit takes in, while the worker already stops delivering to it. Application messages more than 5 epochs old are refused (`409 epoch`).

**Rejoin.** A member that's too far behind, or lost its state, rejoins by an external commit made from the group's GroupInfo, through the commits route. The worker lets a device rejoin a group of its own account's (and a conversation it's one of the people in), and a Claktory only its own channel; members then check the leaf against the person's device list. A Claktory never rejoins a conversation: a device adds it again. The joining leaf must be the caller's own, certified by the account's current identity and on its current list, and the commit may remove only its own earlier leaf. A new device joins its account's groups the same way, once a device of the account has put the list naming it (and handed it the identity, `me/handoffs`). A device that lost every key and started over under a new security key rejoins the same groups this way, past its account's leaves under the old key, then takes those out in its next commit, an ordinary one of its own account's leaves (OpenMLS puts no other leaf's Remove in an external commit); members send nothing to a group that still holds such a device leaf. A conversation's group that doesn't hold a device of one of its two people yet (row 676) takes a rejoin only from that person's devices: the other side's, already in, get `409 waiting_for_friend`, and wait for a member to add them, so none of them runs ahead of the friend.

**What a commit may do.** Adds, removes and updates only. Every added KeyPackage must be one the worker handed out (`key-packages/claim`; the claim is used up), certified by its account's current identity and on its list, and belong in the group: a conversation's two people's devices while they're friends, and a Claktory only while one of its agents holds a live DM grant on it; a self group's owner's devices; a channel's owner's devices and its Claktory. The new epoch's GroupInfo must match the worker's public state exactly (context and confirmation tag).

**A new security key** (`me/keys/rotate`, dms-e2ee §2.1, R3). The old key is kept as replaced, and no rotation goes back to it (L5); a device list it signs, and a KeyPackage it certified, is refused, and its waiting KeyPackages are dropped. A member's new leaf, when a commit has one, must be its own, certified by the account's current key and on its current list (`403 not_current`). A leaf whose certificate is under a replaced key is inert: its proposals are refused, and so is any commit of its but its own move to the new key, that is an update path to its own leaf under the current key, on the list, with the same signature key, and nothing else in the commit (no proposal of any kind); a commit taking in such a leaf's earlier proposal by reference is refused too (`403 replaced_key`). Its application messages aren't checked here, since that would take loading the group for every message; members drop them.

**KeyPackages.** Each leaf keeps up to 100 one-time KeyPackages and one last-resort one at the worker. Claiming hands one out per leaf and deletes it in the same transaction (the last-resort one is handed out again, and the answer says so). Only the owner's own devices, and a friend's in an active friendship, may claim an account's: a friend gets only its device leaves', never a Claktory's, which only the owner's devices claim. 60 claims an hour per claimer.

**Identity and device lists.** Each account has its identity key (`identity::IdentityPublic`) and its signed device list (`device_list::SignedDeviceList`) at the worker, as opaque signed data that the worker checks only for its signature by the key the account first uploaded. A new list's version must be higher (compare-and-set). Changing the identity key ("Change your security key", or a new one after every device was lost) is a route of its own, so a plain upload can't swap it; it tells the account's devices (`keys` event, M2) and its friends'.

### Routes

Paths are under `/v1/apps/dms/`. "Device" is one of the account's signed-in devices; "Claktory" is its backend calling for its own leaf (no agent named) unless an agent is named.

| Route | Caller | Body | Answer |
|---|---|---|---|
| `GET keys/<account_id>` | device: its own account, or a friend's (active or ended friendship); Claktory: its own account, or the friend in a conversation granted to one of its agents | | `{"account_id", "identity_key", "device_list", "version", "replaced_identity_keys"}` (keys and list base64url): `replaced_identity_keys` the keys the account replaced, oldest first (ties in key order), which a device behind retires only when a commit it holds was made under one (and not at all if the list names a current key). To the account's own devices also `signed_out`: the device leaves on the list whose key isn't signed in now (signed out, or removed and still listed), in the list's order, empty when none; their KeyPackages are dropped as it answers. Nothing is taken off the list for them: that's the person's Remove… |
| `PUT me/keys` | device | `{"identity_key"?, "device_list", "version", "previous_version"}`: `identity_key` only the first time | the account's keys; `409 stale` with them (`{"error", "message", "keys"}`, `keys` null before the first upload) if `previous_version` isn't current; `409 identity_changed` if `identity_key` isn't the account's; `409 removed_device` if the list names a device key removed from the account (the service passes them with the caller): a removed device never comes back |
| `POST me/keys/rotate` | device | `{"identity_key", "device_list", "version": 1}` | the account's keys; 3 a day; never a key the account replaced; `409 removed_device` as for `PUT me/keys`; `403 sign_in_again` unless the device signed in with an emailed code in the last 15 minutes (the service passes when, `signed_in_at`, with the caller): a session alone, stolen or left open, makes no new key, with a device to hand it over or none |
| `POST key-packages` | device, Claktory | `{"key_packages": [envelope…], "last_resort"?: envelope}`, each for the caller's own leaf, certified by the account's identity and on its list | `{"remaining", "last_resort"}`; `409 not_set_up` before the account's first `PUT me/keys`. The leaf's held ones under another certificate are dropped first: a former install's (a reinstall keeps the leaf id, and maybe its key), whose secrets went with its store |
| `GET key-packages` | device, Claktory | | `{"remaining", "last_resort"}`: the caller's leaf's supply |
| `POST key-packages/claim` | device | `{"account_id", "leaf_ids"?}`: every leaf of that account in its device list, or those named | `{"key_packages": [{"leaf_id", "key_package": envelope, "last_resort"}]}`; the account's own devices (from a service that sends `bound_devices`) get none for a leaf that isn't signed in now: a device not among `bound_devices`, a Claktory's whose id and epoch aren't among `registrations` (signed out, moved to another account, or signed in again under a new leaf), and that leaf's KeyPackages are dropped |
| `POST groups` | device | `{"kind", "conversation_id"?, "contact_generation"?, "claktory_id"?, "group_info": envelope}`: the GroupInfo at epoch 0 (with the ratchet tree extension), the creator its only member | `{"group_id", "epoch": 0, "last_seq": 0}`; `409 exists` |
| `GET groups` | device, Claktory | | `{"groups": [{"group_id", "kind", "conversation_id"?, "contact_generation"?, "accounts"?: [two account ids], "claktory_id"?, "epoch", "last_seq", "grants"?: [{"agent": {"id", "incarnation"}, "from_ms"}]}]}`: the groups the caller may fetch; `accounts` (the two people) for a conversation, `grants` for a Claktory's conversations |
| `GET groups/<id>/messages?after=N&limit=` (50, up to 100) | who fetches it | | `{"messages": [{"seq", "kind": "application"\|"commit"\|"proposal", "envelope", "accepted_ms", "epoch", "from"?: "you"\|"them" (conversations), "by_agent"?, "client_message_id"? (your own account's)}], "has_more"}`, oldest first, at most 256 KiB a page; `410 too_far_behind` (with `"epoch"`) past the mailbox: rejoin |
| `POST groups/<id>/commits` | a member's leaf; a rejoining one (above) | `{"commit": envelope, "welcome"?: envelope, "group_info": envelope}` | `{"seq", "epoch"}`; `409 epoch` |
| `POST groups/<id>/proposals` | a member's leaf | `{"proposal": envelope}`: only a leaf's own Remove | `{"seq"}` |
| `POST groups/<id>/messages` | a member's leaf, in self and channel groups only (a conversation's go through its own route); a Claktory only in its own channel, never a self group | `{"client_message_id", "envelope"}` | `{"seq", "accepted_ms"}`; the same id with the same bytes is the same message, with other bytes `409 conflict` |
| `GET groups/<id>/group-info` | who fetches it, and a device that may rejoin it | | `{"epoch", "group_info": envelope}` |
| `GET welcomes` | device, Claktory | | `{"welcomes": [{"id", "group_id", "kind", "conversation_id"?, "contact_generation"?, "accounts"?, "claktory_id"?, "welcome": envelope, "seq", "accepted_ms"}]}`: those for the caller's leaf; `seq` is the commit that added it, so the leaf reads on after it |
| `DELETE welcomes/<id>` | its leaf | | `204` |
| `POST conversations/<id>/messages` | device | `{"client_message_id", "contact_generation", "envelope"}` | `{"message_id", "seq", "accepted_ms"}`; as today, plus `409 epoch`, `409 not_set_up` (no group for this generation yet) and `409 waiting_for_friend` (row 676: the group hasn't held a device of each of the two people, on their current device lists, since the message's epoch or before; nothing is kept or counted, and the message goes once they're both in, at an epoch from then on); `400` for another group's message. Its `seq` is the group's mailbox seq, the same in the conversation's history |
| `GET conversations/<id>/messages`, `GET agent/conversations/<id>/messages` | as today | | as today, `body` replaced by `"envelope"` and `"epoch"` |
| `POST conversations/<id>/proposals` | the agent's Claktory, for it | `{"client_message_id", "contact_generation", "envelope", "agent_name"?}`: an application message in the Claktory's channel | status, as today; `409 epoch` for a channel epoch too old, `409 not_set_up` with no channel. The same id with other bytes while it waits is the next revision (the same waiting slot, its expiry renewed) |
| `GET proposals/<id>?wait=`, `DELETE proposals/<id>` | that agent | | status, as today: never the envelope |
| `GET proposals`, `GET proposals/<id>` | device | | as today, `text` and `content_hash` replaced by `"envelope"`, `"epoch"` and `"proposal_digest"` (lowercase hex SHA-256 of its bytes) |
| `POST proposals/<id>/approve` | device | `{"revision", "proposal_digest", "envelope"}`: the approved message, read from the channel and sealed again in the conversation's group | status (`sent`); `409 changed`, `409 epoch`, `409 not_set_up`, `409 waiting_for_friend` (nothing decided), `410 unsendable` |
| `PUT conversations/<id>/agent-drafts/<client id>` | agent, dm.draft | `{"contact_generation", "envelope", "agent_name"?}`: an application message in the Claktory's channel | as today; `409 epoch`, `409 not_set_up` with no channel |
| `GET conversations/<id>/agent-drafts` | device | | as today, `body` replaced by `"envelope"` |
| `GET claktory/events?since=&wait=` (1–25 s) | Claktory | | `{"events": [{"kind": "group_message", "group_id", "seq"} or {"kind": "welcome", "id"} or {"kind": "groups"}], "next_cursor"}`: its leaf's groups; without `since`, none and the cursor now; `410 resync_required` |
| `POST me/handoffs` | device | `{"to": "<device_key>", "sealed": "<base64url, at most 512 KiB>"}`: `to` a device leaf on the account's current list (PUT it first), never one removed from the account (`409 removed_device`) | `{"id"}`; 3 waiting per device, 10 an hour per sender |
| `GET me/handoffs` | device | | `{"handoffs": [{"id", "from", "sealed", "accepted_ms"}]}`: those to the caller's own device, for 30 minutes |
| `DELETE me/handoffs/<id>` | its device | | `204` |

A device's outbox (Sync) gets `group_message` (`group_id`, `seq`), `welcome` (`id`), `groups`, `keys` (`account_id`) and `handoff` (`id`) events; a conversation's message gives both `group_message` and `message`. `PUT conversations/<id>/read` and `PUT conversations/<id>/draft` are gone: read positions and drafts travel in the self group, and a view's `draft` reads empty. `POST conversations/<id>/delete` stays, as the account's own `hidden_through`. A friend shows with their `account_id` (whose keys a device fetches): in `with` of `GET conversations` and the Sync snapshot's conversations, and in `GET contacts` and an accepted request's contact; never in a request or a block. `agent/events` and `GET agent/conversations` stay as they are; the latter adds each conversation's `group_id` (null before it has one) and the friend's `account_id`.

New refusals: `409 epoch` (`{"epoch"}`: the group's current one), `409 not_set_up` (no group or channel to send in yet), `409 waiting_for_friend` (a conversation's group without both people yet), `409 exists`, `410 too_far_behind`, and `400 invalid_request` for an envelope that isn't what the route takes (format 1 included), with the reason.

## Running it

```sh
claktory-user-apps init-key /etc/claktory-user-apps/dm.key   # once
claktory-user-apps serve --db /var/lib/claktory-user-apps/apps.db --dm-key /etc/claktory-user-apps/dm.key \
  --socket /run/claktory-user-apps/root.sock --root-uid <claktory-account's uid>
claktory-account serve … --user-apps /run/claktory-user-apps/root.sock --user-apps-uid <claktory-user-apps's uid>
```

`claktory-user-apps backup --db FILE --to DIR [--keep N]` writes a consistent copy while it runs (`apps-<UTC time>.db`), keeping the newest N (7).

The socket is made mode 0660. The deploy gives its folder to the worker's user, with the account service's group able to reach it.
