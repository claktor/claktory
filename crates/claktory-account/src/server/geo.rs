//! Where a Claktory is, roughly: country, region and city from its IP,
//! looked up in an offline GeoIP file (DB-IP's free "IP to City Lite", or
//! MaxMind's GeoLite2 City). Only the place is kept, never the IP.

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A place, in English. Any part can be unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
}

impl Location {
    /// "Amsterdam, North Holland, Netherlands", or None when nothing's known.
    pub fn describe(&self) -> Option<String> {
        let parts: Vec<&str> = [&self.city, &self.region, &self.country].into_iter().flatten().map(String::as_str).collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

impl Location {
    /// "Amsterdam, Netherlands": the city and country, as the account page
    /// shows them.
    pub fn short(&self) -> Option<String> {
        let parts: Vec<&str> = [&self.city, &self.country].into_iter().flatten().map(String::as_str).collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// What finds a place from an IP.
pub trait Locator: Send + Sync {
    fn locate(&self, ip: IpAddr) -> Option<Location>;
    /// Who owns the network an IP is in (its ASN's organisation, such as
    /// "Hetzner Online GmbH"), from an ASN file; None from a city file.
    fn network(&self, _ip: IpAddr) -> Option<String> {
        None
    }
}

/// A GeoIP file in the MaxMind DB format.
pub struct GeoIp(maxminddb::Reader<maxminddb::Mmap>);

impl GeoIp {
    pub fn open(path: &Path) -> Result<GeoIp, String> {
        // SAFETY: the file is mapped, not read; changing it in place while
        // it's mapped would be undefined behaviour. The deploy kit replaces
        // it with a new file (a rename) and restarts the service, so the
        // mapped file never changes.
        let reader = unsafe { maxminddb::Reader::open_mmap(path) }.map_err(|error| format!("can't open {}: {error}", path.display()))?;
        Ok(GeoIp(reader))
    }
}

impl Locator for GeoIp {
    fn locate(&self, ip: IpAddr) -> Option<Location> {
        let found = self.0.lookup(ip).ok()?;
        let city: maxminddb::geoip2::City = found.decode().ok()??;
        let owned = |text: Option<&str>| text.filter(|text| !text.is_empty()).map(str::to_owned);
        let location = Location {
            country: owned(city.country.names.english),
            region: owned(city.subdivisions.first().and_then(|region| region.names.english)),
            city: owned(city.city.names.english),
        };
        location.describe().map(|_| location)
    }

    fn network(&self, ip: IpAddr) -> Option<String> {
        let found = self.0.lookup(ip).ok()?;
        let asn: maxminddb::geoip2::Asn = found.decode().ok()??;
        asn.autonomous_system_organization.filter(|owner| !owner.is_empty()).map(str::to_owned)
    }
}

/// Whether an IP is one a place can be found for: not private, loopback
/// or the like.
pub fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()),
        IpAddr::V6(ip) => {
            !(ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}
