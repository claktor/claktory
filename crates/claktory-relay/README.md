# claktory-relay

Claktory's relay, relay.claktory.com (#450; the design is Fugo's
coordination/security/relay-access.md). It's iroh-relay's server (Claktory's
copy, `vendor/iroh-relay`) with one rule: **every client gets in, and a packet
is forwarded only when its sender or its recipient is a Claktory signed in to
a claktory.com account.** Such a Claktory shows a relay token from the account
service (`POST /v1/relay/token`, `crates/claktory-account/API.md`) when it
connects. Its devices, phones pairing with it and pipe peers that aren't signed
in need nothing. Two endpoints that aren't Claktory users can't use the relay
with each other. The relay stays blind: it carries only end-to-end-encrypted
traffic, and a token gives no access to anyone's Claktory.

## Running it

```sh
claktory-relay --config-path /etc/claktory-relay/claktory-relay.toml
```

The config is iroh-relay's (`iroh-relay.toml`, its TLS, QUIC address
discovery, metrics and limits) plus a `[claktory]` table:

```toml
[claktory]
mode = "observe"            # or "enforce"
keys = "/etc/claktory-relay/relay-token-keys.json"
denylist = "/etc/claktory-relay/relay-token-denylist.json"
```

iroh-relay's own `access` setting is refused: the access is always this one.

- **observe** forwards everything, and counts what enforce would drop.
- **enforce** drops it, and counts it. It won't start, or be switched to by a
  reload, without a valid key file: no keys would cut every Claktory off.
  Observe starts without the files, saying so.

### The files

- Key file, at most 4 KB: `{"seq": N, "keys": ["<64 hex>", …]}`, 1 or 2
  Ed25519 public keys (the current one and the previous one, while it
  rotates), as `claktory-account relay-token-pubkey` prints them.
- Denylist: `{"seq": N, "a": ["<32 hex>", …]}`, at most 100,000 accounts (a
  token's opaque `a`). An empty list is valid.

Both are parsed strictly (unknown or duplicate fields refused). A file that
doesn't parse, or whose `seq` is lower than the loaded one's, is refused and
the last good one stays (logged, and counted in `reload_failures`). The
relay reads them again within 5 seconds of a change (it watches the paths,
so a file replaced with a rename is seen), and on SIGHUP, which also reads
`mode` from the config file again; the rest of the config takes a restart.
Neither drops a connection. A denylist takes authorization away at once from
the live connections of the accounts it names.

### Tokens

`Authorization: Bearer base64url(P) "." base64url(S)`, both unpadded:

- `P` is the exact JSON bytes `{"v":1,"e":"<endpoint id, 64 hex>","a":"<32 hex>","iat":…,"exp":…}`.
- `S` is Ed25519 over `"claktory-relay-token-v1" 0x00` followed by `P`.

A token authorizes the endpoint it's bound to (`e`), while its connection
lasts, when it's at most 512 bytes, signed by a key in the key file, not
expired, issued at most 60 s ahead of the relay's clock, living at most 8
days, and its account isn't denied. Tokens are never logged; rejections log
the endpoint's short id and the reason.

### Metrics

Served with iroh-relay's own on `metrics_bind_addr`:

| Metric | |
|---|---|
| `claktory_relay_clients_authorized` | endpoints connected with a good token |
| `claktory_relay_clients_unauthorized` | endpoints connected without one |
| `claktory_relay_forward_denied_packets_total`, `…_bytes_total` | packets between two endpoints without one: dropped in enforce, forwarded in observe |
| `claktory_relay_tokens_accepted_total` | |
| `claktory_relay_tokens_rejected_{bad,expired,endpoint,denied}_total` | by reason |
| `claktory_relay_enforcing` | 1 or 0 |
| `claktory_relay_keys_loaded`, `claktory_relay_denylist_entries` | |
| `claktory_relay_reload_failures_total` | |

## Building

From a commit's export, static for the relay box (aarch64 musl), as
`claktory-ops/relay` builds it: `cargo build --release -p claktory-relay
--target aarch64-unknown-linux-musl`.

`main.rs` is iroh-relay 1.5.0's own `src/main.rs`, changed only where marked
`Claktory:`; `access.rs` is the rule.
