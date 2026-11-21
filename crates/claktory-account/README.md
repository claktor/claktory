# claktory-account

Claktory's account service: email sign-in, the Claktories of each account
and the devices signed in to it (to find them), the vault's service share, daily check-ins, the relay allowlist and the
account page (claktory.com/account). One binary with one SQLite file and one
secrets file; anyone can run their own and point their Claktory's
`account.service` at it.

- [API.md](API.md) is the HTTP API, the contract with factoryd.
- The library's `protocol` module (with `default-features = false`) is what a
  client needs: the API's types, request signing, answer checking and opening
  a sealed share. The `server` feature (on by default) is the service.

## Running it

```sh
cargo build --release -p claktory-account
claktory-account init-secrets secrets.json     # new keys, chmod 600; never over a file
claktory-account --db account.db --secrets secrets.json --mail-dir mail/
```

It listens on 127.0.0.1:8080 (`--listen`). With `--mail-dir` it writes each
email to a file there instead of sending it, so a local instance needs no mail
provider: the sign-in code is in the newest file. To send for real through
Cloudflare Email Sending, put a Cloudflare API token with Email Sending: Edit
in the secrets file (`claktory-account set-cloudflare-token secrets.json`,
the token on standard input) and give `--cloudflare-account ID`; Resend still
works too (`set-resend-key`, `--email resend`).

Behind a TLS proxy such as Caddy, add `--trust-proxy`: the client's IP is then
the last entry of `X-Forwarded-For`, and the account page's cookie is Secure.
`claktory-account help` lists every option; `claktory-account stats --db
account.db` prints the usage counts (accounts and Claktories in touch per
day, week and month; tokens all time, per day and per model; agent hours by
hour of the day; countries, versions, platforms; sign-ins per day), and the
same report is at `/account/admin` for the `--admin-email` addresses.
`--geoip FILE` takes a GeoIP city file (`.mmdb`: DB-IP's free "IP to City
Lite", <https://db-ip.com/db/download/ip-to-city-lite>, CC BY 4.0, or
GeoLite2 City) to know roughly where Claktories are.

**Back up `k_share` and `signing_key` from the secrets file offline.** Every
Claktory's share derives from `k_share`, and Claktories pin the signing key:
without them, no Claktory signed in here can unlock its secrets after a
restart except with its recovery key. The database can be lost (Claktories
then sign in again); the secrets can't.

If a key leaks, `claktory-account rotate-secrets secrets.json --signing-key`
(or `--k-share`, or both) replaces only the keys named, keeping the file as
it was beside it (`secrets.json.before-<time>`), then restart the service. A
new signing key makes every Claktory sign in again, and its secrets open by
themselves after; a new `k_share` makes every Claktory open its secrets with
its recovery key once, and they open by themselves again from then on.

## What it keeps

Accounts (email, when made, last sign-in); Claktories (id, name, backend
public key, token hash, epoch, version, platform, last contact, when
removed, their counts of agents, projects and devices, and their place:
country, region and city; their own name and color and their relays, as
they last sent them); the devices signed in (account public key, name,
platform, version, when signed in, last contact) and the tunnel keys they
got assertions for, for a day; the days each was in touch; their usage per day,
provider and model, and their agents' activity per hour; the ids of the
periods they reported (120 days, to know one sent again); their relay
endpoint ids; share releases with their IP, for 30 days; pending sign-in codes (hashed, 15 minutes); rate-limit counters (2
days); web sessions (1 hour); sign-ins per day. No shares, tokens or codes in
the clear, and none of them in its logs. Deleting an account deletes all of
it, except that its Claktories' usage and activity join anonymous totals.
