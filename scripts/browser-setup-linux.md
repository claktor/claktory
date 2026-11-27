# Claktory's browser on Linux: what setup installs (contract C4, #435)

`scripts/browser-setup-linux.sh` is the one administrator step that lets a Linux
Claktory run its own browser (agent-browser.md Rev 2, D6). It runs as root, once,
and again whenever Claktory's setup check asks; running it again changes nothing
that's already right. Claktory's setup check (`browser_host/setup.rs`) checks each
of these and shows the person this script's exact command, or the NixOS lines below,
when one is missing. Nothing here is used by anything but the browser.

| What | Exact path or name | Owner, mode | Why |
|---|---|---|---|
| The browser's user | `claktory-browser` (system user, no login shell, home `/var/lib/claktory-browser`) | | Chrome runs as it, never as the person |
| Its home | `/var/lib/claktory-browser` | claktory-browser, 0700 | |
| Profiles | `/var/lib/claktory-browser/profiles` | claktory-browser, 0700 | the kept profile (Matt's decision, 11-24) |
| The runner's folder | `/opt/claktory-browser` | the person, 0755 | factoryd copies itself here before each start |
| The runner | `/opt/claktory-browser/runner` | the person, 0755 | `factoryd browser-runner`, run through sudo |
| Sudo, exactly the runner | `/etc/sudoers.d/claktory-browser`, one line: `<person> ALL=(claktory-browser) NOPASSWD: /opt/claktory-browser/runner browser-runner *` | root, 0440, checked with `visudo -cf` | |
| Chrome's policy | `/etc/opt/chrome/policies/managed` (and `/etc/chromium/policies/managed` where Chromium is there): empty mount points, real folders (never links); Claktory's own `claktory.json` from earlier setups is taken out, and any other file there is left alone (an employer's policy for your own Chrome, say). The policy itself is never written there: the runner mounts a private tmpfs over the folder inside its own namespace and writes `claktory.json` into it, `{"LocalNetworkAccessRestrictionsEnabled": true, "LocalNetworkAccessBlockedForUrls": ["*"], "BrowserSignin": 0, "SyncDisabled": true, "CommandLineFlagSecurityWarningsEnabled": false, "AutofillCreditCardEnabled": false, "AutofillAddressEnabled": false}`, so only the errands' Chrome sees it and your own Chrome on this machine doesn't (Ghiaccio 49028, Fugo 49032) | root, 0755 | pages can't reach the local network (Fugo); Chrome never signs in or syncs itself, so a sign-in on a site stays that site's cookies (Fugo 41236); no warning bar for the runner's fixed flags, which hide `navigator.webdriver` while an agent drives (R11, Fugo 41700); no autofill of cards or addresses |
| The firewall | nft table `inet claktory_browser`, from `/etc/claktory-browser.nft` (a copy of `scripts/claktory-browser.nft`), loaded by `/usr/local/sbin/claktory-browser-nft` (a copy of `scripts/claktory-browser-nft.sh`, which the Mac image uses too), which replaces only that table in one transaction and then adds the resolvers in `/etc/resolv.conf` to its sets `dns4` and `dns6` (port 53 only), run by `claktory-browser-nft.service` (`/etc/systemd/system`, enabled, wanted by sysinit.target, before network-pre.target, restarted with nftables.service). `/etc/nftables.conf` and nftables.service are left alone (Fugo) | root; 0644, the loader 0755 | the browser's user reaches neither this machine nor the local network, by any protocol |
| User namespaces for the runner (Ubuntu 23.10 and later: only where `kernel.apparmor_restrict_unprivileged_userns` exists) | `/etc/apparmor.d/claktory-browser-runner`: profile `claktory-browser-runner` for `/opt/claktory-browser/runner`, `flags=(unconfined)`, `userns,` only; loaded with `apparmor_parser -r`, checked with `aa-status` | root, 0644 | the runner's private /proc and PID 1 (Ghiaccio). It lets the runner, and anything the person puts at its path, make user namespaces (Fugo accepted) |
| X's socket folder | `/tmp/.X11-unix`, made at every boot by `/etc/tmpfiles.d/claktory-browser.conf` (`D! /tmp/.X11-unix 1777 root root -`, as systemd's own x11.conf); each runner mounts its own tmpfs over it and refuses to start unless it's a real folder of root's | root, 1777 | two browsers can't take each other's display (Ghiaccio, Fugo 38527) |
| Packages | `google-chrome-stable` (Google's apt repository: brought in once with the key in `/etc/apt/keyrings/google-chrome.gpg`, then Chrome's own `google-chrome.sources` keeps it updated), `xvfb`, `xauth`, `tzdata` (the person's time zone, #435 change 3), fonts: a normal desktop set (Liberation and Liberation 2, DejaVu, Noto, Noto Color Emoji, Noto CJK; change 3) | | |

The firewall's browser chain, matched by the user positively (`meta skuid
"claktory-browser"`), in this order: accept established and related; accept DNS
(tcp and udp 53) to 127.0.0.53 and to the resolvers in `/etc/resolv.conf` as they
were when the table was loaded; reject (a TCP reset for TCP) anything to this
machine's own addresses (`fib daddr type local`), loopback, 0.0.0.0/8, 10/8,
100.64/10 (CGNAT, Tailscale), 127/8, 169.254/16, 172.16/12, 192.0.0/24,
192.168/16, 198.18/15, multicast and 240/4 (broadcast included), and for IPv6 ::,
::1, ::ffff:0:0/96, 64:ff9b::/96 and 64:ff9b:1::/48 (NAT64), 2002::/16 (6to4),
fc00::/7, fe80::/10, ff00::/8. Everything else is accepted (Landlock limits
Chrome's TCP to ports 80 and 443).

The script refuses to go on, rather than follow a symbolic link, when any path it
writes or changes as root is one (`/opt/claktory-browser`, the runner, the
browser's home and profiles, the nft file, the loader, the AppArmor profile); the
runner must be a regular file. Chrome's apt source: the script adds
`/etc/apt/sources.list.d/google-chrome.list`, signed by Google's key pinned by its
fingerprint (EB4C 1BFD 4F04 2F6D DDCC EC91 7721 F63B D38B 4796, as
https://www.google.com/linuxrepositories/ lists it), only to bring Chrome in;
Chrome's package then replaces it with its own `google-chrome.sources`.

Supported: Ubuntu 24.04 and Debian 12 or later, x86_64 and aarch64 (Chrome for arm64
Linux from Google's repository where Google publishes it). Landlock ABI 4 or later
(Ubuntu 24.04's 6.8 kernel is 4). Others get the NixOS lines or a setup card that
says so.

    sudo bash browser-setup-linux.sh [--user NAME]   # NAME: who runs Claktory (default: the sudo caller)

The release carries the script; the setup card shows it with the release's path.

## NixOS

The same, declared in `configuration.nix` (then `nixos-rebuild switch`), with NAME
the user Claktory runs as. The manual and the setup card show these lines.

```nix
{ pkgs, lib, ... }:
let
  person = "NAME";  # who runs Claktory
  # Google's Chrome where nixpkgs has it for this machine, Chromium elsewhere.
  chrome = if lib.meta.availableOn pkgs.stdenv.hostPlatform pkgs.google-chrome then pkgs.google-chrome else pkgs.chromium;
in {
  nixpkgs.config.allowUnfree = true;
  environment.systemPackages = with pkgs; [ chrome xvfb xorg.xauth ];
  # Fonts go in fonts.packages, where fontconfig (and so Chrome) finds them.
  fonts.packages = with pkgs; [ liberation_ttf dejavu_fonts noto-fonts noto-fonts-color-emoji noto-fonts-cjk-sans ];
  users.groups.claktory-browser = {};
  users.users.claktory-browser = { isSystemUser = true; group = "claktory-browser"; home = "/var/lib/claktory-browser"; createHome = true; homeMode = "700"; };
  systemd.tmpfiles.rules = [
    "d /var/lib/claktory-browser/profiles 0700 claktory-browser claktory-browser -"
    "d /opt/claktory-browser 0755 ${person} users -"
    "D! /tmp/.X11-unix 1777 root root -"
  ];
  security.sudo.extraRules = [{
    users = [ person ]; runAs = "claktory-browser";
    commands = [{ command = "/opt/claktory-browser/runner browser-runner *"; options = [ "NOPASSWD" ]; }];
  }];
  # Chrome's policy is the runner's own (a tmpfs inside its namespace); these
  # only make the empty mount points, root's.
  environment.etc."opt/chrome/policies/managed/claktory-browser-mount.json".text = "{}";
  environment.etc."chromium/policies/managed/claktory-browser-mount.json".text = "{}";
  networking.nftables.enable = true;
  networking.nftables.tables.claktory_browser = { family = "inet"; content = builtins.readFile ./claktory-browser.nft; };
}
```

`claktory-browser.nft` there is `scripts/claktory-browser.nft`'s table body (the
release carries it beside this script; the setup card shows its contents). On NixOS
the sets `dns4` and `dns6` stay empty, so only 127.0.0.53 answers the browser's
user: the setup check reads /etc/resolv.conf and says so when it names another
resolver.

## The Mac image

On a Mac the browser runs in a Linux VM (Apple's `container`), from the image
each release builds and ships beside its backend as an OCI archive, never from
a registry (#435 W11): `claktory-browser.oci.tar.zst` in the release folder,
and on GitHub as the release's asset `claktory-browser-vX.Y.Z-arm64.oci.tar.zst`.
`scripts/release.sh` builds the release's linux-aarch64 runner, then the image,
then compiles into the Mac factoryd `CLAKTORY_BROWSER_REF` (the name
`container image load` prints, `localhost/claktory-browser:vX.Y.Z`),
`CLAKTORY_BROWSER_IMAGE` (`claktory-browser@sha256:` the image's manifest
digest, which `container image inspect` shows as its one linux/arm64
variant's), `CLAKTORY_BROWSER_ARCHIVE_SHA256` (the .zst's) and
`CLAKTORY_BROWSER_ARCHIVE_URL` (the asset's address). factoryd checks the
archive's SHA-256 before anything reads it, loads it, checks the variant's
digest before every run and runs it by that name, never by digest (`container`
asks a registry for any digest reference). `scripts/browser-image.sh` builds it:
a base (`browser-image/Containerfile.base`: Ubuntu 24.04 arm64, Chrome from
Google's repository with the key pinned as above, Xvfb, xauth, the fonts, the
same policy, `/etc/claktory-browser.nft` and the loader) and, per release, the
runner (that release's linux-aarch64 static factoryd) at
`/opt/claktory-browser/runner`, root's, 0755, with the entrypoint
`/usr/local/sbin/claktory-browser-entry` (`browser-image/entry.sh`). The
entrypoint, as root, loads the firewall or exits 70; makes the volume's mount
point `/var/lib/claktory-browser` claktory-browser's, 0700, and has that user
make `profiles/`; then execs the runner (`browser-runner run`, or its own
arguments) through `setpriv` as claktory-browser with no capabilities, an
empty bounding set and no_new_privs. Until that exec it writes only to stderr:
stdout is C1. It needs `--cap-add CAP_NET_ADMIN` for the firewall, and, under Apple's
`container` (1.7.1 or later, which setup asks for), `--masked-path NONE --read-only-path NONE`: `container`
masks and makes read-only paths under /proc, and the kernel then refuses the
runner its own procfs (EPERM; each flag alone isn't enough), so it fails closed
(Cioccolata 38429). Fugo's reach check from inside the VM (38475): as
claktory-browser, the VM's gateway, the Mac's LAN and Tailscale addresses, the
router, 7420/7421, 22 and 169.254.169.254 are all refused; 1.1.1.1:443 connects. Only the VM's root sees the unmasked paths, until the
entrypoint drops to claktory-browser with no capabilities; the VM is the
boundary. Podman's equivalent is `--security-opt unmask=ALL` (plus
`label=disable` where SELinux enforces, as in podman's own VM).
