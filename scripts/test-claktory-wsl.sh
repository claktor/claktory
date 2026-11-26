#!/usr/bin/env bash
# Tries scripts/claktory-wsl.sh (Claktory on WSL2, #476) in a throwaway
# Ubuntu 24.04 container, with WSL faked: its kernel markers through
# CLAKTORY_WSL_PROC, and Windows' programs (cmd.exe, reg.exe, powershell.exe,
# schtasks.exe, wsl.exe), wslpath and loginctl as stubs that answer as a
# Windows 11 host would, from files a case sets. Checks each refusal and
# warning, and that the Windows task is added only after a yes typed in a
# terminal, with the settings it promises. Needs podman. A real Windows
# host is QA's (wsl-support.md §5).
#
#   scripts/test-claktory-wsl.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
W="$(mktemp -d "$HOME/.claktory-wsl-test.XXXXXX")" # podman sees $HOME, not /tmp
trap 'rm -rf "$W"' EXIT
cp "$HERE/claktory-wsl.sh" "$W/claktory-wsl"
mkdir -p "$W/release"; cp "$HERE/install-linux.sh" "$HERE/claktory-wsl.sh" "$W/release/"; printf '#!/bin/sh\nexit 0\n' >"$W/release/factoryd"
cat >"$W/run.sh" <<'RUN'
set -u
apt-get update -qq >/dev/null 2>&1; apt-get install -y -qq util-linux >/dev/null 2>&1
useradd -m ana
S=/stubs; mkdir -p $S /mnt/c /win/temp /win/users/ana
# Windows, as the stubs see it: files under /win/state a case changes.
mkdir -p /win/state; chmod -R 777 /win
cat >$S/cmd.exe <<'EOF'
#!/bin/sh
[ -f /win/state/no-interop ] && exit 1
case "$*" in
  *ver*) printf 'Microsoft Windows [Version 10.0.26100.2605]\r\n' ;;
  *USERPROFILE*) printf 'C:\\Users\\ana\r\n' ;;
  *USERDOMAIN*) printf 'LAPTOP-1\\ana\r\n' ;;
  *TEMP*) printf 'C:\\Users\\ana\\AppData\\Local\\Temp\r\n' ;;
esac
EOF
cat >$S/reg.exe <<'EOF'
#!/bin/sh
K='HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Lxss\{0b3c3d}'
case "$*" in
  */f*) printf '\r\n%s\r\n    DistributionName    REG_SZ    Ubuntu-24.04\r\n\r\nEnd of search: 1 match(es) found.\r\n' "$K" ;;
  *BasePath*) printf '\r\n%s\r\n    BasePath    REG_SZ    \\\\?\\C:\\Users\\ana\\AppData\\Local\\wsl\\{0b3c3d}\r\n' "$K" ;;
  *VhdFileName*) printf '\r\n%s\r\n    VhdFileName    REG_SZ    ext4.vhdx\r\n' "$K" ;;
  *DisableAutomaticRestartSignOn*) [ -f /win/state/arso-policy-off ] && printf '\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\r\n    DisableAutomaticRestartSignOn    REG_DWORD    0x1\r\n' || exit 1 ;;
  *UserARSO*S-1-5-21-1-2-3-1001*OptOut*) [ -f /win/state/arso-unknown ] && exit 1; [ -f /win/state/arso-off ] && v=0x1 || v=0x0; printf '\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\r\n    OptOut    REG_DWORD    %s\r\n' "$v" ;;
esac
EOF
cat >$S/whoami.exe <<'EOF'
#!/bin/sh
printf '"laptop-1\\ana","S-1-5-21-1-2-3-1001"\r\n'
EOF
cat >$S/powercfg.exe <<'EOF'
#!/bin/sh
# STANDBYIDLE or HIBERNATEIDLE; the words in German, to show they aren't read.
ac=0x00000000; case "$*" in *STANDBYIDLE*) [ -f /win/state/sleeps ] && ac=0x00000708 ;; esac
printf 'Energieschema-GUID: 381b4222\r\n    Minimaler Wert: 0x00000000\r\n    Maximaler Wert: 0xffffffff\r\n    Aktueller Wechselstrom-Einstellungsindex: %s\r\n    Aktueller Gleichstrom-Einstellungsindex: 0x00000384\r\n' "$ac"
EOF
cat >$S/powershell.exe <<'EOF'
#!/bin/sh
# Get-Item on the disk: the path must come through WSLENV, not the command.
[ "$CLAKTORY_VHD" = 'C:\Users\ana\AppData\Local\wsl\{0b3c3d}\ext4.vhdx' ] || { echo "wrong path: $CLAKTORY_VHD" >&2; exit 1; }
if [ -f /win/state/sparse ]; then printf 'Archive, SparseFile\r\n'; else printf 'Archive\r\n'; fi
EOF
cat >$S/schtasks.exe <<'EOF'
#!/bin/sh
printf '%s|' "$@" >>/win/state/schtasks.log; echo >>/win/state/schtasks.log
xml=""; prev=""; for a in "$@"; do [ "$prev" = /XML ] && xml="$a"; prev="$a"; done
case "$1" in
  /Query) [ -f /win/state/task.xml ] && printf 'TaskName: \\Claktory\\Keep Ubuntu-24.04 running\r\nStatus: Running\r\nLast Result: 267009\r\n' || exit 1 ;;
  /Create) cp "/win/temp/${xml##*\\}" /win/state/task.xml && printf 'SUCCESS: The scheduled task has successfully been created.\r\n' ;;
  /Delete) rm -f /win/state/task.xml ;;
  /Run | /End) [ -f /win/state/task.xml ] || exit 1 ;;
esac
EOF
cat >$S/wsl.exe <<'EOF'
#!/bin/sh
printf 'Ubuntu-24.04\r\nkali-linux\r\n' | iconv -f UTF-8 -t UTF-16LE
EOF
cat >$S/wslpath <<'EOF'
#!/bin/sh
case "$2" in
  'C:\Users\ana') echo /win/users/ana ;;
  'C:\Users\ana\AppData\Local\Temp') echo /win/temp ;;
  *) exit 1 ;;
esac
EOF
cat >$S/loginctl <<'EOF'
#!/bin/sh
case "$1" in
  show-user) [ -f /win/state/linger ] && echo Linger=yes || echo Linger=no ;;
  enable-linger) [ -f /win/state/linger-allowed ] && touch /win/state/linger ;;
esac
EOF
chmod +x $S/*
# Every call to a Windows stub, and loginctl's changes, logged: plain Linux
# must make none.
for f in cmd.exe reg.exe powershell.exe schtasks.exe wsl.exe whoami.exe powercfg.exe; do mv $S/$f $S/.$f; printf '#!/bin/sh\necho %s >>/win/state/calls.log\nexec %s/.%s "$@"\n' $f $S $f >$S/$f; chmod +x $S/$f; done
sed -i 's|^  enable-linger)|  enable-linger) echo enable-linger >>/win/state/calls.log;|' $S/loginctl
install -m 0755 /t/claktory-wsl /usr/local/bin/claktory-wsl
# The fake kernel: WSL 2 with systemd as process 1, unless a case changes it.
P=/proc-fake; mkdir -p $P/sys/kernel $P/1 $P/sys/fs/binfmt_misc
echo 6.6.87.2-microsoft-standard-WSL2 >$P/sys/kernel/osrelease; echo systemd >$P/1/comm
chmod -R 777 $P
as_ana() { su ana -c "PATH=$S:\$PATH CLAKTORY_WSL_PROC=$P WSL_DISTRO_NAME=Ubuntu-24.04 $*"; }
case_() { printf '%-34s ' "$1"; shift; out="$(as_ana claktory-wsl check 2>&1)"; code=$?; echo "exit $code: $(tail -n 1 <<<"$out" | cut -c1-150)"; }
touch /win/state/linger-allowed
echo "== check (expect: note on Linux space or none, exit 0; then each refusal or note)"
case_ "healthy (linger turned on)"; [ -f /win/state/linger ] && echo "   lingering turned on"
touch /win/state/sleeps; case_ "sleeps after 30 min on AC"; rm /win/state/sleeps
touch /win/state/arso-off; case_ "no sign-in after updates"; rm /win/state/arso-off
touch /win/state/arso-unknown; case_ "sign-in after updates unknown"; rm /win/state/arso-unknown
touch /win/state/arso-policy-off; case_ "turned off by policy"; rm /win/state/arso-policy-off
echo 4.4.0-19041-Microsoft >$P/sys/kernel/osrelease; case_ "WSL 1"; echo 6.6.87.2-microsoft-standard-WSL2 >$P/sys/kernel/osrelease
echo init >$P/1/comm; case_ "no systemd"; echo systemd >$P/1/comm
touch /win/state/sparse; case_ "sparse disk"; rm /win/state/sparse
rm /win/state/linger /win/state/linger-allowed; case_ "no lingering, not allowed"; touch /win/state/linger
touch /win/state/no-interop; case_ "interop off (unverified note)"; rm /win/state/no-interop
printf '[experimental]\r\nsparseVhd = true\r\n' >/win/users/ana/.wslconfig; case_ ".wslconfig sparseVhd"; rm /win/users/ana/.wslconfig
chmod 555 /home/ana; case_ "home not writable"; chmod 755 /home/ana
echo 5.15.0-91-generic >$P/sys/kernel/osrelease; rmdir $P/sys/fs/binfmt_misc 2>/dev/null; case_ "not WSL"; echo 6.6.87.2-microsoft-standard-WSL2 >$P/sys/kernel/osrelease
U=/home/ana/.config/systemd/user; mkdir -p $U; printf '[Unit]\nDescription=Claktory\n\n[Service]\nRestart=always\n' >$U/factoryd.service; chown -R ana /home/ana/.config
case_ "older service file (note)"
printf '%-34s ' "the same, as the installer's"; as_ana claktory-wsl preflight 2>&1 | grep -c 'older install' | sed 's/^/notes about it: /'
printf '[Unit]\nStartLimitIntervalSec=0\n[Service]\nLimitNOFILE=65536\n' >$U/factoryd.service; printf '%-34s ' "current service file"; as_ana claktory-wsl check 2>&1 | grep -c 'older install' | sed 's/^/notes about it: /'; rm -rf /home/ana/.config
echo "== a distribution name with ; & or quotes (expect: refused, exit 3, Task Scheduler never asked)"
: >/win/state/schtasks.log
su ana -c "PATH=$S:\$PATH CLAKTORY_WSL_PROC=$P WSL_DISTRO_NAME='Ub;untu&x\"' claktory-wsl keep-running off" 2>&1 | sed 's/^/   /'; echo "   exit ${PIPESTATUS[0]}"
echo "   schtasks calls: $(wc -l </win/state/schtasks.log)"
su ana -c "PATH=$S:\$PATH CLAKTORY_WSL_PROC=$P WSL_DISTRO_NAME='Ub;untu' claktory-wsl check" 2>&1 | tail -n 1 | cut -c1-150 | sed 's/^/   check: /'; echo "   check exit ${PIPESTATUS[0]}"
echo "== keep-running (expect: off; refused with no terminal; not added on no; added on yes; on; removed)"
as_ana claktory-wsl keep-running; echo "   exit $?"
as_ana claktory-wsl keep-running on </dev/null 2>&1 | tail -n 1; echo "   exit ${PIPESTATUS[0]}"
echo n | su ana -c "PATH=$S:\$PATH CLAKTORY_WSL_PROC=$P WSL_DISTRO_NAME=Ubuntu-24.04 script -qec 'claktory-wsl keep-running on' /dev/null" | tail -n 1 | tr -d '\r'
[ -f /win/state/task.xml ] && echo "   WRONG: added on no" || echo "   nothing added"
echo y | su ana -c "PATH=$S:\$PATH CLAKTORY_WSL_PROC=$P WSL_DISTRO_NAME=Ubuntu-24.04 script -qec 'claktory-wsl keep-running on' /dev/null" >/win/state/consent.txt; tail -n 1 /win/state/consent.txt | tr -d '\r'
grep -q 'It also changes how Windows sleeps' /win/state/consent.txt && grep -q 'keeps Windows from sleeping on its own? \[y/N\]' /win/state/consent.txt && echo "   the question says it changes how Windows sleeps"
as_ana claktory-wsl keep-running
touch /win/state/sleeps; case_ "sleeps on AC, keep-running on"; rm /win/state/sleeps
echo "== the task Windows got"
ls /win/temp | grep -c xml | sed 's/^/   temp files left: /'
head -c 2 /win/state/task.xml | od -An -tx1 | sed 's/^/   BOM:/'
iconv -f UTF-16LE -t UTF-8 /win/state/task.xml | grep -oE '<(UserId|LogonType|RunLevel|DisallowStartIfOnBatteries|StopIfGoingOnBatteries|WakeToRun|ExecutionTimeLimit|MultipleInstancesPolicy|Command|Arguments)>[^<]*' | sed 's/^/   /'
grep -hE '^/(Create|Run)' /win/state/schtasks.log | sed 's/^/   schtasks /'
as_ana claktory-wsl keep-running off; as_ana claktory-wsl keep-running
grep -hE '^/(End|Delete)' /win/state/schtasks.log | sed 's/^/   schtasks /'
echo "== plain Linux, not WSL (expect: check exits 0 and calls nothing; install-linux.sh never runs claktory-wsl and stops only at systemd, as before)"
rm -f /win/state/calls.log /win/state/linger; echo 6.8.0-49-generic >$P/sys/kernel/osrelease; rmdir $P/sys/fs/binfmt_misc 2>/dev/null || true
as_ana claktory-wsl check 2>&1 | sed 's/^/   /'; echo "   check exit ${PIPESTATUS[0]}"
cp -R /t/release /home/ana/plain; chown -R ana /home/ana/plain
as_ana bash /home/ana/plain/install-linux.sh /home/ana/plain 2>&1 | tail -n 1 | sed 's/^/   /'
echo "   Windows programs or lingering changes called: $(cat /win/state/calls.log 2>/dev/null | wc -l)"
[ -e /home/ana/.factory/bin/claktory-wsl ] && echo "   WRONG: claktory-wsl installed" || echo "   no claktory-wsl installed"
echo 6.6.87.2-microsoft-standard-WSL2 >$P/sys/kernel/osrelease; touch /win/state/linger
echo "== install-linux.sh on an unsafe WSL (expect: exit 3 with the reason, nothing written to ~/.factory)"
touch /win/state/sparse; cp -R /t/release /home/ana/release; chown -R ana /home/ana/release
as_ana bash /home/ana/release/install-linux.sh /home/ana/release 2>&1 | tail -n 1 | cut -c1-120; echo "   exit ${PIPESTATUS[0]}"
[ -e /home/ana/.factory ] && echo "   WRONG: ~/.factory written" || echo "   ~/.factory untouched"
RUN
podman run --rm -v "$W:/t" docker.io/library/ubuntu:24.04 bash /t/run.sh
