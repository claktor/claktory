#!/usr/bin/env bash
# Claktory on Windows, through WSL2 (#476, factory-research/wsl-support.md):
# the Linux release's helper for a WSL distribution. install-linux.sh runs
# its preflight before it changes anything, and installs it as
# ~/.factory/bin/claktory-wsl (on PATH beside factoryd). Each Linux release
# also carries it as claktory-wsl, which updates put in ~/.factory/bin.
#
#   claktory-wsl check              what Claktory needs from this WSL, and
#                                   what it found; exit 0, or 3 when Claktory
#                                   can't run safely here (the reason last, on
#                                   stderr). Its one change: it turns lingering
#                                   on for you when it's off and you may (as
#                                   install-linux.sh does anyway)
#   claktory-wsl preflight          the same, for the installers, without the
#                                   note on an older install's service file
#   claktory-wsl keep-running       whether Windows keeps this Linux running
#   claktory-wsl keep-running on    adds the one Windows task that does, after
#                                   asking you in this terminal
#   claktory-wsl keep-running off   removes it
#   claktory-wsl is-wsl             exit 0 on WSL, 1 elsewhere (for the installer)
#
# What it reads on Windows (through WSL's own Windows programs, as the
# Windows user who owns this Linux; nothing elevated): the user's name and
# folders (cmd.exe), where this distribution's disk is (reg.exe query, this
# distribution's key only), whether that disk is sparse (powershell.exe,
# Get-Item), .wslconfig, Windows' sleep timers (powercfg.exe /query), whether
# Windows signs the user in after an update restart (whoami.exe, reg.exe
# query of that one setting), and Claktory's own task (schtasks.exe /Query). What
# it changes on Windows: only `keep-running on` and `off`, Claktory's own task
# "\Claktory\Keep <distribution> running", and `on` only after a yes typed
# at a terminal: a consent prompt for the person there, refused with no
# terminal. It isn't a barrier against an agent (one can get a terminal, or
# run schtasks.exe itself); what keeps agents from adding host tasks is the
# manual's rule, "The computer itself". The task also keeps Windows from
# sleeping on its own while it runs (SetThreadExecutionState, as a video
# player does; #490): a change to Windows' power behaviour, which the consent
# question says in so many words, and which ends with the task (off, sign
# out). It never changes WSL's settings, the disk, the power plan or anything
# else on the host.
#
# Standing condition (Fugo 33662): the task's action runs only Windows' own
# programs and /usr/bin/sleep, never anything under ~/.factory, ~/.local or
# any other path a user, an agent or a release can rewrite: that would be
# code Windows runs at every sign-in.
#
# CLAKTORY_WSL_PROC (default /proc) is for its tests.
set -euo pipefail
PROC="${CLAKTORY_WSL_PROC:-/proc}"
STATE="${FACTORY_HOME:-$HOME/.factory}"

say() { echo "$*" >&2; }
refuse() { echo "Claktory can't run safely here: $*" >&2; exit 3; }
# Windows programs: each answer as one line of text, CR and BOM gone, or
# nothing (interop off, or it didn't answer in 20 seconds).
win() { timeout 20 "$@" 2>/dev/null </dev/null | tr -d '\r' | sed '1s/^\xef\xbb\xbf//' || true; }
have_interop() { command -v cmd.exe >/dev/null 2>&1 && [[ -n "$(win cmd.exe /d /c ver)" ]]; }

# WSL: from the kernel, not only the environment (WSL_DISTRO_NAME can be set
# anywhere). WSL 1's kernel release ends in "-Microsoft"; WSL 2's has
# "microsoft-standard-WSL2" (or its own kernel, with WSLInterop or /run/WSL).
wsl_version() {
  local release
  release="$(cat "$PROC/sys/kernel/osrelease" 2>/dev/null || true)"
  if [[ "$release" == *-Microsoft ]]; then echo 1
  elif [[ "$release" == *[Mm]icrosoft* || -e "$PROC/sys/fs/binfmt_misc/WSLInterop" || -d /run/WSL ]]; then echo 2
  else echo 0
  fi
}

distro() {
  local name="${WSL_DISTRO_NAME:-}"
  # Nothing for any other name (and success, which set -e needs).
  if [[ "$name" =~ ^[A-Za-z0-9._-]{1,64}$ ]]; then echo "$name"; fi
}

# This distribution's folder on Windows (BasePath, from its own key under
# Lxss), as a Linux path to its ext4.vhdx, or nothing.
vhd_path() {
  local name="$1" key base file
  key="$(win reg.exe query 'HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss' /s /f "$name" /d /e |
    grep -E '^HKEY_CURRENT_USER\\' | head -n 1)"
  [[ -n "$key" ]] || return 0
  base="$(win reg.exe query "$key" /v BasePath | sed -n 's/^ *BasePath *REG_[A-Z_]* *//p' | head -n 1)"
  base="${base#\\\\\?\\}" # a \\?\ prefix
  [[ "$base" =~ ^[A-Za-z]:\\ ]] || return 0
  file="$(win reg.exe query "$key" /v VhdFileName | sed -n 's/^ *VhdFileName *REG_[A-Z_]* *//p' | head -n 1)"
  [[ "$file" =~ ^[A-Za-z0-9._-]+$ ]] || file=ext4.vhdx
  echo "$base\\$file"
}

# Bytes free on the Windows drive holding WIN_PATH (C:\…), from Linux's view
# of it (/mnt/c), or nothing.
windows_free() {
  local letter mount
  letter="$(tr 'A-Z' 'a-z' <<<"${1:0:1}")"
  mount="/mnt/$letter"
  [[ -d "$mount" ]] || return 0
  df -PB1 "$mount" 2>/dev/null | awk 'NR==2{print $4}'
}

# Whether WIN_PATH carries Windows' sparse-file attribute: yes, no, or nothing.
sparse_flag() {
  local attrs
  attrs="$(CLAKTORY_VHD="$1" WSLENV="${WSLENV:+$WSLENV:}CLAKTORY_VHD" win powershell.exe -NoProfile -NonInteractive -Command '(Get-Item -LiteralPath $env:CLAKTORY_VHD -Force).Attributes.ToString()')"
  [[ -n "$attrs" ]] || return 0
  if [[ "$attrs" == *SparseFile* ]]; then echo yes; else echo no; fi
}

# Windows' idle timer for SETTING (STANDBYIDLE: sleep, HIBERNATEIDLE:
# hibernate) on mains power, in seconds (0: never), or nothing. powercfg's
# words are in Windows' language, so the numbers are read by position: its
# last two hex values are the current AC and DC settings.
idle_after_ac() {
  win powercfg.exe /query SCHEME_CURRENT SUB_SLEEP "$1" | grep -oE '0x[0-9a-fA-F]{8}' | tail -n 2 | head -n 1 |
    { read -r hex && echo $((hex)); } 2>/dev/null || true
}

# Whether Windows signs this user in after an update restarts the computer
# ("Use my sign-in info to automatically finish setting up after an
# update"): on, off, or nothing (couldn't tell).
arso() {
  local sid policy optout
  policy="$(win reg.exe query 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' /v DisableAutomaticRestartSignOn |
    sed -n 's/^ *DisableAutomaticRestartSignOn *REG_DWORD *//p' | head -n 1)"
  [[ "$policy" != 0x1 ]] || { echo off; return 0; }
  sid="$(win whoami.exe /user /fo csv /nh | sed -n 's/.*,"\(S-1-5-[0-9-]*\)"$/\1/p' | head -n 1)"
  [[ -n "$sid" ]] || return 0
  optout="$(win reg.exe query "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon\\UserARSO\\$sid" /v OptOut |
    sed -n 's/^ *OptOut *REG_DWORD *//p' | head -n 1)"
  case "$optout" in 0x0) echo on ;; 0x1) echo off ;; esac
}

gib() { awk -v b="$1" 'BEGIN{printf "%.1f GB", b/1073741824}'; }

# The checks. Unsafe states refuse; what can't be seen is said to be
# unverified, never called safe.
check() {
  local mode="${1:-check}" version name user linux_free inodes fs probe vhd free sparse wslconfig warnings=()
  version="$(wsl_version)"
  [[ "$version" != 0 ]] || { say "This isn't WSL: nothing to check."; return 0; }
  [[ "$version" == 2 ]] || refuse "this is WSL 1, and Claktory needs WSL 2. In Windows PowerShell: wsl --set-version ${WSL_DISTRO_NAME:-<distribution>} 2 (back up first with wsl --export), then install again."
  name="$(distro)"; user="$(id -un)"
  # systemd runs this Linux (process 1), or nothing keeps Claktory running.
  [[ "$(cat "$PROC/1/comm" 2>/dev/null)" == systemd ]] ||
    refuse "systemd isn't running in this Linux. Add these two lines to /etc/wsl.conf (sudo nano /etc/wsl.conf):  [boot]  and  systemd=true  then in Windows PowerShell run  wsl --terminate ${name:-<distribution>}  (it stops this Linux and whatever runs in it), open it again, and install again."
  # Claktory's data stays in Linux's own disk, never on a Windows drive.
  fs="$(stat -f -c %T "$HOME" 2>/dev/null || true)"
  case "$fs" in
    v9fs | 9p | drvfs | fuseblk | cifs | smb2) refuse "your home folder ($HOME) is on a Windows drive ($fs). Claktory keeps its data in your Linux home: use a Linux user whose home is in this Linux (/home/…)." ;;
  esac
  # Writable, all the way: a file written, flushed and renamed.
  probe="$HOME/.claktory-wsl-probe.$$"
  if ! { printf 'probe' >"$probe.tmp" && sync "$probe.tmp" 2>/dev/null && mv -f "$probe.tmp" "$probe" && rm -f "$probe"; } 2>/dev/null; then
    rm -f "$probe.tmp" "$probe" 2>/dev/null || true
    refuse "this Linux can't write to your home folder (its disk may be read-only after an error). Don't install or repair anything now: in Windows, back up what you can, and repair or restore the disk first."
  fi
  # Room in Linux, in bytes and in files.
  linux_free="$(df -PB1 "$HOME" | awk 'NR==2{print $4}')"
  inodes="$(df -Pi "$HOME" | awk 'NR==2{print ($2 > 0 ? $4 : "")}')"
  ((linux_free >= 2 * 1073741824)) || refuse "this Linux has $(gib "$linux_free") free, and Claktory needs at least 2 GB. Free some space in it first."
  [[ ! "$inodes" =~ ^[0-9]+$ ]] || ((inodes >= 10000)) || refuse "this Linux's disk has room for only $inodes more files. Free some first."
  ((linux_free >= 10 * 1073741824)) || warnings+=("this Linux has $(gib "$linux_free") free; agents' work grows, so keep at least 10 GB.")
  # Lingering: Claktory's service starts when this Linux starts, with no one
  # logged in. Turning it on for yourself may need root.
  if ! loginctl show-user "$user" -p Linger 2>/dev/null | grep -q yes && ! loginctl enable-linger "$user" 2>/dev/null; then
    refuse "Claktory has to start when this Linux starts, which needs lingering for $user. Run  sudo loginctl enable-linger $user  then install again."
  fi
  # An older install's service file (#490): updates don't rewrite it, and
  # Claktory runs without these two lines (factoryd raises its own open-files
  # limit), so it's a note, not a refusal. Not while installing: the install
  # writes it anew.
  local unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/factoryd.service"
  if [[ "$mode" == check && -f "$unit" ]] && ! { grep -qx 'StartLimitIntervalSec=0' "$unit" && grep -qx 'LimitNOFILE=65536' "$unit"; }; then
    warnings+=("Claktory's service file ($unit) is from an older install. Claktory works with it, but it lacks two settings: one keeps systemd restarting Claktory however often it stops, the other lets it open more files at once. To add them, run Claktory's install command again: it keeps your agents, settings and sign-ins, and restarts Claktory.")
  fi
  # Windows: the disk's drive and its sparse flag. Linux's df shows the disk's
  # ceiling (often 1 TB), not what the Windows drive under it has left.
  if [[ -z "$name" ]] || ! have_interop; then
    warnings+=("Claktory couldn't ask Windows about this Linux's disk (WSL's Windows interop is off, or the distribution has no name), so the Windows drive's free space and whether the disk is sparse are unverified. Check in Windows that the drive holding it has at least 10 GB free, and never make it sparse.")
  else
    vhd="$(vhd_path "$name")"
    if [[ -z "$vhd" ]]; then
      warnings+=("Claktory couldn't find ${name}'s disk in Windows, so the drive's free space and whether the disk is sparse are unverified.")
    else
      sparse="$(sparse_flag "$vhd")"
      [[ "$sparse" != yes ]] || refuse "${name}'s disk ($vhd) is set as sparse, which WSL itself marks unsafe (--allow-unsafe): it can corrupt the disk. Don't install anything now. In Windows PowerShell: back this Linux up (wsl --export $name <file>.tar), then  wsl --terminate $name  and  wsl --manage $name --set-sparse false  then open it again and install again."
      [[ -n "$sparse" ]] || warnings+=("Claktory couldn't read whether ${name}'s disk is sparse: unverified.")
      free="$(windows_free "$vhd")"
      if [[ "$free" =~ ^[0-9]+$ ]]; then
        ((free >= 5 * 1073741824)) || refuse "the Windows drive holding this Linux's disk (${vhd:0:2}) has $(gib "$free") free. When it fills, this Linux's disk can fail. Free space on ${vhd:0:2} first: at least 10 GB."
        ((free >= 20 * 1073741824)) || warnings+=("the Windows drive holding this Linux's disk (${vhd:0:2}) has $(gib "$free") free; keep at least 20 GB, since this Linux's disk grows into it.")
      else
        warnings+=("Claktory couldn't read the free space on ${vhd:0:2}, which holds this Linux's disk: unverified.")
      fi
    fi
    # Unattended (#490): Windows' own sleep stops Claktory, and after an
    # update restart nothing runs until someone signs in.
    local sleep_ac hib_ac awake=false restart
    keep_running_status "$name" >/dev/null 2>&1 && awake=true
    sleep_ac="$(idle_after_ac STANDBYIDLE)"; hib_ac="$(idle_after_ac HIBERNATEIDLE)"
    if [[ -z "$sleep_ac" ]]; then
      warnings+=("Claktory couldn't read when Windows goes to sleep: unverified. While Windows sleeps, Claktory stops.")
    elif ((sleep_ac > 0 || ${hib_ac:-0} > 0)) && ! $awake; then
      warnings+=("Windows goes to sleep after $(( (sleep_ac > 0 ? sleep_ac : hib_ac) / 60 )) minutes idle even when plugged in, and Claktory stops while it sleeps. To keep it working: claktory-wsl keep-running on (it also keeps Windows from sleeping on its own), or in Windows Settings → System → Power, set sleep when plugged in to Never.")
    fi
    restart="$(arso)"
    if [[ "$restart" != on ]]; then
      local why="Claktory couldn't tell whether Windows signs you in after an update"
      [[ "$restart" != off ]] || why="Windows doesn't sign you in after an update"
      warnings+=("after Windows Update restarts this computer, Claktory waits until you sign in again ($why). To have it carry on: Windows Settings → Accounts → Sign-in options → turn on \"Use my sign-in info to automatically finish setting up after an update\" (Windows then signs you in and locks the screen after an update restart).")
    fi
    # .wslconfig asking for sparse disks: new ones would be made sparse.
    wslconfig="$(win cmd.exe /d /c 'echo %USERPROFILE%')"
    if [[ -n "$wslconfig" ]] && wslconfig="$(wslpath -u "$wslconfig" 2>/dev/null)/.wslconfig" && [[ -f "$wslconfig" ]] &&
      tr -d '\r ' <"$wslconfig" | grep -qi '^sparseVhd=true'; then
      warnings+=("your .wslconfig has sparseVhd=true, which makes new WSL disks sparse, and WSL marks that unsafe. Remove that line in Windows ($wslconfig).")
    fi
  fi
  local w
  for w in ${warnings[@]+"${warnings[@]}"}; do say "Note: $w"; done
  return 0
}

# The Windows task: one per distribution, in Task Scheduler's Claktory folder.
task_name() { echo "\\Claktory\\Keep $1 running"; }

# The distribution's name goes into the task's name, its XML and its
# PowerShell command: only these characters, whoever calls (Fugo, #490).
safe_name() { [[ "$1" =~ ^[A-Za-z0-9._-]{1,64}$ ]] || { say "This WSL distribution's name can't go in a Windows task safely."; exit 3; }; }

keep_running_status() {
  local name="$1" out
  safe_name "$name"
  out="$(win schtasks.exe /Query /TN "$(task_name "$name")" /FO LIST /V)"
  if [[ -z "$out" ]]; then
    echo "off: Windows stops $name a few seconds after its last window closes, and Claktory with it."
    return 1
  fi
  echo "on: Windows starts $name when you sign in, keeps it running and doesn't sleep on its own meanwhile (task \"$(task_name "$name")\", $(sed -n 's/^Status: *//p' <<<"$out" | head -n 1 | tr 'A-Z' 'a-z'), last result $(sed -n 's/^Last Result: *//p' <<<"$out" | head -n 1))."
}

keep_running_on() {
  local name="$1" user windows_user tmp_win tmp xml answer
  safe_name "$name"
  user="$(id -un)"
  [[ "$user" =~ ^[a-z_][a-z0-9_.-]{0,31}$ ]] || { say "Your Linux user name ($user) can't go in a Windows task safely."; exit 3; }
  windows_user="$(win cmd.exe /d /c 'echo %USERDOMAIN%\%USERNAME%')"
  [[ "$windows_user" =~ ^[A-Za-z0-9._\ -]{1,64}\\[A-Za-z0-9._\ -]{1,64}$ ]] || { say "Claktory couldn't read your Windows user name."; exit 1; }
  win wsl.exe --list --quiet | iconv -f UTF-16LE -t UTF-8 2>/dev/null | tr -d '\r\0' | grep -qxF "$name" ||
    win wsl.exe --list --quiet | tr -d '\r\0' | grep -qxF "$name" || { say "Windows doesn't list a distribution named $name."; exit 1; }
  # The person's consent, typed at a terminal; with no terminal, refused.
  { : </dev/tty >/dev/tty; } 2>/dev/null || { say "Adding the Windows task needs your answer in a terminal: run  claktory-wsl keep-running on  yourself."; exit 3; }
  cat >/dev/tty <<ASK

Windows stops this Linux ($name) a few seconds after its last window
closes, and Claktory stops with it. Claktory can add one task to Windows:

  "$(task_name "$name")", which, when $windows_user signs in to Windows,
  starts $name as $user and keeps it running until you sign out.

It also changes how Windows sleeps: while it runs, Windows doesn't go to
sleep on its own when the computer sits idle, plugged in or on battery,
so a laptop uses more power. Your screen still turns off as set, and
closing the lid, pressing the power button or choosing Sleep still put
the computer to sleep (and Claktory stops until it wakes). Nothing else
changes: not your power plan, not WSL's settings. It needs no
administrator rights and keeps no password. Turning it off ends both:
claktory-wsl keep-running off  (or in Task Scheduler, folder Claktory: End, then Delete).

ASK
  printf 'Add this task, which also keeps Windows from sleeping on its own? [y/N] ' >/dev/tty
  read -r answer </dev/tty || answer=""
  [[ "$answer" =~ ^[Yy]([Ee][Ss])?$ ]] || { echo "Not added." >/dev/tty; exit 0; }
  tmp_win="$(win cmd.exe /d /c 'echo %TEMP%')"
  tmp="$(wslpath -u "$tmp_win" 2>/dev/null)" && [[ -d "$tmp" ]] || { say "Claktory couldn't reach your Windows temporary folder."; exit 1; }
  xml="$tmp/claktory-keep-running-$$.xml"
  # Task Scheduler's own format, as UTF-16. No elevation, no stored
  # password, no wake timer, no time limit, one instance, restarted 3 times
  # a minute apart if it stops, on battery too. conhost --headless runs, with
  # no window, Windows PowerShell with a fixed command written here in the
  # task, readable in Task Scheduler (no script file anywhere): it asks
  # Windows not to sleep while its thread runs (ES_CONTINUOUS |
  # ES_SYSTEM_REQUIRED; the lid, the power button and Sleep still work), then
  # runs wsl.exe and waits on it. The C# string's quotes are [char]34, so the
  # command has no quote to escape on its way through conhost. If PowerShell
  # can't load the call (a locked-down Windows), it still keeps Linux running.
  { printf '\xff\xfe'; cat <<XML | iconv -f UTF-8 -t UTF-16LE; } >"$xml"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>Claktory</Author>
    <Description>Starts the Linux distribution $name as $user when $windows_user signs in, and keeps it running so Claktory keeps working; while it runs, Windows doesn't go to sleep on its own when idle (the lid, the power button and Sleep still work). Added by Claktory with your consent. Remove it with: claktory-wsl keep-running off</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>$windows_user</UserId></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author"><UserId>$windows_user</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>%SystemRoot%\System32\conhost.exe</Command>
      <Arguments>--headless %SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe -NoProfile -NonInteractive -Command \$q=[char]34; try { \$k=Add-Type -Name ExecutionState -Namespace Claktory -PassThru -MemberDefinition ('[DllImport('+\$q+'kernel32.dll'+\$q+')] public static extern uint SetThreadExecutionState(uint flags);'); [void]\$k::SetThreadExecutionState([uint32]2147483649) } catch {}; &amp; \$env:SystemRoot\System32\wsl.exe --distribution $name --user $user --exec /usr/bin/sleep infinity; exit \$LASTEXITCODE</Arguments>
    </Exec>
  </Actions>
</Task>
XML
  local result
  result="$(win schtasks.exe /Create /TN "$(task_name "$name")" /XML "$tmp_win\\claktory-keep-running-$$.xml" /F)"
  rm -f "$xml"
  if keep_running_status "$name" >/dev/null; then
    # Started now, not only at the next sign-in.
    win schtasks.exe /Run /TN "$(task_name "$name")" >/dev/null
    echo "Added and started. Windows now starts $name when you sign in, keeps it running, and doesn't sleep on its own meanwhile. To remove it: claktory-wsl keep-running off"
  else
    say "Windows didn't add the task${result:+: $result}"
    exit 1
  fi
}

keep_running_off() {
  local name="$1"
  safe_name "$name"
  if ! keep_running_status "$name" >/dev/null; then echo "There's no Claktory task for $name in Windows."; return 0; fi
  # The running one first: ending it ends its hold on sleep too.
  win schtasks.exe /End /TN "$(task_name "$name")" >/dev/null
  win schtasks.exe /Delete /TN "$(task_name "$name")" /F >/dev/null
  if keep_running_status "$name" >/dev/null; then say "Windows didn't remove the task; delete \"$(task_name "$name")\" in Task Scheduler."; exit 1; fi
  echo "Removed. Windows sleeps as it did before, and stops $name a few seconds after its last window closes, and Claktory with it."
}

case "${1:-}" in
  check | preflight) check "$1" ;;
  # For install-linux.sh: exit 0 on WSL (1 or 2), 1 elsewhere.
  is-wsl) [[ "$(wsl_version)" != 0 ]] ;;
  distro) distro ;;
  keep-running)
    [[ "$(wsl_version)" == 2 ]] || { say "This isn't WSL 2."; exit 3; }
    name="$(distro)"
    [[ -n "$name" ]] || { say "This WSL distribution has no name Claktory can use."; exit 3; }
    have_interop || { say "WSL's Windows interop is off here, so Claktory can't reach Windows' Task Scheduler. Add the task in Windows yourself, or turn interop on."; exit 3; }
    case "${2:-status}" in
      status) keep_running_status "$name" || true ;;
      on) keep_running_on "$name" ;;
      off) keep_running_off "$name" ;;
      *) say "usage: claktory-wsl keep-running [on|off]"; exit 2 ;;
    esac
    ;;
  *) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 2 ;;
esac
