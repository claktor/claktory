# Deploying claktory-account

How to put Claktory's account service on a server of your own, at
`account.claktory.com`. You rent the server, point DNS at it, and paste a
Resend key; `scripts/deploy-account.sh` does the rest from this Mac:

- builds the service for the server;
- installs it under systemd, with Caddy in front for HTTPS (Let's Encrypt);
- makes the service's keys once and copies them to you to keep offline;
- backs up the database daily, and fetches DB-IP's free GeoIP file monthly.

Running it again updates the service; if the new version doesn't start,
the one before it is put back.

## 1. A server

- Any small Linux server with systemd: Debian 12, Ubuntu 24.04, Fedora…;
  1–2 vCPU and 1–2 GB is plenty (Hetzner CX22, a DigitalOcean droplet…).
  x86_64 or ARM.
- SSH access with your key, as root or as a user with sudo (no password
  prompt: `NOPASSWD`).
- Its firewall open on 22 (SSH), 80 and 443 only.
- Add it to `~/.ssh/config` so it has a short name:
  ```
  Host claktory-account
    HostName 203.0.113.7
    User root
  ```
- This Mac needs podman (`brew install podman; podman machine init --rosetta;
  podman machine start`) or Docker, to build the service for Linux.

## 2. DNS for claktory.com

At claktory.com's DNS host:

| Type | Name | Value | For |
|---|---|---|---|
| A (and AAAA if it has IPv6) | `account` | the server's IP | the service |
| TXT | `resend._domainkey.auth` | from Resend (step 3) | DKIM: Resend signs the emails |
| MX | `send.auth` | from Resend, e.g. `feedback-smtp.us-east-1.amazonses.com`, priority 10 | bounces (Return-Path) |
| TXT | `send.auth` | from Resend, e.g. `v=spf1 include:amazonses.com ~all` | SPF |
| TXT | `_dmarc` | `v=DMARC1; p=none; rua=mailto:dmarc@claktory.com` | DMARC; later `p=quarantine`, then `p=reject` after two weeks of clean reports |

- Copy Resend's records exactly as its dashboard shows them; the names
  above are relative to claktory.com (`resend._domainkey.auth.claktory.com`).
  With Cloudflare, turn the proxy (orange cloud) off for all of them, and for
  `account`.
- If claktory.com itself sends no email, also add `v=spf1 -all` as a TXT on
  the root, unless your own mailbox uses claktory.com (its host's SPF goes
  there instead).
- **claktory.com/account** is the address the emails give. Wherever
  claktory.com is hosted, redirect `/account` to
  `https://account.claktory.com/account`. If claktory.com has no site yet,
  point it at the server too (A record for `@`) and deploy with
  `--redirect-from claktory.com`: all of claktory.com then redirects there.

## 3. Resend

1. Make an account at resend.com.
2. **Domains → Add Domain**: `auth.claktory.com`, the region nearest your
   users. Put the records it shows in DNS (step 2) and wait for **Verified**.
3. **API Keys → Create API Key**, with **Sending access** to
   `auth.claktory.com` only. Copy it (it starts with `re_`); you paste it in
   step 4.
4. The free plan sends 100 emails a day and 3,000 a month; the service stops
   at 90 a day (`--email-daily-cap`) and tells you at 80%. Move to Pro
   ($20 a month, 50,000) before inviting more than a few dozen people, then
   deploy again with a higher `--email-daily-cap`.

## 4. Deploy

From this checkout:

```sh
scripts/deploy-account.sh --admin-email you@example.com claktory-account
```

- `--admin-email` is who runs it: the 80% email alert, and **Usage counts**
  at claktory.com/account once signed in with that address. Repeat it for
  more people.
- Add `--redirect-from claktory.com` if claktory.com points at the server
  (step 2).
- The first run makes the service's keys and copies them to
  `~/Claktory account/claktory-account/secrets.json`. **Keep them offline**:
  in your password manager, and on paper or a USB stick somewhere else. Then
  delete that file. Without `k_share` and `signing_key`, no Claktory can
  unlock its secrets after a restart except with its recovery key; the
  database can be lost (Claktories sign in again), the keys can't.

Then the Resend key:

```sh
scripts/deploy-account.sh claktory-account resend-key
```

Until it's there, emails aren't sent: they're written to files on the
server, in `/var/lib/claktory-account/mail`.

Check it: <https://account.claktory.com/.well-known/claktory-account>
answers with the service key, and <https://claktory.com/account> lets you
sign in with a code from your email.

## Afterwards

| | |
|---|---|
| Update to this checkout's service | `scripts/deploy-account.sh --admin-email you@example.com claktory-account` (the same options as before) |
| Is it running? | `scripts/deploy-account.sh claktory-account status` |
| A copy of the database here | `scripts/deploy-account.sh claktory-account backup` |
| The keys again | `scripts/deploy-account.sh claktory-account secrets` |
| Usage counts | claktory.com/account, signed in with an `--admin-email` address; or on the server, `sudo -u claktory-account claktory-account stats --db /var/lib/claktory-account/account.db` |

- **Backups:** every day the server keeps a copy of the database in
  `/var/lib/claktory-account/backups` (14 days). `backup` above copies the
  newest one to this Mac; do it now and then, or before changing servers.
- **A new server:** deploy to it, then copy the old one's
  `/var/lib/claktory-account/secrets.json` and newest backup (as
  `account.db`) into `/var/lib/claktory-account` on the new one, owned by
  `claktory-account`, and restart it (`systemctl restart claktory-account`).
  Then point DNS at it.
- **GeoIP:** DB-IP's free "IP to City Lite" file (CC BY 4.0), fetched again
  on the 3rd of each month. Pages that show places say "IP Geolocation by
  DB-IP", as its license asks. `--no-geoip` leaves it out.

## Where things are on the server

| | |
|---|---|
| `/usr/local/bin/claktory-account` | the service (`.previous`: the one before) |
| `/var/lib/claktory-account/` | `account.db`, `secrets.json`, `mail/`, `backups/`, `geoip/`: only the `claktory-account` user can read it |
| `/etc/claktory-account/options` | its options (written by each deploy) |
| `/etc/systemd/system/claktory-account*.{service,timer}` | the service, the daily backup, the monthly GeoIP file |
| `/etc/caddy/claktory-account.caddy` | the site in Caddy (imported by `/etc/caddy/Caddyfile`) |
| `journalctl -u claktory-account` | its log |

## Trying it without a server

`scripts/test-deploy-account.sh` deploys to a Debian container in podman
(with Caddy's own certificate authority, since there's no DNS) and goes
through all of the above: a Claktory signing in, unlocking and checking in
over HTTPS, an update, a rollback, a restart and a backup.
