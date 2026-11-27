//! The Windows app's check of whom it reached on this PC, before it sends
//! its own Claktory anything, the app's token included: another account
//! could be listening on the backend's port while the backend is down, and
//! would otherwise get the token and whatever the person typed after it.
//!
//! The function is here on every platform, so the bindings are the same
//! everywhere; the check itself is Windows only.

/// Whether the other end of the app's loopback connection, from
/// 127.0.0.1:[client_port] to 127.0.0.1:[server_port], belongs to a process
/// running as this same user. True only when exactly one TCP connection on
/// this PC has the local end 127.0.0.1:[server_port] and the remote end
/// 127.0.0.1:[client_port], and its process's user is this process's user.
/// False or an error otherwise (no such connection, more than one, a
/// process that can't be read): both mean "don't send anything". The error
/// says why, for the log. IPv4 only: the app connects to 127.0.0.1.
pub fn loopback_peer_same_user(server_port: u16, client_port: u16) -> Result<bool, String> {
    #[cfg(windows)]
    {
        check::peer_same_user(server_port, client_port)
    }
    #[cfg(not(windows))]
    {
        let _ = (server_port, client_port);
        Err("only on Windows".into())
    }
}

#[cfg(windows)]
mod check {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS};
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    use windows_sys::Win32::Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

    /// 127.0.0.1 as the table holds it: in network byte order.
    const LOOPBACK: u32 = u32::from_ne_bytes([127, 0, 0, 1]);

    pub(super) fn peer_same_user(server_port: u16, client_port: u16) -> Result<bool, String> {
        let rows = connections()?;
        let matching: Vec<&MIB_TCPROW_OWNER_PID> = rows
            .iter()
            .filter(|row| {
                row.dwLocalAddr == LOOPBACK
                    && port(row.dwLocalPort) == server_port
                    && row.dwRemoteAddr == LOOPBACK
                    && port(row.dwRemotePort) == client_port
            })
            .collect();
        let [row] = matching[..] else {
            return Err(format!("{} connections from port {client_port} to port {server_port}, not one", matching.len()));
        };
        // The idle and System processes never hold a connection of ours.
        if row.dwOwningPid <= 4 {
            return Ok(false);
        }
        let process = Handle::open_process(row.dwOwningPid)?;
        let theirs = TokenUserSid::of(&process)?;
        let ours = TokenUserSid::of(&Handle::current_process())?;
        // SAFETY: two valid SIDs, each alive in its buffer.
        Ok(unsafe { EqualSid(theirs.sid(), ours.sid()) } != 0)
    }

    /// A port as the table holds it: in network byte order, in the low 16
    /// bits.
    fn port(raw: u32) -> u16 {
        u16::from_be(raw as u16)
    }

    /// Every IPv4 TCP connection on this PC, with its process.
    fn connections() -> Result<Vec<MIB_TCPROW_OWNER_PID>, String> {
        let mut size: u32 = 0;
        for _ in 0..8 {
            // u64s, so the table is aligned as its rows need.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
            let mut room = (buffer.len() * 8) as u32;
            // SAFETY: `room` bytes to write; it says how many it needs.
            let result = unsafe { GetExtendedTcpTable(buffer.as_mut_ptr().cast(), &raw mut room, 0, u32::from(AF_INET), TCP_TABLE_OWNER_PID_CONNECTIONS, 0) };
            if result == ERROR_INSUFFICIENT_BUFFER {
                // Connections come and go: some room to spare.
                size = room + 16 * size_of::<MIB_TCPROW_OWNER_PID>() as u32;
                continue;
            }
            if result != NO_ERROR {
                return Err(format!("couldn't read the TCP table: error {result}"));
            }
            // MIB_TCPTABLE_OWNER_PID: a u32 count, then the rows, aligned.
            let base = buffer.as_ptr().cast::<u8>();
            // SAFETY: the table's header is at its start.
            let count = unsafe { base.cast::<u32>().read() } as usize;
            let offset = std::mem::offset_of!(windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPTABLE_OWNER_PID, table);
            if offset + count * size_of::<MIB_TCPROW_OWNER_PID>() > buffer.len() * 8 {
                return Err("the TCP table is shorter than it says".into());
            }
            // SAFETY: `count` rows after the header, inside the buffer (checked).
            let rows = unsafe { std::slice::from_raw_parts(base.add(offset).cast::<MIB_TCPROW_OWNER_PID>(), count) };
            return Ok(rows.to_vec());
        }
        Err("the TCP table kept growing".into())
    }

    /// A handle closed when dropped.
    struct Handle(HANDLE);

    impl Handle {
        fn open_process(pid: u32) -> Result<Handle, String> {
            // SAFETY: plain call; the handle is checked.
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if handle.is_null() {
                return Err(format!("couldn't open process {pid}: {}", std::io::Error::last_os_error()));
            }
            Ok(Handle(handle))
        }

        /// This process, by its pseudo handle, which needs no closing.
        fn current_process() -> Handle {
            // SAFETY: plain call.
            Handle(unsafe { GetCurrentProcess() })
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // The pseudo handle (-1) isn't closed.
            if self.0 as isize != -1 {
                // SAFETY: a handle this owns, closed once.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// A process's user, read from its token while the process's handle is
    /// held open.
    struct TokenUserSid {
        buffer: Vec<u64>,
    }

    impl TokenUserSid {
        fn of(process: &Handle) -> Result<TokenUserSid, String> {
            let mut token: HANDLE = std::ptr::null_mut();
            // SAFETY: a process handle and the token to write.
            if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &raw mut token) } == 0 {
                return Err(format!("couldn't read the process's token: {}", std::io::Error::last_os_error()));
            }
            let token = Handle(token);
            let mut needed: u32 = 0;
            // SAFETY: asks for the size only.
            unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &raw mut needed) };
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            // SAFETY: `needed` bytes to write.
            if unsafe { GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &raw mut needed) } == 0 {
                return Err(format!("couldn't read the token's user: {}", std::io::Error::last_os_error()));
            }
            Ok(TokenUserSid { buffer })
        }

        fn sid(&self) -> windows_sys::Win32::Security::PSID {
            // SAFETY: the buffer holds a TOKEN_USER, whose SID points inside it.
            unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_server_in_this_process_is_this_user_and_no_server_is_not() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap().port();
        let client = std::net::TcpStream::connect(("127.0.0.1", server)).unwrap();
        let (_accepted, _) = listener.accept().unwrap();
        let client_port = client.local_addr().unwrap().port();
        assert_eq!(loopback_peer_same_user(server, client_port), Ok(true));
        // A port nothing listens on.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_ne!(loopback_peer_same_user(free, client_port), Ok(true));
    }

    #[cfg(not(windows))]
    #[test]
    fn elsewhere_it_says_it_is_only_on_windows() {
        assert_eq!(loopback_peer_same_user(7420, 50000), Err("only on Windows".into()));
    }
}
