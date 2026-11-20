# Connecting services (one-click secrets): the contract

Status: **approved** by Mike (2026-11-21); the protocol is in factory.proto. Builds on Iris's design,
`factory-research/one-click-secrets.md`, plus Matt's four rules. Backend 2
(Lena) owns the broker core and the protocol; Sam (Backend) the Cloudflare and
AWS recipes and Rivet's tools; Priya (Frontend) the app.

## Words

- **Connector**: a reviewed recipe built into factoryd, one per service
  (`cloudflare`, `aws`). It knows the service's endpoints, its public client,
  the access choices it offers, and how to sign in, refresh, list, mint and
  revoke.
- **Connection**: one account the person connected, kept in the vault as a
  secret of kind *connection*. Its tokens (access, refresh, client
  registration) are **broker-only**: no variables, never given to an agent,
  never revealed, never in a Workspace. It can't be granted.
- **Export**: a secret *made from* a connection with narrower access: a
  minted Cloudflare token for one account with some permissions, or an AWS
  account and role. This is what agents are given (by Rivet or the person),
  with the existing grants. It has ordinary variables.
- **Proposal**: an agent's request to connect a service, shown to the person
  as a card. Only agents allowed to propose have the tool.
- **Flow**: one sign-in in progress, bound to the device that started it.

## Matt's rules, as built

1. Secrets extension option **`connect`**, "Can propose connecting services"
   (switch, off by default). Only agents with it on get the tool
   `secrets_propose_connection`. Rivet or the person turns it on per agent.
2. A proposal shows as a card in the agent's conversation (an Entry) and at
   the top of Settings → Secrets (VaultState.proposals). The person clicks
   Connect: the app starts a flow, opens the service's page on the device,
   catches the callback and hands it to the backend over its connection (the
   tunnel too). The backend exchanges the code itself and seals the result
   in the vault.
3. A connection is granted to **no one**, the proposer included. When one is
   made for a proposal, Rivet (each admin agent with Secrets on) gets an event
   with the proposal and what the connection actually reaches. Rivet inspects
   it (Sam's tools: provider, account, scopes, resources and expiry, never
   values), makes an export with the least access the work needs, and gives
   that with `secrets_grant`. The person can do the same in Settings.
4. The person can connect without a proposal: Settings → Secrets → Add →
   a service.

## Protocol (add-only)

`proto/factory/v1/factory.proto` is the source of truth, with every field's
comment; this is its outline.

```proto
// ClientMessage
StartConnect start_connect = 90;           // → ConnectStarted
FinishConnect finish_connect = 91;         // → Ack (the result shows in VaultState.flows)
CancelConnect cancel_connect = 92;         // → Ack; also removes an ended flow from the list
DismissConnectProposal dismiss_connect_proposal = 93;  // → Ack
ListConnectChoices list_connect_choices = 94;          // → ConnectChoices
DeriveSecret derive_secret = 95;           // → Ack (the export shows in VaultState.secrets)
DisconnectService disconnect_service = 96; // → Ack

// ServerMessage (to the asker only)
ConnectStarted connect_started = 34;
ConnectChoices connect_choices = 35;

// Entry
ConnectProposal connect_proposal = 13;     // the proposing tool call's card, kept up to date

// VaultState
repeated Connector connectors = 6;         // the services Claktory can connect (this version's catalog)
repeated ConnectProposal proposals = 7;    // pending ones, oldest first (decided ones live on in the Entry)
repeated ConnectFlow flows = 8;            // flows in progress or just ended, every device sees them

// SecretInfo
SecretConnection connection = 13;          // set when the secret is a connection
SecretExport export = 14;                  // set when it was made from one

message Connector {
  string id = 1;                    // "cloudflare", "aws"
  string name = 2;                  // "Cloudflare"
  string description = 3;           // what a connection gives, one sentence
  repeated ConnectorAccess accesses = 4;
  repeated ConnectorField fields = 5;     // asked before connecting (AWS: portal URL, region)
  ConnectorFlowKind flow = 6;
  // BROWSER only: the callbacks it accepts; "{port}" is any port. The app
  // picks one it can receive and sends it, port filled in, in StartConnect.
  //   "http://127.0.0.1:{port}/oauth/cloudflare"   (Mac: a loopback listener)
  //   "com.claktory.app:/oauth/cloudflare"         (iPhone, ASWebAuthenticationSession)
  //   "com.claktory.app.dev:/oauth/cloudflare"     (Claktory Dev)
  repeated string redirect_uris = 7;
  repeated string variables = 8;    // what an agent given an export of it gets
  string setup_note = 9;            // what the person needs first, one or two sentences; "" none
  string setup_url = 10;            // where to read about it; "" none
}

message ConnectorAccess {
  string id = 1;                    // "deploy"
  string label = 2;                 // "Deploy websites"
  string why = 3;                   // the plain sentence the person reads
  repeated string scopes = 4;       // exactly what's asked of the service
  bool preselected = 5;             // the default choice
}

message ConnectorField {
  string id = 1;                    // "start_url", "region"
  string label = 2;
  string placeholder = 3;
  string hint = 4;
  bool optional = 5;
}

enum ConnectorFlowKind {
  CONNECTOR_FLOW_KIND_UNSPECIFIED = 0;
  // The app opens the service's page and catches the callback (Cloudflare).
  CONNECTOR_FLOW_KIND_BROWSER = 1;
  // The app opens a verification page with a code; the backend polls, no
  // callback (AWS IAM Identity Center).
  CONNECTOR_FLOW_KIND_DEVICE = 2;
}

message ConnectProposal {
  string id = 1;
  string agent_id = 2;
  string connector_id = 3;
  string access_id = 4;             // the ConnectorAccess it asks for; "" for the preselected one
  string purpose = 5;               // what it's for, in the agent's words
  string access = 6;                // the access it needs: permissions, resources (zones, accounts, roles)
  string reason = 7;                // why
  ConnectProposalState state = 8;
  int64 created_ms = 9;
  int64 decided_ms = 10;
  string comment = 11;              // the person's, on dismissing
  string secret_id = 12;            // the connection made for it
  string secret_name = 13;
}

enum ConnectProposalState {
  CONNECT_PROPOSAL_STATE_UNSPECIFIED = 0;
  CONNECT_PROPOSAL_STATE_PENDING = 1;
  CONNECT_PROPOSAL_STATE_CONNECTED = 2;
  CONNECT_PROPOSAL_STATE_DISMISSED = 3;
}

message StartConnect {
  string connector_id = 1;
  string access_id = 2;             // "" for the preselected one
  map<string, string> fields = 3;   // ConnectorField id → value
  string proposal_id = 4;           // the proposal it answers, if any
  string redirect_uri = 5;          // BROWSER: the receiver the app made; "" for DEVICE
  string secret_id = 6;             // reconnect this connection in place; "" for a new one
}

message ConnectStarted {
  string flow_id = 1;
  string url = 2;                   // open this (DEVICE: the verification page, code filled in)
  string user_code = 3;             // DEVICE: the code the page shows, to check
  int64 expires_ms = 4;
}

message FinishConnect {
  string flow_id = 1;
  string callback_url = 2;          // the whole URL the browser came back to, as is
}

message CancelConnect { string flow_id = 1; }

message ConnectFlow {
  string id = 1;
  string connector_id = 2;
  string device_name = 3;           // where it was started
  ConnectFlowState state = 4;
  string error = 5;                 // FAILED: a sentence for the person
  string secret_id = 6;             // CONNECTED: the connection
  string proposal_id = 7;
  int64 started_ms = 8;
  int64 expires_ms = 9;
}

enum ConnectFlowState {
  CONNECT_FLOW_STATE_UNSPECIFIED = 0;
  CONNECT_FLOW_STATE_WAITING = 1;   // for the person, in the browser
  CONNECT_FLOW_STATE_EXCHANGING = 2;
  CONNECT_FLOW_STATE_CHECKING = 3;  // reading the account and what was granted
  CONNECT_FLOW_STATE_CONNECTED = 4;
  CONNECT_FLOW_STATE_FAILED = 5;
  CONNECT_FLOW_STATE_EXPIRED = 6;
  CONNECT_FLOW_STATE_CANCELLED = 7;
}

message DismissConnectProposal { string proposal_id = 1; string comment = 2; }

message SecretConnection {
  string connector_id = 1;
  string account = 2;               // whose: "matt@example.com", "d-1234 portal (us-east-1)"
  repeated string access = 3;       // what it can do, sentences from what the service granted
  repeated string scopes = 4;       // exactly what was granted
  repeated string resources = 5;    // what it reaches: "Cloudflare account Acme", "AWS 123456789012 · ReadOnly"
  ConnectionStatus status = 6;
  string problem = 7;               // NEEDS_RECONNECT: why, a sentence
  int64 expires_ms = 8;             // when it needs connecting again; 0 unknown
  int64 checked_ms = 9;             // last refreshed or checked
  string proposal_id = 10;
}

enum ConnectionStatus {
  CONNECTION_STATUS_UNSPECIFIED = 0;
  CONNECTION_STATUS_READY = 1;
  CONNECTION_STATUS_NEEDS_RECONNECT = 2;
}

message SecretExport {
  string connection_id = 1;         // the connection it's made from
  repeated string access = 2;       // what it can do, sentences
  repeated string permissions = 3;  // exactly
  repeated string resources = 4;
  int64 expires_ms = 5;             // 0: doesn't expire
  bool per_use = 6;                 // a fresh short-lived credential for each command (AWS)
}

message ListConnectChoices { string secret_id = 1; }   // a connection

message ConnectChoices {
  string secret_id = 1;
  repeated ConnectChoice choices = 2;
  string rules = 3;                 // how to pick, for the person: "Pick one account, …"
}

message ConnectChoice {
  string kind = 1;                  // "account", "zone", "role", "permission"
  string id = 2;                    // what DeriveSecret takes
  string label = 3;
  string parent = 4;                // e.g. a zone's or role's account id
  string why = 5;                   // permissions: what it allows, a sentence
}

message DeriveSecret {
  string secret_id = 1;             // the connection
  string name = 2;                  // the new secret's
  string description = 3;
  repeated string permissions = 4;  // ConnectChoice ids
  repeated string resources = 5;    // ConnectChoice ids
  int64 expires_ms = 6;             // 0: the connector's default
}

message DisconnectService {
  string secret_id = 1;             // a connection
  // Also revoke it at the service. Its exports are deleted from the vault,
  // and their minted tokens at the service where it can, either way.
  bool revoke = 2;
}
```

Errors are the usual `Error` with a sentence to show as is: `INVALID_REQUEST`
(a wrong redirect, an unknown or ended flow, a callback from another device,
a bad field), `VAULT_LOCKED`, `UNREACHABLE` (the service didn't answer),
`NOT_FOUND`. A flow that fails after FinishConnect's Ack says why in
`ConnectFlow.error`.

## The flow

```
app (device D)                      backend                              service
StartConnect(connector, access,  →  check redirect against the connector's
  fields, proposal, redirect)       shapes; flow {id, D, state (256 bit),
                                    PKCE verifier, redirect, scopes,
                                    expires 10 min}
                                 ←  ConnectStarted {flow_id, url}
opens url (browser / ASWeb…)     ───────────────────────────────────────→ sign in, approve
receiver gets the callback URL   ←─────────────────────────────────────── redirect ?code&state
FinishConnect(flow_id, url)      →  same device D? state matches? not
                                    used? → Ack; EXCHANGING
                                    code + verifier ─────────────────────→ token
                                    CHECKING: account, scopes, resources ─→
                                    seal in the vault (connection secret),
                                    proposal → CONNECTED, CONNECTED
                                    events: proposer, Rivet
```

- **Device binding.** A flow belongs to the device that started it: its
  paired device id (the same one Welcome.device_id gives), never its name or
  anything in the request. FinishConnect from any other device is refused.
  The loopback port without a token (before this computer's app pairs) binds
  to that one connection. Unpairing a device cancels its flows. A restart of
  the backend ends every flow (the app shows Expired; retry).
- **Callbacks.** The app makes its receiver first (Mac: a Dart `HttpServer`
  on 127.0.0.1, port 0, path `/oauth/<connector>`, closed after one request
  or a cancel; iPhone: `ASWebAuthenticationSession` with the
  `com.claktory.app` scheme, `.dev` for Claktory Dev), then sends
  StartConnect with that exact URI. The backend accepts only the
  connector's shapes. The app sends FinishConnect with the whole URL it got,
  query as is, including `error=` ones; it never reads or keeps the code
  itself, nor logs the URL. The Mac's loopback page should say "You can close
  this tab and go back to Claktory."
- **Device flow (AWS).** ConnectStarted's `url` is the verification page
  with the code filled in, and `user_code` the code to compare. The app opens
  the URL and waits; the backend polls the service (obeying its interval and
  slow-down) and the flow moves on by itself. No FinishConnect.
- **Who sees what.** Only the asker gets ConnectStarted (the URL, the code).
  VaultState.flows shows every device the flow's state, never a URL or code.
  An ended flow stays in the list for 10 minutes, or until CancelConnect.
- **Reconnect.** StartConnect with `secret_id` of a connection: the new
  tokens replace the old in place; its exports and grants stay. Refused if
  the account that comes back is a different one.
- **Nothing is granted** when a flow ends: not to the proposer, not to
  anyone.
- **Names.** The backend names a connection after the service and account,
  "Cloudflare (matt@example.com)" (" 2" if taken; secret names may hold
  `@ ( )` now); UpdateSecret renames it
  and changes its description (entries are refused for a connection).
- **Picking an export's makings.** ConnectChoices.rules says it in words,
  and DeriveSecret's refusal says what's wrong. Cloudflare: exactly one
  account, at least one permission, zones of that account (none: the whole
  account). AWS: exactly one role (its parent is its account), no
  permissions (the role's permission set is the access).

## The vault

All in `vault.db`, never factory.db or config.json; a migration (older
backends after a rollback ignore the new columns and table; a connection
looks like a secret with no variables to them, and exports like ordinary
secrets).

- `secrets` gains `kind` (`''` ordinary, `connection`, `export`),
  `connector`, `parent` (an export's connection) and `meta` (public JSON:
  account, scopes, resources, status, expiry, the proposal; what SecretConnection
  and SecretExport show).
- `controls (secret, version, sealed)`: a connection's broker-only payload,
  the recipe's JSON (tokens, refresh token, client registration), sealed
  with the secret's own key and bound (AAD) to the vault, secret, connector
  and version. `values_of`, `reveal`, `resolve` never read it; a connection
  has no variables and can't be granted (refused, with a sentence pointing to
  exports).
- `connect_proposals`: the proposals, like `requests`.
- Exports are ordinary secrets with variables plus `parent` and `meta`.
  When an export is due (`renew_at_ms`, which the recipe sets: Cloudflare 7
  days before its expiry, default 30 days; AWS 2 minutes before its
  credentials expire) the broker renews it **before** the command gets it;
  `per_use` only tells the app it's short-lived and renewed for the person.
  `ExportMeta.public` names variables that aren't secret (AWS_REGION): they're
  set like the rest but never hidden in what agents read.
  A renewal that fails keeps the old values while they still work.
  The recipe returns new values (a new version) or the same values with a
  new expiry (Cloudflare extends the same token, so a running command keeps
  working; no new version): in the async wrapper around `vault.resolve`, with one renewal at a
  time per connection, the new values saved (a new version) before use.
  Refresh-token rotation is saved before the new access token is used; a
  Problem::Reconnect from any recipe call (refresh, choices, derive, renew)
  marks the connection NEEDS_RECONNECT, and commands asking for its exports
  are told why. When a renewal brings new values and the recipe's meta
  changed (a new minted token), the old one is removed at the service.
- Redaction keeps hiding every value it knows (every_variable), now also
  exports' renewed values. A connection's own tokens are hidden too.

## Recipes (Rust, Sam's), behind the core's trait (mine)

`crates/factoryd/src/connectors/`: `mod.rs` (registry, the broker, flows,
PKCE, state, binding, renewals, the vault calls), `proposals.rs` (the
tool, the card, events), `cloudflare.rs` and `aws.rs` (Sam's). A sketch,
final shape in my first commit, which Sam builds on:

```rust
pub(crate) trait Recipe: Send + Sync {
    fn spec(&self) -> &Spec;                       // catalog entry: id, name, accesses, fields, flow, redirects, variables
    // BROWSER: the authorize URL for client, redirect, state, PKCE challenge, scopes, fields.
    fn authorize_url(&self, start: &Start<'_>) -> Result<Url, Problem>;
    // BROWSER: code + verifier → the connection's control payload.
    fn exchange<'a>(&'a self, http: &'a Http, start: &'a Start<'_>, code: &'a str, verifier: &'a str) -> BoxFuture<'a, Result<Control, Problem>>;
    // DEVICE: start, then poll (Pending | SlowDown | Done(Control)).
    fn start_device<'a>(&'a self, http: &'a Http, start: &'a Start<'_>) -> BoxFuture<'a, Result<Device, Problem>>;
    fn poll_device<'a>(&'a self, http: &'a Http, device: &'a Device) -> BoxFuture<'a, Result<Poll, Problem>>;
    // Who and what: account, granted scopes, resources, expiry (SecretConnection's fields).
    fn inspect<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Inspected, Problem>>;
    // Fresh tokens; None when they're still good. Problem::Reconnect when the grant is gone.
    fn refresh<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Option<Control>, Problem>>;
    fn choices<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Vec<Choice>, Problem>>;
    // An export: variables + its meta + expiry (+ provider ids to revoke later), and renewing one.
    fn derive<'a>(&'a self, http: &'a Http, control: &'a Control, ask: &'a Derive) -> BoxFuture<'a, Result<Derived, Problem>>;
    fn renew<'a>(&'a self, http: &'a Http, control: &'a Control, export: &'a ExportMeta) -> BoxFuture<'a, Result<Derived, Problem>>;
    fn revoke<'a>(&'a self, http: &'a Http, control: &'a Control, exports: &'a [ExportMeta]) -> BoxFuture<'a, Result<(), Problem>>;
}
```

`Control` is the recipe's own JSON in a `Zeroizing<String>`. `Http` is one
reqwest client that never follows redirects, with each recipe's base URLs
overridable for tests only (`Factory::set_connector_base(id, url)`, hidden).
The core does state, PKCE (S256, 64-character verifier), expiry, device
binding, single-use codes, the device-flow polling loop, storage, renewal
timing and the events; recipes do only what's particular to the service.

### What each recipe stores (Sam to confirm)

| | Connection (broker-only control) | Connection meta (shown) | Export variables | Export meta |
|---|---|---|---|---|
| Cloudflare | access token, refresh token, expiry, granted scopes | user's email if userinfo answers (else the account names), accounts (id, name), scopes | `CLOUDFLARE_API_TOKEN` (an account-owned token minted for it, 30 days, extended 7 days before), `CLOUDFLARE_ACCOUNT_ID` | account, permission groups and zones (ids and names, the policy as given, to rebuild it), expiry, the minted token's id |
| AWS | region, portal (start URL), the OIDC registration (client id, secret, its expiry, about 90 days), access token, refresh token, expiry | portal, region, accounts and their roles | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION`, `AWS_DEFAULT_REGION` (fresh, cached until 2 min before they expire) | account id and name, role name, per use |

## Proposals and events (mine)

- Tool `secrets_propose_connection` (only with the `connect` option on):
  `{service, purpose, access, reason, access_id?}`. `service` is a
  connector id or name; anything else is refused with the list. One pending
  proposal per agent and service. The result's details carry the proposal;
  the transcript turns it into Entry.connect_proposal, updated when decided.
- Connected: the proposer is told "Matt connected Cloudflare (account) for
  your proposal. You don't have it: Rivet decides what you get, and you'll be
  told." Rivet (every admin agent with Secrets on) gets an event with the
  proposal (purpose, access, reason) and what the connection reaches, asking
  it to compare them and give the least that does the job.
- Dismissed: the proposer is told, with the comment.
- Connections the person makes on their own: Rivet isn't told (nobody asked
  for anything).

## Rivet's tools (Sam's)

In the Secrets extension, admin agents only: `secrets_inspect` (a
connection or export: connector, account, scopes, resources, status, expiry,
the proposal it answered, its exports and who has them; never values),
`secrets_choices` (what an export can be made of: accounts, zones, roles,
permissions), `secrets_derive` (make an export; `secrets_grant` then gives
it). `secrets_grant` refuses a connection. `secrets_list` shows kinds.

## What Matt must set up

- **Cloudflare**: one public OAuth client in a Cloudflare account Claktory
  owns (Manage Account → OAuth clients): token auth method `none` (public,
  PKCE), grant `authorization_code` (+ `refresh_token`), redirect URLs
  `http://127.0.0.1/oauth/cloudflare` (loopback; any port, to be tested),
  `com.claktory.app:/oauth/cloudflare`, `com.claktory.app.dev:/oauth/cloudflare`.
  Scopes as Sam listed (API Tokens Write to mint narrow tokens; account and
  zone read; the permissions minted tokens may carry). Private is enough for
  Matt's own accounts; public later needs claktory.com's TXT verification.
  We need back only the client id (public).
- **AWS**: nothing to register. His AWS needs IAM Identity Center on, his
  user in it, and permission sets assigned (narrow ones for agents); he
  enters the access portal URL and region when connecting.
- **Testing**: once the Cloudflare client exists, one real connect from
  Claktory Dev (Mac loopback and iPhone scheme) to check the loopback port
  and refresh behaviour.

## Not in this slice

Resend, GitHub, Hetzner's guided import, Apps, MCP, the claimed HTTPS
callback (auth.claktory.com), a flow surviving a backend restart.

## The manual, with the first service

Until a service is offered (Cloudflare waits for its client id, AWS for a
try against a real account: `SHIPPED` in aws.rs), nothing about connecting
shows (the option, the tool, its prompt section, Rivet's tools, the app's
Connect a service…), so the manual says nothing of it. The dev Claktory
has AWS with `FACTORY_AWS_ENABLED=1` (AWS itself) or `FACTORY_AWS_BASE`
(a stand-in), and Cloudflare with `FACTORY_CLOUDFLARE_CLIENT_ID` and
`FACTORY_CLOUDFLARE_BASE`. When one ships, add
back, under `## Secrets` in the core (naming only the services offered):

```
- {{user}} can also **connect a service** (Cloudflare, AWS) by approving Claktory on its own site; the vault keeps the connection, which no agent ever gets. An agent is given a secret made from it with only the access its work needs, and uses it as any other.
```

as the last part of the `ext-secrets` note's item (the AWS sentence only
with AWS, and the examples only of the services offered):

```
  - **Connecting services.** With the option **Can suggest connecting services** (`connect`, off unless turned on), an agent also gets `secrets_propose_connection`: `service` (a connector's id or name, such as `cloudflare` or `aws`), `purpose`, `access` (which permissions, on which zones, accounts or roles), `reason`, and optionally `access_id`. One suggestion per agent and service waits at a time; it shows in its conversation and at the top of Settings → Secrets, where {{user}} connects it or dismisses it. Connecting opens the service's page on {{user}}'s device (any paired device works, over the tunnel too); the device hands where the browser came back to to the backend, which signs in itself and keeps the tokens in the vault, never as variables: a connection is given to no one, and `secrets_grant` refuses it. When one is made for a suggestion, the agent is told it has nothing yet, and every admin agent with Secrets is told what was asked and what the connection reaches, to make a secret from it with only the access the work needs and give that. Secrets made from a connection (a Cloudflare token for some zones, an AWS account and role) are renewed before a command gets them when they're due; one the service no longer accepts shows **Needs reconnecting**. **AWS** connects through IAM Identity Center: {{user}} gives the access portal URL and its region, and approves the code on AWS's page. A secret made from it is one role in one account (its permission set is all its access), as `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_REGION` and `AWS_DEFAULT_REGION`: short-lived credentials, fetched again 2 minutes before they expire (the region isn't hidden in output). Disconnecting removes the connection and every secret made from it (their tokens at the service too, where it can), and can also revoke it at the service.
```

in the `app-secrets` note: its summary gains ", connecting services" after
".env files"; its item on agents' requests gains, at its end,

```
Agents' suggestions to connect a service show there too, as in their conversations: what it's for, what it needs and why, then **Connect <service>** (with the access to allow and anything the service needs first) or **Dismiss**, with an optional reason. Connecting gives the agent nothing: Rivet, or {{user}}, gives it a secret made from the connection.
```

then, right after that item, **Connect a service…**, and after the item on what each secret shows, what a connection shows:

```
- **Connect a service…**: choose a service (such as AWS or Cloudflare), the access to allow and what it needs first (AWS: the access portal URL and region), then **Connect**. Its page opens on the device {{user}} is at, the backend's machine or another one: in the browser on a Mac, which comes back to the app by itself; in a sign-in sheet on iPhone; for AWS, a page with a code to check. The app shows how it's going, or why it failed, with **Try again**. Claktory keeps what it gets in the vault as a connection; nobody has it.
```

```
- A connection shows its service and account, what it can do and reach, and has no values: it's never given to an agent. **Make a secret from it…** makes a narrower secret to give like any other (on Cloudflare an account, any of its zones and what it may do; on AWS a role), which Claktory renews before it expires (on AWS, a new key 2 minutes before the last one expires); it says which connection it was made from. **Connect again** signs in again in its place, keeping the secrets made from it and who has them; **Edit…** renames it; **Disconnect…** removes it and the secrets made from it, and cancels it at the service too unless turned off.
```

With AWS, `docs/CHANGELOG.md` gains under `## Unreleased` (Hana's words):

```
- Added: Connect AWS in Settings → Secrets, then give an agent one role in one account and nothing more. You sign in through IAM Identity Center with your access portal URL and region. The agent's short-lived keys renew by themselves, and your sign-in never goes to any agent.
```

and, in the `ext-secrets` note, after `secrets_revoke`'s sentence (Sam's tools, shown to
admin agents only once a service is offered):

```
Once Claktory connects a service, admin agents also get `secrets_inspect` (`secret`: what a secret is and reaches, never a value; for a connection its account, status, access, scopes, resources, the suggestion it was connected for and the secrets made from it with who has each; for a secret made from one its connection, access, permissions, resources, expiry and who has it), `secrets_choices` (`secret`, a connection: its accounts, zones, roles and permissions with the ids to pick, and how to pick) and `secrets_derive` (`secret`, `name`, `description`, `permissions` and `resources` from the choices, `days`, or the service's default: Cloudflare's tokens last 30 days and are extended while they're used): a secret with only that access, given to no one until `secrets_grant` gives it. `secrets_list` says which secrets are connections and what each export was made from.
```

Rivet's default prompt (crates/factoryd/src/rivet-prompt.md) then gains, after
its secrets item, with the new SHA-256 in `SHIPPED_RIVET_PROMPTS`:

```
- A connected service (such as Cloudflare or AWS) is never given to an agent. When {{user}} connects one for an agent's suggestion, compare what the agent asked for with what the connection reaches (secrets_inspect), make a secret from it with only the access that work needs (secrets_choices, then secrets_derive), give that to the agent with secrets_grant, and tell it what it got.
```
