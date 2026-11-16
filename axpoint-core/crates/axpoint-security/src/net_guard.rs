//! Outbound HTTP/HTTPS URL & DNS SSRF guard (`07-STEP07-08` §07.0 & Step 01 §01.1).

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::OutboundUrlGuardPort;

/// Object-safe asynchronous DNS host resolver consumed by [`NetGuard`].
pub trait HostResolver: Send + Sync {
    /// Resolves `(host, port)` into one or more socket addresses.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Forbidden`] (`PERMISSION_DENIED`) when resolution fails or yields no
    /// addresses so outbound SSRF probes cannot distinguish unresolvable internal names from
    /// blocked network ranges.
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Vec<SocketAddr>>;
}

/// Production DNS resolver backed by [`tokio::net::lookup_host`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioDnsResolver;

impl TokioDnsResolver {
    /// Constructs a [`TokioDnsResolver`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl HostResolver for TokioDnsResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Vec<SocketAddr>> {
        Box::pin(async move {
            let iter = tokio::net::lookup_host((host, port)).await.map_err(|err| {
                tracing::warn!(
                    host = %host,
                    port = port,
                    error = %err,
                    "outbound DNS resolution failed"
                );
                AppError::Forbidden("PERMISSION_DENIED")
            })?;
            let addrs: Vec<SocketAddr> = iter.collect();
            if addrs.is_empty() {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
            Ok(addrs)
        })
    }
}

/// Deterministic in-memory DNS resolver for offline unit tests.
#[derive(Debug, Clone, Default)]
pub struct StaticDnsResolver {
    hosts: HashMap<String, Vec<IpAddr>>,
}

impl StaticDnsResolver {
    /// Constructs an empty [`StaticDnsResolver`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs a [`StaticDnsResolver`] pre-populated with `(hostname, ips)` mappings.
    #[must_use]
    pub fn from_hosts<I, K, V>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: IntoIterator<Item = IpAddr>,
    {
        let mut resolver = Self::new();
        for (host, ips) in entries {
            resolver.insert(host.as_ref(), ips);
        }
        resolver
    }

    /// Inserts or replaces the resolved IP list for `host` and returns `self`.
    #[must_use]
    pub fn with_host<I>(mut self, host: &str, ips: I) -> Self
    where
        I: IntoIterator<Item = IpAddr>,
    {
        self.insert(host, ips);
        self
    }

    /// Inserts or replaces the resolved IP list for `host`.
    pub fn insert<I>(&mut self, host: &str, ips: I)
    where
        I: IntoIterator<Item = IpAddr>,
    {
        let key = normalize_domain(host);
        self.hosts.insert(key, ips.into_iter().collect());
    }
}

impl HostResolver for StaticDnsResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Vec<SocketAddr>> {
        let key = normalize_domain(host);
        let result = match self.hosts.get(&key) {
            Some(ips) if !ips.is_empty() => Ok(ips
                .iter()
                .copied()
                .map(|ip| SocketAddr::new(ip, port))
                .collect()),
            _ => Err(AppError::Forbidden("PERMISSION_DENIED")),
        };
        Box::pin(async move { result })
    }
}

/// Outbound HTTP/HTTPS SSRF guard enforcing URL syntax, hostname blocklists, and DNS IP policy.
///
/// # Examples
///
/// ```
/// use std::net::IpAddr;
/// use axpoint_security::net_guard::NetGuard;
///
/// let pub_ip: IpAddr = "93.184.216.34".parse()?;
/// NetGuard::check_ip_allowed(pub_ip, false)?;
///
/// let imds_ip: IpAddr = "169.254.169.254".parse()?;
/// assert!(NetGuard::check_ip_allowed(imds_ip, true).is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct NetGuard {
    resolver: Arc<dyn HostResolver>,
}

impl fmt::Debug for NetGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetGuard")
            .field("resolver", &"<dyn HostResolver>")
            .finish()
    }
}

impl Default for NetGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl NetGuard {
    /// Constructs a [`NetGuard`] using [`TokioDnsResolver`].
    #[must_use]
    pub fn new() -> Self {
        let resolver: Arc<dyn HostResolver> = Arc::new(TokioDnsResolver);
        Self { resolver }
    }

    /// Constructs a [`NetGuard`] using a custom [`HostResolver`].
    #[must_use]
    pub fn with_resolver(resolver: Arc<dyn HostResolver>) -> Self {
        Self { resolver }
    }

    /// Validates whether `ip` is permitted for outbound connections.
    ///
    /// Delegates to [`check_ip_allowed`].
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Forbidden`] (`PERMISSION_DENIED`) when `ip` is in a blocked range.
    pub fn check_ip_allowed(ip: IpAddr, allow_private_network: bool) -> Result<(), AppError> {
        check_ip_allowed(ip, allow_private_network)
    }

    /// Validates URL scheme, embedded credentials, literal IP hosts, and blocked domain names.
    ///
    /// Delegates to [`check_host_literal_and_name`].
    ///
    /// # Errors
    ///
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) on invalid scheme, credentials, or
    /// missing host/port, and [`AppError::Forbidden`] (`PERMISSION_DENIED`) on blocked hosts/IPs.
    pub fn check_host_literal_and_name(
        url: &reqwest::Url,
        allow_private_network: bool,
    ) -> Result<(String, u16), AppError> {
        check_host_literal_and_name(url, allow_private_network)
    }

    /// Parses `raw_url`, checks scheme/host rules, resolves DNS for domain hosts, and validates
    /// every resolved IP address against [`check_ip_allowed`].
    ///
    /// # Errors
    ///
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `raw_url` is malformed, uses a
    /// non-`http`/`https` scheme, or embeds credentials, and [`AppError::Forbidden`]
    /// (`PERMISSION_DENIED`) if the host or any resolved IP address is disallowed.
    pub async fn validate_url(
        &self,
        raw_url: &str,
        allow_private_network: bool,
    ) -> Result<reqwest::Url, AppError> {
        let parsed = reqwest::Url::parse(raw_url).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid outbound URL: {err}"))
        })?;
        let (host, port) = check_host_literal_and_name(&parsed, allow_private_network)?;

        if host.parse::<IpAddr>().is_err() {
            let addrs = self.resolver.resolve(&host, port).await?;
            if addrs.is_empty() {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
            for addr in addrs {
                check_ip_allowed(addr.ip(), allow_private_network)?;
            }
        }

        Ok(parsed)
    }

    /// Builds a [`reqwest::redirect::Policy`] enforcing `max_hops` and SSRF URL checks on each
    /// redirect target.
    ///
    /// Delegates to [`safe_redirect_policy`].
    #[must_use]
    pub fn safe_redirect_policy(
        allow_private_network: bool,
        max_hops: usize,
    ) -> reqwest::redirect::Policy {
        safe_redirect_policy(allow_private_network, max_hops)
    }
}

impl OutboundUrlGuardPort for NetGuard {
    fn check_outbound_url<'a>(
        &'a self,
        url: &'a str,
        allow_private_network: bool,
    ) -> BoxFuture<'a, String> {
        Box::pin(async move {
            let validated = self.validate_url(url, allow_private_network).await?;
            Ok(validated.to_string())
        })
    }
}

/// Validates whether an outbound target [`IpAddr`] is permitted under SSRF policy.
///
/// Link-local / cloud metadata (`169.254.0.0/16`, `fe80::/10`), unspecified (`0.0.0.0`, `::`),
/// broadcast (`255.255.255.255`), and multicast addresses are **always** blocked, even when
/// `allow_private_network` is `true`. IPv4-mapped IPv6 (`::ffff:0:0/96`) and RFC 6052 NAT64
/// (`64:ff9b::/96`) addresses are unwrapped and evaluated as IPv4.
///
/// # Errors
///
/// Returns [`AppError::Forbidden`] (`PERMISSION_DENIED`) when `ip` falls within a forbidden range.
pub fn check_ip_allowed(ip: IpAddr, allow_private_network: bool) -> Result<(), AppError> {
    match ip {
        IpAddr::V4(v4) => check_ipv4_allowed(v4, allow_private_network),
        IpAddr::V6(v6) => {
            if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                return check_ipv4_allowed(mapped_v4, allow_private_network);
            }
            if let Some(nat64_v4) = extract_nat64_ipv4(v6) {
                return check_ipv4_allowed(nat64_v4, allow_private_network);
            }
            check_ipv6_allowed(v6, allow_private_network)
        }
    }
}

/// Validates URL scheme (`http`/`https`), credentials absence, blocked hostnames, and literal IPs,
/// returning `(normalized_host, port)`.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when the URL scheme is not `http`/`https`,
/// credentials are embedded, or host/port is absent; returns [`AppError::Forbidden`]
/// (`PERMISSION_DENIED`) when the hostname or IP literal violates SSRF rules.
pub fn check_host_literal_and_name(
    url: &reqwest::Url,
    allow_private_network: bool,
) -> Result<(String, u16), AppError> {
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "outbound URL scheme must be http or https",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "outbound URL must not embed credentials",
        ));
    }

    let raw_host = url
        .host_str()
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "outbound URL is missing host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "outbound URL is missing port"))?;

    if let Some(domain) = url.domain() {
        let normalized = normalize_domain(domain);
        if normalized.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "outbound URL host is empty",
            ));
        }
        if let Ok(ip) = normalized.parse::<IpAddr>() {
            check_ip_allowed(ip, allow_private_network)?;
            return Ok((ip.to_string(), port));
        }
        check_domain_name_allowed(&normalized, allow_private_network)?;
        return Ok((normalized, port));
    }

    let unbracketed = raw_host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw_host);
    let ip = unbracketed
        .parse::<IpAddr>()
        .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid IP literal in outbound URL"))?;
    check_ip_allowed(ip, allow_private_network)?;
    Ok((ip.to_string(), port))
}

/// Builds a [`reqwest::redirect::Policy`] enforcing a maximum hop count and SSRF target checks on
/// every HTTP redirect.
///
/// Note: in `reqwest`, `attempt.previous()` includes the initial request URL, so its length equals
/// the 1-based redirect hop count (matching [`reqwest::redirect::Policy::limited`]).
#[must_use]
pub fn safe_redirect_policy(
    allow_private_network: bool,
    max_hops: usize,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > max_hops {
            return attempt.error("too many redirects");
        }
        if let Err(err) = check_host_literal_and_name(attempt.url(), allow_private_network) {
            return attempt.error(err.to_string());
        }
        attempt.follow()
    })
}

fn normalize_domain(raw: &str) -> String {
    raw.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn check_domain_name_allowed(
    normalized_domain: &str,
    allow_private_network: bool,
) -> Result<(), AppError> {
    if normalized_domain == "metadata.google.internal" || normalized_domain == "metadata" {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    if !allow_private_network {
        let has_dot = normalized_domain.contains('.');
        let last_label = normalized_domain.rsplit('.').next();
        if normalized_domain == "localhost"
            || (has_dot && matches!(last_label, Some("localhost" | "local" | "internal")))
        {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
    }
    Ok(())
}

fn check_ipv4_allowed(v4: Ipv4Addr, allow_private_network: bool) -> Result<(), AppError> {
    if v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    if !allow_private_network && is_private_or_reserved_ipv4(v4) {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    Ok(())
}

const fn is_private_or_reserved_ipv4(v4: Ipv4Addr) -> bool {
    let [o0, o1, o2, _o3] = v4.octets();
    let is_cgnat = o0 == 100 && (o1 & 0xc0) == 64;
    let is_this_net = o0 == 0;
    let is_ietf_protocol = o0 == 192 && o1 == 0 && o2 == 0;
    let is_doc_net_1 = o0 == 192 && o1 == 0 && o2 == 2;
    let is_benchmarking = o0 == 198 && (o1 & 0xfe) == 18;
    let is_doc_net_2 = o0 == 198 && o1 == 51 && o2 == 100;
    let is_doc_net_3 = o0 == 203 && o1 == 0 && o2 == 113;
    let is_reserved_class_e = (o0 & 0xf0) == 240;

    v4.is_loopback()
        || v4.is_private()
        || is_cgnat
        || is_this_net
        || is_ietf_protocol
        || is_doc_net_1
        || is_benchmarking
        || is_doc_net_2
        || is_doc_net_3
        || is_reserved_class_e
}

fn check_ipv6_allowed(v6: Ipv6Addr, allow_private_network: bool) -> Result<(), AppError> {
    let [s0, s1, ..] = v6.segments();
    let is_link_local = (s0 & 0xffc0) == 0xfe80;
    if is_link_local || v6.is_unspecified() || v6.is_multicast() {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    if !allow_private_network {
        let is_unique_local = (s0 & 0xfe00) == 0xfc00;
        let is_site_local = (s0 & 0xffc0) == 0xfec0;
        let is_documentation = s0 == 0x2001 && s1 == 0x0db8;
        if v6.is_loopback() || is_unique_local || is_site_local || is_documentation {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
    }
    Ok(())
}

const fn extract_nat64_ipv4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = v6.segments();
    if s0 == 0x0064 && s1 == 0xff9b && s2 == 0 && s3 == 0 && s4 == 0 && s5 == 0 {
        let [b0, b1] = s6.to_be_bytes();
        let [b2, b3] = s7.to_be_bytes();
        Some(Ipv4Addr::new(b0, b1, b2, b3))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::Arc;

    use axpoint_domain::AppError;
    use axpoint_domain::ports::OutboundUrlGuardPort;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{
        HostResolver, NetGuard, StaticDnsResolver, TokioDnsResolver, check_host_literal_and_name,
        check_ip_allowed, safe_redirect_policy,
    };

    fn assert_permission_denied<T: std::fmt::Debug>(res: Result<T, AppError>) {
        let err = res.err();
        assert!(
            matches!(err, Some(AppError::Forbidden("PERMISSION_DENIED"))),
            "expected Forbidden(PERMISSION_DENIED), got {err:?}"
        );
    }

    fn assert_invalid_argument<T: std::fmt::Debug>(res: Result<T, AppError>) {
        let err = res.err();
        assert!(
            matches!(
                err,
                Some(AppError::BadRequest {
                    code: "INVALID_ARGUMENT",
                    ..
                })
            ),
            "expected BadRequest(INVALID_ARGUMENT), got {err:?}"
        );
    }

    #[tokio::test]
    async fn allows_public_ipv4_and_ipv6_literals_and_domains() -> Result<(), Box<dyn Error>> {
        let pub_v4: IpAddr = "93.184.216.34".parse()?;
        let pub_v6: IpAddr = "2606:2800:220:1:248:1893:25c8:1946".parse()?;
        check_ip_allowed(pub_v4, false)?;
        check_ip_allowed(pub_v6, false)?;

        let resolver = StaticDnsResolver::from_hosts([("example.com", vec![pub_v4, pub_v6])]);
        let resolver_arc: Arc<dyn HostResolver> = Arc::new(resolver);
        let guard = NetGuard::with_resolver(resolver_arc);

        let parsed_v4 = guard
            .validate_url("https://93.184.216.34/api/v1", false)
            .await?;
        assert_eq!(parsed_v4.host_str(), Some("93.184.216.34"));

        let parsed_v6 = guard
            .validate_url(
                "https://[2606:2800:220:1:248:1893:25c8:1946]:8443/ok",
                false,
            )
            .await?;
        assert_eq!(parsed_v6.port(), Some(8443));

        let parsed_dom = guard
            .validate_url("https://Example.COM./feed", false)
            .await?;
        assert_eq!(parsed_dom.scheme(), "https");
        Ok(())
    }

    #[tokio::test]
    async fn rejects_loopback_private_cgnat_and_ula_when_private_disallowed()
    -> Result<(), Box<dyn Error>> {
        let guard = NetGuard::with_resolver(Arc::new(StaticDnsResolver::new()));

        let blocked_urls = [
            "http://localhost:8080/health",
            "http://sub.localhost/api",
            "http://printer.local/ipp",
            "http://api.internal/v1",
            "http://127.0.0.1:8001/health",
            "http://[::1]:8001/health",
            "http://[::ffff:127.0.0.1]/admin",
            "http://0x7f000001/hex-loopback",
            "http://2130706433/int-loopback",
            "http://10.1.2.3/internal",
            "http://172.16.0.1/internal",
            "http://172.31.255.254/internal",
            "http://192.168.1.1/router",
            "http://100.64.0.1/cgnat",
            "http://100.100.100.200/latest/meta-data",
            "http://[fc00::1]/ula",
            "http://[fd12::1]/ula",
            "http://[fec0::1]/site-local",
        ];

        for raw in blocked_urls {
            assert_permission_denied(guard.validate_url(raw, false).await);
        }
        Ok(())
    }

    #[test]
    fn enforces_rfc1918_172_16_slash_12_exact_boundaries() -> Result<(), Box<dyn Error>> {
        let below: IpAddr = "172.15.255.255".parse()?;
        let start: IpAddr = "172.16.0.1".parse()?;
        let end: IpAddr = "172.31.255.254".parse()?;
        let above: IpAddr = "172.32.0.1".parse()?;

        check_ip_allowed(below, false)?;
        assert_permission_denied(check_ip_allowed(start, false));
        assert_permission_denied(check_ip_allowed(end, false));
        check_ip_allowed(above, false)?;

        let cgnat_below: IpAddr = "100.63.255.255".parse()?;
        let cgnat_above: IpAddr = "100.128.0.1".parse()?;
        check_ip_allowed(cgnat_below, false)?;
        check_ip_allowed(cgnat_above, false)?;
        Ok(())
    }

    #[test]
    fn rejects_reserved_ietf_documentation_and_benchmarking_ipv4() -> Result<(), Box<dyn Error>> {
        let reserved_ips = [
            "0.0.0.0",
            "0.1.2.3",
            "192.0.0.1",
            "192.0.2.10",
            "198.18.0.1",
            "198.19.255.254",
            "198.51.100.5",
            "203.0.113.9",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ];
        for s in reserved_ips {
            let ip: IpAddr = s.parse()?;
            assert_permission_denied(check_ip_allowed(ip, false));
        }
        Ok(())
    }

    #[tokio::test]
    async fn allows_private_and_loopback_when_allow_private_network_is_true()
    -> Result<(), Box<dyn Error>> {
        let resolver = StaticDnsResolver::new()
            .with_host("localhost", [IpAddr::V4(Ipv4Addr::LOCALHOST)])
            .with_host("webhook.internal", [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))]);
        let guard = NetGuard::with_resolver(Arc::new(resolver));

        let hook = guard
            .validate_url("http://127.0.0.1:18900/hook", true)
            .await?;
        assert_eq!(hook.port(), Some(18900));

        let priv_api = guard.validate_url("http://10.0.0.5:8080/api", true).await?;
        assert_eq!(priv_api.port(), Some(8080));

        guard
            .validate_url("http://localhost:18900/hook", true)
            .await?;
        guard
            .validate_url("http://webhook.internal:8080/events", true)
            .await?;
        guard.validate_url("http://[::1]:18900/hook", true).await?;
        guard
            .validate_url("http://[fd12::1]:8080/api", true)
            .await?;
        guard
            .validate_url("http://100.64.0.10:8080/api", true)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn blocks_imds_link_local_and_metadata_even_when_private_allowed()
    -> Result<(), Box<dyn Error>> {
        let resolver = StaticDnsResolver::new()
            .with_host(
                "metadata.google.internal",
                [IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))],
            )
            .with_host("metadata", [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))])
            .with_host(
                "sneaky.internal",
                [IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))],
            );
        let guard = NetGuard::with_resolver(Arc::new(resolver));

        let always_blocked = [
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.170.2/v2/credentials",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            "http://[64:ff9b::a9fe:a9fe]/latest/meta-data/",
            "http://[fe80::1]/",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://metadata.google.internal./computeMetadata/v1/",
            "http://metadata/computeMetadata/v1/",
            "http://sneaky.internal/latest",
            "http://0.0.0.0/",
            "http://[::]/",
            "http://255.255.255.255/",
            "http://224.0.0.1/",
            "http://[ff02::1]/",
        ];

        for raw in always_blocked {
            assert_permission_denied(guard.validate_url(raw, true).await);
            assert_permission_denied(guard.validate_url(raw, false).await);
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_dual_record_dns_when_any_address_is_forbidden() -> Result<(), Box<dyn Error>> {
        let pub_ip: IpAddr = "93.184.216.34".parse()?;
        let lo_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let imds_ip = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));

        let resolver = StaticDnsResolver::new()
            .with_host("rebind.example.com", [pub_ip, lo_ip])
            .with_host("imds-rebind.example.com", [pub_ip, imds_ip]);
        let guard = NetGuard::with_resolver(Arc::new(resolver));

        assert_permission_denied(
            guard
                .validate_url("https://rebind.example.com/hook", false)
                .await,
        );
        // Private allowed permits [pub_ip, 127.0.0.1]
        guard
            .validate_url("https://rebind.example.com/hook", true)
            .await?;
        // IMDS in dual-record is STILL rejected even with allow_private_network = true
        assert_permission_denied(
            guard
                .validate_url("https://imds-rebind.example.com/hook", true)
                .await,
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unresolvable_or_empty_dns_with_permission_denied() {
        let mut resolver = StaticDnsResolver::new();
        resolver.insert("empty.example.com", std::iter::empty());
        let guard = NetGuard::with_resolver(Arc::new(resolver));

        assert_permission_denied(
            guard
                .validate_url("https://unknown.example.com/", false)
                .await,
        );
        assert_permission_denied(
            guard
                .validate_url("https://empty.example.com/", false)
                .await,
        );
    }

    #[tokio::test]
    async fn rejects_non_http_schemes_credentials_and_malformed_urls() {
        let guard = NetGuard::default();

        let invalid_urls = [
            "file:///etc/passwd",
            "ftp://example.com/pub/file.txt",
            "gopher://127.0.0.1:6379/_INFO",
            "javascript:alert(1)",
            "data:text/plain;base64,SGVsbG8=",
            "https://user:pass@example.com/secret",
            "https://user@example.com/secret",
            "https://:pass@example.com/secret",
            "not a valid url",
            "://missing-scheme",
        ];

        for raw in invalid_urls {
            assert_invalid_argument(guard.validate_url(raw, false).await);
            assert_invalid_argument(guard.validate_url(raw, true).await);
        }
    }

    #[tokio::test]
    async fn outbound_url_guard_port_trait_object_validates_and_formats()
    -> Result<(), Box<dyn Error>> {
        let pub_ip: IpAddr = "93.184.216.34".parse()?;
        let resolver = StaticDnsResolver::new().with_host("api.example.com", [pub_ip]);
        let port: Arc<dyn OutboundUrlGuardPort> =
            Arc::new(NetGuard::with_resolver(Arc::new(resolver)));

        let checked = port
            .check_outbound_url("https://api.example.com/v1/items?limit=10", false)
            .await?;
        assert_eq!(checked, "https://api.example.com/v1/items?limit=10");

        assert_permission_denied(
            port.check_outbound_url("http://127.0.0.1:9995/api", false)
                .await,
        );
        Ok(())
    }

    #[test]
    fn net_guard_debug_and_tokio_resolver_constructors_work() -> Result<(), Box<dyn Error>> {
        let resolver = TokioDnsResolver::new();
        assert!(format!("{resolver:?}").contains("TokioDnsResolver"));

        let guard = NetGuard::new();
        assert!(format!("{guard:?}").contains("NetGuard"));

        let parsed = reqwest::Url::parse("https://93.184.216.34:8443/test")?;
        let (host, port) = check_host_literal_and_name(&parsed, false)?;
        assert_eq!(host, "93.184.216.34");
        assert_eq!(port, 8443);

        let v6_loopback = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_permission_denied(NetGuard::check_ip_allowed(v6_loopback, false));
        NetGuard::check_ip_allowed(v6_loopback, true)?;
        Ok(())
    }

    #[tokio::test]
    async fn safe_redirect_policy_blocks_ssrf_redirect_and_enforces_max_hops()
    -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            for _ in 0..16_u8 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0_u8; 1024];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[]));
                let resp = if req.starts_with("GET /to-imds ") {
                    String::from(
                        "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                } else if req.starts_with("GET /hop1 ") {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{addr}/hop2\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else if req.starts_with("GET /hop2 ") {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{addr}/ok\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    String::from(
                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                };
                if let Err(err) = stream.write_all(resp.as_bytes()).await {
                    tracing::warn!(error = %err, "mock redirect server write failed");
                }
            }
        });

        let client_one_hop = reqwest::Client::builder()
            .no_proxy()
            .redirect(safe_redirect_policy(true, 1))
            .build()?;

        // Redirect to cloud IMDS must be rejected even with allow_private_network = true
        let imds_err = client_one_hop
            .get(format!("http://{addr}/to-imds"))
            .send()
            .await
            .err();
        assert!(imds_err.as_ref().is_some_and(reqwest::Error::is_redirect));

        // 2-hop chain (/hop1 -> /hop2 -> /ok) must fail under max_hops = 1
        let hop_err = client_one_hop
            .get(format!("http://{addr}/hop1"))
            .send()
            .await
            .err();
        assert!(hop_err.as_ref().is_some_and(reqwest::Error::is_redirect));

        // 1-hop chain (/hop2 -> /ok) succeeds under max_hops = 1 when private network is allowed
        let ok_res = NetGuard::safe_redirect_policy(true, 1);
        let client_ok = reqwest::Client::builder()
            .no_proxy()
            .redirect(ok_res)
            .build()?;
        let resp = client_ok.get(format!("http://{addr}/hop2")).send().await?;
        assert_eq!(resp.status().as_u16(), 200);

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn safe_redirect_policy_blocks_private_redirect_when_private_disallowed()
    -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0_u8; 512];
                let _ = stream.read(&mut buf).await.unwrap_or(0);
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{addr}/internal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                if let Err(err) = stream.write_all(resp.as_bytes()).await {
                    tracing::warn!(error = %err, "mock redirect server write failed");
                }
            }
        });

        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(safe_redirect_policy(false, 5))
            .build()?;
        let redirect_err = client
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .err();
        assert!(
            redirect_err
                .as_ref()
                .is_some_and(reqwest::Error::is_redirect)
        );

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn tokio_dns_resolver_resolves_numeric_socket_and_rejects_invalid_host()
    -> Result<(), Box<dyn Error>> {
        let resolver = TokioDnsResolver::new();
        let addrs = resolver.resolve("127.0.0.1", 8080).await?;
        assert_eq!(
            addrs.first().map(std::net::SocketAddr::ip),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );

        assert_permission_denied(resolver.resolve("invalid host spaces", 80).await);
        Ok(())
    }
}
