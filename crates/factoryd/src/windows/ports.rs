//! What listens on a port of this computer, on Windows: what /proc's TCP
//! tables and lsof tell the backend elsewhere (`card_actions::heard`), from
//! the TCP tables Windows keeps with each socket's process.

use factory_agent::windows::system::{creation_time, image_path, open_process, user_sid, user_sid_of};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// A socket listening on the port asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socket {
    pub v6: bool,
    /// Its address as lsof spells it: "127.0.0.1", "*", "[::1]"; empty for
    /// any other (one of the network's).
    pub address: &'static str,
    pub pid: u32,
}

/// The sockets listening on `port`, IPv4 and IPv6.
pub fn listening(port: u16) -> std::io::Result<Vec<Socket>> {
    let mut sockets = Vec::new();
    for row in table::<MIB_TCPROW_OWNER_PID>(AF_INET, std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table))? {
        if network_port(row.dwLocalPort) == port {
            let address = match row.dwLocalAddr.to_ne_bytes() {
                [127, 0, 0, 1] => "127.0.0.1",
                [0, 0, 0, 0] => "*",
                _ => "",
            };
            sockets.push(Socket { v6: false, address, pid: row.dwOwningPid });
        }
    }
    for row in table::<MIB_TCP6ROW_OWNER_PID>(AF_INET6, std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table))? {
        if network_port(row.dwLocalPort) == port {
            let loopback = std::net::Ipv6Addr::LOCALHOST.octets();
            let address = match row.ucLocalAddr {
                address if address == [0; 16] => "*",
                address if address == loopback => "[::1]",
                _ => "",
            };
            sockets.push(Socket { v6: true, address, pid: row.dwOwningPid });
        }
    }
    Ok(sockets)
}

/// A process holding a socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// This user's: its program's file name ("node.exe") and when it
    /// started (FILETIME units), which tells it from a later process given
    /// the same pid.
    Mine { image: String, started: u64 },
    /// Another account's, or one this user can't look into (the system's).
    Others,
}

/// Whose process `pid` is; None when it's gone.
pub fn owner(pid: u32) -> Option<Owner> {
    // The idle and System processes.
    if pid <= 4 {
        return Some(Owner::Others);
    }
    let process = match open_process(pid) {
        Ok(process) => process,
        // This user can open its own processes: one it can't is another's.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Some(Owner::Others),
        Err(_) => return None,
    };
    let ours = user_sid().ok()?;
    if user_sid_of(&process).ok()? != ours {
        return Some(Owner::Others);
    }
    let path = image_path(&process);
    let image = path.rsplit(['\\', '/']).next().unwrap_or_default().to_owned();
    Some(Owner::Mine { image, started: creation_time(&process).ok()? })
}

/// A port as the tables hold it: in network byte order, in the low 16 bits.
fn network_port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

/// The listening rows of one of Windows' TCP tables: a u32 count, then the
/// rows, starting at `offset`.
fn table<Row: Copy>(family: u16, offset: usize) -> std::io::Result<Vec<Row>> {
    let mut size: u32 = 0;
    for _ in 0..8 {
        // u64s, so the table is aligned as its rows need.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let mut room = (buffer.len() * 8) as u32;
        // SAFETY: `room` bytes to write; it says how many it needs.
        let result = unsafe {
            GetExtendedTcpTable(buffer.as_mut_ptr().cast(), &raw mut room, 0, u32::from(family), TCP_TABLE_OWNER_PID_LISTENER, 0)
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            // Sockets come and go: some room to spare.
            size = room + 16 * size_of::<Row>() as u32;
            continue;
        }
        if result != NO_ERROR {
            return Err(std::io::Error::from_raw_os_error(result as i32));
        }
        let base = buffer.as_ptr().cast::<u8>();
        // SAFETY: the table's count is at its start.
        let count = unsafe { base.cast::<u32>().read() } as usize;
        if offset + count * size_of::<Row>() > buffer.len() * 8 {
            return Err(std::io::Error::other("the TCP table is shorter than it says"));
        }
        // SAFETY: `count` rows after the header, inside the buffer (checked).
        return Ok(unsafe { std::slice::from_raw_parts(base.add(offset).cast::<Row>(), count) }.to_vec());
    }
    Err(std::io::Error::other("the TCP table kept growing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listener_of_this_process_is_found_and_is_its_users() {
        let v4 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = v4.local_addr().unwrap().port();
        let v6 = std::net::TcpListener::bind(("::1", port)).ok();
        let found = listening(port).unwrap();
        let pid = std::process::id();
        assert!(found.contains(&Socket { v6: false, address: "127.0.0.1", pid }), "{found:?}");
        if v6.is_some() {
            assert!(found.contains(&Socket { v6: true, address: "[::1]", pid }), "{found:?}");
        }
        let Some(Owner::Mine { image, started }) = owner(pid) else { panic!("this process is this user's") };
        assert!(image.to_ascii_lowercase().ends_with(".exe") && started > 0, "{image}");
        assert_eq!(owner(4), Some(Owner::Others), "the System process");
        drop(v4);
        assert!(!listening(port).unwrap().iter().any(|socket| socket.pid == pid && !socket.v6));
    }
}
