use crate::{AccountError, Result, validate_host_input};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAddress {
    pub address: IpAddr,
    pub scope_id: u32,
}
impl ResolvedAddress {
    pub fn numeric(&self) -> String {
        match self.address {
            IpAddr::V6(_) if self.scope_id != 0 => format!("{}%{}", self.address, self.scope_id),
            _ => self.address.to_string(),
        }
    }
}

pub trait EndpointResolver {
    /// Resolve exactly this hostname on port 22. Implementations must not scan.
    fn resolve(&self, host: &str) -> Result<Vec<ResolvedAddress>>;
}

#[derive(Default)]
pub struct SystemResolver;
impl EndpointResolver for SystemResolver {
    fn resolve(&self, host: &str) -> Result<Vec<ResolvedAddress>> {
        let entries = (host, 22)
            .to_socket_addrs()
            .map_err(|_| AccountError::invalid("Cannot resolve the Pi's local address."))?;
        Ok(entries
            .map(|address| ResolvedAddress {
                address: address.ip(),
                scope_id: match address {
                    SocketAddr::V6(value) => value.scope_id(),
                    _ => 0,
                },
            })
            .collect())
    }
}

fn local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [a, b, c, d] = address.octets();
            // Match ipaddress's private/special-use ranges while rejecting loopback,
            // unspecified, multicast, shared (100.64/10), and every global endpoint.
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && (address.is_private()
                    || address.is_link_local()
                    || a == 0
                    || a >= 240
                    || (a == 192 && b == 0 && c == 0 && d != 9 && d != 10)
                    || (a == 192 && b == 0 && c == 2)
                    || (a == 198 && (b == 18 || b == 19))
                    || (a == 198 && b == 51 && c == 100)
                    || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(address) => {
            if address.is_loopback() || address.is_unspecified() || address.is_multicast() {
                return false;
            }
            if let Some(mapped) = address.to_ipv4_mapped() {
                return local(IpAddr::V4(mapped));
            }
            let segments = address.segments();
            address.is_unique_local()
                || address.is_unicast_link_local()
                || (segments[0] == 0x100 && segments[1..4] == [0, 0, 0])
                || (segments[0] == 0x2001 && segments[1] == 0xdb8)
                || (segments[0] == 0x2001
                    && segments[1] < 0x200
                    && !(segments[1] == 1 && [1, 2, 3].contains(&segments[7]))
                    && segments[1] != 3
                    && segments[1] != 4
                    && !(0x20..=0x3f).contains(&segments[1]))
                || (segments[0] == 0x64 && segments[1] == 0xff9b && segments[2] == 1)
        }
    }
}

pub fn local_address(
    host: &str,
    automatic: bool,
    resolver: &dyn EndpointResolver,
) -> Result<String> {
    validate_host_input(host, false)?;
    let entries = resolver.resolve(host).map_err(|_| AccountError::invalid(if automatic {
        "Automatic lookup could not resolve minwinpc. Uncheck Automatic address and enter the Pi's IP address."
    } else { "Cannot resolve the Pi's local address." }))?;
    let mut candidates = Vec::new();
    for entry in entries {
        if !local(entry.address) {
            return Err(AccountError::invalid(
                "IoT account setup is limited to a device on your private local network.",
            ));
        }
        let numeric = entry.numeric();
        if !candidates.contains(&numeric) {
            candidates.push(numeric);
        }
    }
    if candidates.is_empty() {
        return Err(AccountError::invalid(
            "The Pi address has no usable local network address.",
        ));
    }
    if automatic && candidates.len() != 1 {
        return Err(AccountError::invalid(
            "Automatic lookup returned multiple addresses. Uncheck Automatic address and enter the Pi's IP address to identify the intended device.",
        ));
    }
    Ok(candidates.remove(0))
}
