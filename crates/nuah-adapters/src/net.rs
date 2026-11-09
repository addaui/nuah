// SPDX-License-Identifier: Apache-2.0
//! The `net` Port's Adapter for the public web ([ADR 0040], rules from [ADR 0026]): http and https on the
//! standard ports only, no credentials in the address, and a connection is made only to public addresses,
//! checked on the address actually used, so a redirect or a changed DNS answer is checked when it happens.
//!
//! [ADR 0040]: https://docs.addaui.com/nuah/adr/0040-outbound-operations-run-outside-the-transaction-and-reach-the-network-through-a-net-port/
//! [ADR 0026]: https://docs.addaui.com/nuah/adr/0026-files-follow-the-pages-that-use-them-and-link-previews-are-fetched-by-a-guarded-provider/

use nuah_kernel::*;
use serde_json::json;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use ureq::ResponseExt;

/// An address nothing here may connect to: this machine, the private network, link-local (cloud metadata lives
/// at 169.254.169.254), carrier-grade NAT, multicast, or an IPv4 address dressed as IPv6.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => public_v4(a),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let s = a.segments();
            !(a.is_unspecified()
                || a.is_loopback()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || s[0] == 0x2001 && s[1] == 0x0db8 // documentation
                || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64 can reach v4 private hosts
                || a == Ipv6Addr::LOCALHOST)
        }
    }
}

fn public_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_multicast()
        || a.is_broadcast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64/10 carrier-grade NAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18/15 benchmarking
        || o[0] >= 240)
}

fn refused() -> Error {
    // Always the same answer, so it says nothing about what is inside the network.
    Error::new(Category::Invalid, "net.refused", "That address cannot be reached")
}

/// The address must be a plain public web address: http or https, no credentials, port 80 or 443.
fn check_url(raw: &str, allow_private: bool) -> Result<url::Url> {
    let u = url::Url::parse(raw.trim()).map_err(|_| refused())?;
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some() || u.host_str().is_none() {
        return Err(refused());
    }
    if !allow_private {
        if !matches!(u.port_or_known_default(), Some(80) | Some(443)) {
            return Err(refused());
        }
        match u.host() {
            Some(url::Host::Ipv4(a)) if !is_public(IpAddr::V4(a)) => return Err(refused()),
            Some(url::Host::Ipv6(a)) if !is_public(IpAddr::V6(a)) => return Err(refused()),
            Some(url::Host::Domain(d)) if d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local") || d.ends_with(".internal") => return Err(refused()),
            _ => {}
        }
    }
    Ok(u)
}

/// Resolves names like the system does, then keeps only public addresses. The client connects with what this returns.
#[derive(Debug)]
struct PublicOnly {
    inner: ureq::unversioned::resolver::DefaultResolver,
    allow_private: bool,
}

impl ureq::unversioned::resolver::Resolver for PublicOnly {
    fn resolve(&self, uri: &ureq::http::Uri, config: &ureq::config::Config, timeout: ureq::unversioned::transport::NextTimeout) -> std::result::Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let found = self.inner.resolve(uri, config, timeout)?;
        if self.allow_private {
            return Ok(found);
        }
        if !matches!(uri.port_u16().or(match uri.scheme_str() { Some("https") => Some(443), Some("http") => Some(80), _ => None }), Some(80) | Some(443)) {
            return Err(ureq::Error::HostNotFound);
        }
        let mut out = self.empty();
        for a in found.iter().filter(|a: &&SocketAddr| is_public(a.ip())) {
            out.push(*a);
        }
        if out.is_empty() {
            Err(ureq::Error::HostNotFound)
        } else {
            Ok(out)
        }
    }
}

struct Public {
    allow_private: bool,
}

impl Net for Public {
    fn name(&self) -> &str {
        "net-public"
    }
    fn fetch(&self, req: &NetRequest) -> Result<NetResponse> {
        let url = check_url(&req.url, self.allow_private)?;
        let config = ureq::config::Config::builder()
            .proxy(None)
            .max_redirects(3)
            .timeout_global(Some(Duration::from_millis(req.timeout_ms.max(1))))
            .http_status_as_error(false)
            .user_agent("Mozilla/5.0 (compatible; Nuah/0.8)")
            .build();
        let resolver = PublicOnly { inner: Default::default(), allow_private: self.allow_private };
        let agent = ureq::Agent::with_parts(config, ureq::unversioned::transport::DefaultConnector::new(), resolver);
        let method = ureq::http::Method::from_bytes(req.method.to_uppercase().as_bytes()).map_err(|_| Error::invalid("method", "is not an HTTP method"))?;
        let mut builder = ureq::http::Request::builder().method(method).uri(url.as_str());
        for (n, v) in &req.headers {
            builder = builder.header(n.as_str(), v.as_str());
        }
        let request = builder.body(req.body.clone().unwrap_or_default()).map_err(|e| Error::invalid("request", &e.to_string()))?;
        let resp = agent.run(request).map_err(|_| Error::new(Category::Unsupported, "net.unavailable", "The request could not be completed"))?;
        let status = resp.status().as_u16();
        let final_url = resp.get_uri().to_string();
        let headers: Vec<(String, String)> = resp.headers().iter().filter_map(|(n, v)| v.to_str().ok().map(|v| (n.as_str().to_lowercase(), v.to_string()))).collect();
        let mut body = vec![];
        resp.into_body().into_reader().take(req.max_bytes as u64 + 1).read_to_end(&mut body).map_err(|_| Error::new(Category::Unsupported, "net.unavailable", "The request could not be completed"))?;
        let truncated = body.len() > req.max_bytes;
        body.truncate(req.max_bytes);
        Ok(NetResponse { status, url: final_url, headers, body, truncated })
    }
}

/// `net-public`. `allow_private` exists for tests that talk to a server on this machine; it is not a setting.
#[derive(Default)]
pub struct NetPublic {
    pub allow_private: bool,
}

impl Plugin for NetPublic {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "net-public", "version": "0.1.0", "name": "Public web access",
            "description": "Outbound http and https to public addresses only: no private network, no credentials in the address, standard ports, at most 3 redirects",
            "fills": "net"
        }))
        .unwrap()
    }
    fn fill(&self, _s: &serde_json::Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Net(Arc::new(Public { allow_private: self.allow_private }))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_special_addresses_are_refused() {
        for bad in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1", "::", "fe80::1", "fc00::1", "fd12::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "64:ff9b::7f00:1"] {
            assert!(!is_public(bad.parse().unwrap()), "{bad} must be refused");
        }
        for good in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "172.32.0.1", "2606:4700:4700::1111"] {
            assert!(is_public(good.parse().unwrap()), "{good} is public");
        }
    }

    #[test]
    fn only_plain_public_web_addresses_pass() {
        assert!(check_url("https://example.com/a?b=1", false).is_ok());
        assert!(check_url("http://example.com:80/", false).is_ok());
        for bad in ["ftp://example.com", "file:///etc/passwd", "javascript:alert(1)", "https://user:pw@example.com", "https://user@example.com", "http://example.com:8080/", "http://127.0.0.1/", "http://[::1]/", "http://localhost/", "http://a.localhost/", "http://169.254.169.254/latest/meta-data", "http://printer.local/", "nonsense", "", "https://"] {
            assert!(check_url(bad, false).is_err(), "{bad} must be refused");
        }
        assert!(check_url("http://127.0.0.1:8080/", true).is_ok(), "tests may reach a local server");
    }

    #[test]
    fn the_client_itself_refuses_a_private_address_even_when_the_address_check_is_bypassed() {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut s = s;
                let mut b = [0u8; 512];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        // the resolver is the second line of defence, the one a redirect meets: test it without the first
        let agent = |allow| {
            let config = ureq::config::Config::builder().proxy(None).build();
            ureq::Agent::with_parts(config, ureq::unversioned::transport::DefaultConnector::new(), PublicOnly { inner: Default::default(), allow_private: allow })
        };
        assert!(agent(false).get(&url).call().is_err(), "the resolver filters loopback");
        assert!(agent(true).get(&url).call().is_ok());
        let r = Public { allow_private: true }.fetch(&NetRequest::get(&url)).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, b"ok".as_slice()));
        let cut = Public { allow_private: true }.fetch(&NetRequest { max_bytes: 1, ..NetRequest::get(&url) }).unwrap();
        assert!(cut.truncated && cut.body == b"o");
    }
}
