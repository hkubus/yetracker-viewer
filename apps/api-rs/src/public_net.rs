//! Keeps outbound fetches on the public internet. Download links come from a
//! third-party catalog and the hosts they point at can redirect anywhere, so
//! without this a hostile link or redirect could make the server request
//! loopback, private-network or cloud-metadata addresses (SSRF).
//!
//! Two layers, because reqwest never resolves IP-literal URLs: a DNS resolver
//! that drops non-public addresses ([`PublicResolver`]), and a redirect check
//! that refuses non-public literals and `localhost` ([`is_public_url`]).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::{Host, Url};

/// Whether `address` is a globally routable unicast address.
pub fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || a == 0 // "this network"
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT, 100.64.0.0/10
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (b == 18 || b == 19)) // benchmarking, 198.18.0.0/15
        || a >= 240) // reserved
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(v4) = address.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    let segments = address.segments();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00 // unique local, fc00::/7
        || (segments[0] & 0xffc0) == 0xfe80 // link-local, fe80::/10
        || (segments[0] & 0xffc0) == 0xfec0 // site-local (deprecated), fec0::/10
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
        || (segments[0] == 0x0064 && segments[1] == 0xff9b) // NAT64, embeds IPv4
        || segments[..6] == [0; 6]) // IPv4-compatible (deprecated)
}

/// Whether `url` may be requested: http(s), and not an IP literal outside the
/// public internet or a `localhost` name. Hostnames are checked again when
/// they resolve (see [`PublicResolver`]).
pub fn is_public_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(Host::Ipv4(v4)) => is_public_ipv4(v4),
        Some(Host::Ipv6(v6)) => is_public_ipv6(v6),
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain != "localhost" && !domain.ends_with(".localhost")
        }
        None => false,
    }
}

/// The system resolver, minus every non-public address. A name that only
/// resolves to such addresses fails to resolve.
#[derive(Debug, Default, Clone, Copy)]
pub struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = resolved
                .filter(|address| is_public_ip(address.ip()))
                .collect();
            if public.is_empty() {
                return Err(format!("{host} does not resolve to a public address").into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn only_global_addresses_are_public() {
        for public in [
            "1.1.1.1",
            "93.184.215.14",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(ip(public)), "{public}");
        }
        for internal in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "::7f00:1",
        ] {
            assert!(!is_public_ip(ip(internal)), "{internal}");
        }
    }

    #[test]
    fn redirect_targets_must_be_public() {
        for allowed in [
            "https://api.pillows.su/api/download/x",
            "http://cdn.example.com/a.mp3",
            "https://1.1.1.1/a",
        ] {
            assert!(is_public_url(&Url::parse(allowed).unwrap()), "{allowed}");
        }
        for refused in [
            "http://127.0.0.1:3000/status",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://localhost/",
            "http://LOCALHOST./",
            "http://api.localhost/",
            "http://2130706433/", // 127.0.0.1 as a single number
            "http://0x7f.1/",
            "file:///etc/passwd",
            "ftp://example.com/a",
        ] {
            assert!(!is_public_url(&Url::parse(refused).unwrap()), "{refused}");
        }
    }
}
