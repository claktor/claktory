//! This computer on Windows: its name and today's date, which the backend
//! reads from gethostname and localtime_r elsewhere.

use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW, GetLocalTime};

/// This computer's host name ("ANAS-LAPTOP"), as gethostname gives it;
/// empty when it can't be read.
pub fn computer_name() -> String {
    let mut buffer = vec![0u16; 256];
    let mut size = buffer.len() as u32;
    // SAFETY: `size` units to write; it says how many it wrote.
    if unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &raw mut size) } == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..size as usize])
}

/// "2026-11-20": today, in this computer's time zone.
pub fn today() -> String {
    // SAFETY: writes one SYSTEMTIME.
    let now = unsafe {
        let mut now = std::mem::zeroed();
        GetLocalTime(&raw mut now);
        now
    };
    format!("{:04}-{:02}-{:02}", now.wYear, now.wMonth, now.wDay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_and_date_read_as_windows_has_them() {
        let name = computer_name();
        assert!(!name.is_empty() && !name.contains('\0'), "{name:?}");
        let expected = std::env::var("COMPUTERNAME").unwrap_or_default();
        assert!(name.eq_ignore_ascii_case(&expected) || expected.is_empty() || name.len() > 15, "{name} / {expected}");
        let today = today();
        assert!(today.len() == 10 && today.starts_with("20") && today.as_bytes()[4] == b'-', "{today}");
    }
}
