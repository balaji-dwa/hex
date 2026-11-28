//! Hex's own HTTPS: model downloads, update checks, and transformation
//! scripts' `fetch`, through one blocking client (ureq) that works on managed
//! networks (#894).
//!
//! - TLS through the platform (`native-tls`: SChannel, Security.framework,
//!   or OpenSSL) and its trusted roots, so a company's own root certificate,
//!   as TLS-inspecting firewalls need, is trusted like it is in a browser.
//! - The proxy the computer uses (`proxy`): `HTTPS_PROXY`, `HTTP_PROXY`,
//!   `ALL_PROXY`, and `NO_PROXY` on every OS, then Windows' proxy settings
//!   (including a PAC file or WPAD) and macOS' network proxy settings.
//! - Failures named for a person: "Couldn't reach huggingface.co: …".

mod download;
mod proxy;
#[cfg(test)]
mod redirect_tests;

use std::time::{Duration, Instant};

use ureq::http::{Response, header::LOCATION};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use url::{Origin, Url};

pub use download::{Download, download};
pub use proxy::proxy_for;

const MAX_REDIRECTS: usize = 10;

/// Request headers that may only be sent to one HTTPS origin.
pub(crate) struct ScopedHeaders<'a> {
    origin: Option<Origin>,
    headers: &'a [(&'a str, &'a str)],
}

impl<'a> ScopedHeaders<'a> {
    pub(crate) fn new(url: &str, headers: &'a [(&'a str, &'a str)]) -> Self {
        Self {
            origin: Url::parse(url)
                .ok()
                .filter(|url| url.scheme() == "https")
                .map(|url| url.origin()),
            headers,
        }
    }

    pub(crate) fn for_url(&self, url: &Url) -> &'a [(&'a str, &'a str)] {
        if url.scheme() == "https" && self.origin.as_ref() == Some(&url.origin()) {
            self.headers
        } else {
            &[]
        }
    }
}

/// Follow redirects with a fresh GET at each hop. Only public headers may
/// travel to every destination. The agent's global timeout covers the chain.
pub(crate) fn get_following_redirects(
    agent: &ureq::Agent,
    url: &str,
    public_headers: &[(&str, &str)],
    scoped_headers: ScopedHeaders<'_>,
) -> Result<(Url, Response<ureq::Body>), ureq::Error> {
    let started = Instant::now();
    let mut url = Url::parse(url).map_err(|_| ureq::Error::BadUri("invalid URL".into()))?;
    let timeout = agent.config().timeouts().global;
    for hop in 0..=MAX_REDIRECTS {
        let remaining = timeout.map(|timeout| timeout.saturating_sub(started.elapsed()));
        if remaining == Some(Duration::ZERO) {
            return Err(ureq::Error::Timeout(ureq::Timeout::Global));
        }
        let mut request = agent
            .get(url.as_str())
            .config()
            .max_redirects(0)
            .timeout_global(remaining)
            .build();
        for &(name, value) in public_headers.iter().chain(scoped_headers.for_url(&url)) {
            request = request.header(name, value);
        }
        let response = request.call()?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Ok((url, response));
        }
        if hop == MAX_REDIRECTS {
            break;
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .filter(|location| !location.is_empty())
            .ok_or(ureq::Error::RedirectFailed)?;
        url = url
            .join(location)
            .map_err(|_| ureq::Error::RedirectFailed)?;
    }
    Err(ureq::Error::TooManyRedirects)
}

/// What a caller adds to [`agent`]'s settings.
pub type Config = ureq::config::ConfigBuilder<ureq::typestate::AgentScope>;

/// An agent for requests to `url`: platform TLS and roots, and the proxy the
/// computer uses for it.
pub fn agent(url: &str, configure: impl FnOnce(Config) -> Config) -> ureq::Agent {
    configure(base().proxy(proxy_for(url))).build().into()
}

/// An agent that only connects to this computer: no proxy, and every name,
/// redirects included, must resolve to loopback addresses alone
/// ([`LoopbackResolver`]), checked on the addresses it then connects to.
pub fn loopback_agent(configure: impl FnOnce(Config) -> Config) -> ureq::Agent {
    ureq::Agent::with_parts(
        configure(base().proxy(None)).build(),
        ureq::unversioned::transport::DefaultConnector::new(),
        LoopbackResolver,
    )
}

fn base() -> Config {
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(15)))
}

/// Resolves as ureq does, then refuses any name with an address off this
/// computer, so neither a hostname nor a redirect can leave it.
#[derive(Debug)]
pub struct LoopbackResolver;

/// Why [`LoopbackResolver`] refused, as the request's error says.
pub const NOT_LOOPBACK: &str = "only this computer (loopback) may be reached";

impl ureq::unversioned::resolver::Resolver for LoopbackResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let resolved = ureq::unversioned::resolver::DefaultResolver::default()
            .resolve(uri, config, timeout)?;
        if resolved.iter().all(|address| address.ip().is_loopback()) {
            Ok(resolved)
        } else {
            Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                NOT_LOOPBACK,
            )))
        }
    }
}

/// The host of `url`, for messages and logs (never the path or query).
pub fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the server".into())
}

/// Why a request failed, in a person's words, without the URL.
pub fn reason(error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(407) => "the proxy needs a sign-in Hex can't give".into(),
        ureq::Error::StatusCode(code) => format!("the server answered HTTP {code}"),
        ureq::Error::HostNotFound => "the name couldn't be looked up (DNS)".into(),
        ureq::Error::ConnectProxyFailed(detail) => format!("the proxy refused ({detail})"),
        ureq::Error::Tls(detail) => tls(detail),
        ureq::Error::NativeTls(detail) => tls(detail),
        ureq::Error::RequireHttpsOnly(_) => "the request requires HTTPS".into(),
        // SChannel, Security.framework, and OpenSSL can also report a
        // rejected certificate as an I/O error naming it.
        ureq::Error::Io(detail) if looks_like_tls(&detail.to_string()) => tls(detail),
        ureq::Error::Timeout(_) => "the connection timed out".into(),
        ureq::Error::ConnectionFailed => "the connection failed".into(),
        ureq::Error::Io(detail) => format!("the connection failed ({detail})"),
        error => error.to_string(),
    }
}

/// A certificate or TLS failure: on a managed network, usually a firewall
/// inspecting TLS whose root certificate this computer doesn't trust.
fn tls(detail: impl std::fmt::Display) -> String {
    format!("the certificate isn't trusted or the secure connection failed ({detail})")
}

fn looks_like_tls(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    [
        "certificate",
        "tls",
        "ssl",
        "handshake",
        "trust",
        "x509",
        "schannel",
    ]
    .iter()
    .any(|word| detail.contains(word))
}

/// "Couldn't reach `host`: `reason`", naming the proxy when one was used.
pub fn unreachable(url: &str, error: &ureq::Error) -> String {
    let via = proxy_for(url)
        .map(|proxy| format!(" through the proxy {}:{}", proxy.host(), proxy.port()))
        .unwrap_or_default();
    format!("Couldn't reach {}{via}: {}", host(url), reason(error))
}
