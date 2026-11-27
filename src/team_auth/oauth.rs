use std::io;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ureq::{Body, http::Response};
use url::Url;

use super::{AuthError, check_cancelled, loopback, now_ms, session::StoredSession};

const WHOAMI: &str = "/api/hex/whoami";

#[derive(Deserialize)]
struct ResourceMetadata {
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Deserialize)]
struct ServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
}

#[derive(Serialize)]
struct Registration<'a> {
    client_name: &'static str,
    redirect_uris: [&'a str; 1],
    grant_types: [&'static str; 2],
    response_types: [&'static str; 1],
    token_endpoint_auth_method: &'static str,
}

#[derive(Deserialize)]
struct Registered {
    client_id: String,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

impl Tokens {
    fn expires_at_ms(&self) -> Option<u64> {
        self.expires_in
            .map(|seconds| now_ms().saturating_add(seconds.saturating_mul(1000)))
    }
}

#[derive(Deserialize)]
pub(super) struct Member {
    principal: String,
    pub sub: String,
    pub email: String,
}

pub(super) fn sign_in(
    origin: &Url,
    override_id: Option<&str>,
    cached: Option<StoredSession>,
    cancelled: &AtomicBool,
    open: impl FnOnce(&Url) -> io::Result<()>,
    save: impl FnOnce(&StoredSession) -> Result<(), AuthError>,
) -> Result<StoredSession, AuthError> {
    let (metadata, resource) = discover(origin, cancelled)?;
    let allow_http = is_loopback(origin);
    let mut authorize = endpoint(&metadata.authorization_endpoint, allow_http)?;
    let token_endpoint = endpoint(&metadata.token_endpoint, allow_http)?;
    let cached = cached.filter(|session| session.token_endpoint == token_endpoint.as_str());
    let (listener, reused_port) = loopback::bind(cached.as_ref().and_then(|s| s.redirect_port))?;
    let redirect_uri = listener.redirect_uri();
    let client_id = match (override_id, cached) {
        (Some(id), _) => id.to_string(),
        (None, Some(session)) if reused_port || session.redirect_port.is_none() => {
            session.client_id
        }
        _ => {
            let registration = metadata
                .registration_endpoint
                .ok_or(AuthError::Configuration)?;
            let registration = endpoint(&registration, allow_http)?;
            check_cancelled(cancelled)?;
            register(&registration, &redirect_uri)?
        }
    };
    let pkce = loopback::Pkce::new()?;
    authorize.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", &redirect_uri),
        ("state", &pkce.state),
        ("code_challenge", &pkce.challenge()),
        ("code_challenge_method", "S256"),
        ("resource", resource.as_str()),
    ]);
    check_cancelled(cancelled)?;
    open(&authorize).map_err(|_| AuthError::Browser)?;
    let (callback, code) = listener.wait(&pkce.state, cancelled)?;
    let result = (|| {
        check_cancelled(cancelled)?;
        let tokens = tokens(
            &token_endpoint,
            [
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("code_verifier", &pkce.verifier),
                ("redirect_uri", &redirect_uri),
                ("client_id", &client_id),
                ("resource", resource.as_str()),
            ],
            AuthError::Denied,
        )?;
        check_cancelled(cancelled)?;
        let member = whoami(origin, &tokens.access_token)?.ok_or(AuthError::Verification)?;
        let session = StoredSession {
            origin: origin.to_string(),
            client_id,
            token_endpoint: token_endpoint.to_string(),
            access_expires_at_ms: tokens.expires_at_ms(),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            member_sub: member.sub,
            email: member.email,
            redirect_port: Some(listener.port),
        };
        save(&session)?;
        Ok(session)
    })();
    loopback::reply(callback, result.is_ok());
    result
}

fn discover(origin: &Url, cancelled: &AtomicBool) -> Result<(ServerMetadata, Url), AuthError> {
    let allow_http = is_loopback(origin);
    check_cancelled(cancelled)?;
    let whoami = join(origin, WHOAMI)?;
    let challenge = agent(&whoami).get(whoami.as_str()).call();
    let metadata_url = challenge
        .ok()
        .and_then(|response| {
            response
                .headers()
                .get_all("WWW-Authenticate")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .find_map(resource_metadata)
                .map(str::to_owned)
        })
        .map(|url| endpoint(&url, allow_http))
        .transpose()?
        .unwrap_or(join(origin, "/.well-known/oauth-protected-resource")?);
    check_cancelled(cancelled)?;
    let metadata: Option<ResourceMetadata> =
        match agent(&metadata_url).get(metadata_url.as_str()).call() {
            Ok(response) if response.status().is_success() => {
                Some(decode(response, AuthError::Service)?)
            }
            _ => None,
        };
    let resource = metadata
        .as_ref()
        .and_then(|metadata| metadata.resource.as_deref())
        .map(|resource| endpoint(resource, allow_http))
        .transpose()?
        .unwrap_or_else(|| origin.clone());
    let mut issuer = metadata
        .and_then(|metadata| metadata.authorization_servers.into_iter().next())
        .map(|issuer| endpoint(&issuer, allow_http))
        .transpose()?
        .unwrap_or_else(|| origin.clone());
    if issuer.query().is_some() {
        return Err(AuthError::Configuration);
    }
    let path = format!(
        "/.well-known/oauth-authorization-server{}",
        issuer.path().trim_end_matches('/')
    );
    issuer.set_path(&path);
    check_cancelled(cancelled)?;
    let response = agent(&issuer)
        .get(issuer.as_str())
        .call()
        .map_err(|_| AuthError::Service)?;
    Ok((decode(response, AuthError::Service)?, resource))
}

fn resource_metadata(challenge: &str) -> Option<&str> {
    // These parameters can follow a scheme (Bearer) or a previous parameter.
    for part in challenge.split(',') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name
            .split_ascii_whitespace()
            .next_back()
            .is_some_and(|name| name.eq_ignore_ascii_case("resource_metadata"))
            && let Some(quoted) = value.trim().strip_prefix('"')
        {
            return quoted.split_once('"').map(|(uri, _)| uri);
        }
    }
    None
}

fn register(url: &Url, redirect_uri: &str) -> Result<String, AuthError> {
    let payload = Registration {
        client_name: "Hex",
        redirect_uris: [redirect_uri],
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        token_endpoint_auth_method: "none",
    };
    let data = serde_json::to_vec(&payload).map_err(|_| AuthError::Configuration)?;
    let response = agent(url)
        .post(url.as_str())
        .header("Content-Type", "application/json")
        .send(data)
        .map_err(|_| AuthError::Service)?;
    let registered: Registered = decode(response, AuthError::Service)?;
    if registered.client_id.is_empty() {
        return Err(AuthError::Configuration);
    }
    Ok(registered.client_id)
}

pub(super) fn refresh(origin: &Url, session: &mut StoredSession) -> Result<(), AuthError> {
    let url = endpoint(&session.token_endpoint, is_loopback(origin))?;
    let refresh_token = session.refresh_token.as_deref().ok_or(AuthError::Refresh)?;
    let fresh = tokens(
        &url,
        [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", session.client_id.as_str()),
        ],
        AuthError::Refresh,
    )?;
    session.access_expires_at_ms = fresh.expires_at_ms();
    session.access_token = fresh.access_token;
    if let Some(refresh_token) = fresh.refresh_token {
        session.refresh_token = Some(refresh_token);
    }
    Ok(())
}

fn tokens<const N: usize>(
    url: &Url,
    fields: [(&str, &str); N],
    rejected: AuthError,
) -> Result<Tokens, AuthError> {
    let response = agent(url)
        .post(url.as_str())
        .send_form(fields)
        .map_err(|_| AuthError::Service)?;
    let tokens: Tokens = decode(response, rejected)?;
    if tokens.access_token.is_empty() {
        return Err(rejected);
    }
    Ok(tokens)
}

pub(super) fn whoami(origin: &Url, access_token: &str) -> Result<Option<Member>, AuthError> {
    let url = join(origin, WHOAMI)?;
    let response = agent(&url)
        .get(url.as_str())
        .header("Authorization", format!("Bearer {access_token}"))
        .call()
        .map_err(|_| AuthError::Service)?;
    if response.status().as_u16() == 401 || is_access_login_redirect(&response) {
        return Ok(None);
    }
    let member: Member = decode(response, AuthError::Verification)?;
    if member.principal != "member"
        || member.sub.trim().is_empty()
        || member.email.trim().is_empty()
    {
        return Err(AuthError::Verification);
    }
    Ok(Some(member))
}

fn is_access_login_redirect(response: &Response<Body>) -> bool {
    matches!(response.status().as_u16(), 302 | 303 | 307)
        && response
            .headers()
            .get("Location")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|location| location.contains("/cdn-cgi/access/login"))
}

pub(super) fn origin(value: &str) -> Result<Url, AuthError> {
    let url = endpoint(value, true)?;
    if url.path() != "/" || url.query().is_some() {
        return Err(AuthError::Configuration);
    }
    Ok(url)
}

fn endpoint(value: &str, allow_http: bool) -> Result<Url, AuthError> {
    let url = Url::parse(value).map_err(|_| AuthError::Configuration)?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || allow_http && url.scheme() == "http" && is_loopback(&url))
    {
        return Err(AuthError::Configuration);
    }
    Ok(url)
}

fn is_loopback(url: &Url) -> bool {
    url.host_str() == Some("127.0.0.1")
}

fn join(origin: &Url, path: &str) -> Result<Url, AuthError> {
    origin.join(path).map_err(|_| AuthError::Configuration)
}

fn agent(url: &Url) -> ureq::Agent {
    let config = |builder: crate::net::Config| {
        builder
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(12)))
    };
    if url.scheme() == "http" {
        crate::net::loopback_agent(config)
    } else {
        crate::net::agent(url.as_str(), config)
    }
}

fn decode<T: DeserializeOwned>(
    mut response: Response<Body>,
    error: AuthError,
) -> Result<T, AuthError> {
    if !response.status().is_success() {
        return Err(error);
    }
    let data = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_vec()
        .map_err(|_| error)?;
    serde_json::from_slice(&data).map_err(|_| error)
}
