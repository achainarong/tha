//! Server-side Docker API allowlist, started by a forced command in `authorized_keys`.
//!
//! This is the security boundary for remote servers: a holder of the SSH key can bypass the
//! `tha` client, so nothing here may trust it. Everything is refused unless a route below
//! allows it, and an allowed call is re-created rather than forwarded, so client headers and
//! bodies never reach the engine.
pub mod setup;

use anyhow::{Context, Result, bail, ensure};
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, os::unix::fs::MetadataExt, path::Path, path::PathBuf, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};

pub const CONFIG_PATH: &str = "/etc/tha/agent.toml";
const MAX_REQUEST_BUFFER: usize = 64 * 1024;
const MAX_LOGGED_TARGET: usize = 256;

type Body = BoxBody<Bytes, hyper::Error>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Lets the key remove containers, images and volumes. Force is refused regardless.
    pub allow_delete: bool,
    pub engines: Vec<AgentEngine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEngine {
    pub name: String,
    pub socket: PathBuf,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<()> {
        for (index, engine) in self.engines.iter().enumerate() {
            ensure!(
                valid_engine_name(&engine.name),
                "Invalid engine name '{}'",
                engine.name.escape_default()
            );
            ensure!(
                engine.socket.is_absolute(),
                "Engine socket must be an absolute path: {}",
                engine.socket.display()
            );
            ensure!(
                self.engines[..index]
                    .iter()
                    .all(|other| other.name != engine.name),
                "Duplicate engine name '{}'",
                engine.name
            );
        }
        Ok(())
    }
}

/// `list` is the reserved discovery command, so it can never name an engine.
pub fn valid_engine_name(name: &str) -> bool {
    name != "list"
        && !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The policy must be writable only by root, otherwise the agent user could widen it.
pub fn load_config(path: &Path) -> Result<AgentConfig> {
    load_config_owned_by(path, 0)
}

fn load_config_owned_by(path: &Path, owner: u32) -> Result<AgentConfig> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(
            "This server's tha-agent is not set up yet: {} is missing. Run: sudo tha-agent setup --user root, or the setup command Tha shows for a dedicated user",
            path.display()
        ),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Cannot read agent configuration {}", path.display()));
        }
    };
    ensure!(
        metadata.file_type().is_file(),
        "Agent configuration must be a regular file: {}",
        path.display()
    );
    ensure!(
        metadata.uid() == owner && metadata.mode() & 0o022 == 0,
        "Agent configuration must be owned by root and not group or world writable: {}",
        path.display()
    );
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read agent configuration {}", path.display()))?;
    let config: AgentConfig = toml::from_str(&text)
        .with_context(|| format!("Invalid agent configuration {}", path.display()))?;
    config.validate()?;
    Ok(config)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    Literal(&'static str),
    /// One container or volume name.
    Name,
    /// An immutable `sha256:` image ID. Removal must not follow a moving tag.
    ImageId,
    /// An image reference, which may span several path segments (`library/nginx:1`).
    ImageRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value {
    Any,
    Number,
    /// `force=true` and similar must never pass, whatever the app would send.
    False,
    Boolean,
}

struct Route {
    method: Method,
    path: &'static [Segment],
    query: &'static [(&'static str, Value)],
    access: Access,
}

use Segment::{ImageId, ImageRef, Literal, Name};

macro_rules! route {
    ($method:ident, $access:ident, $path:expr, $query:expr) => {
        Route {
            method: Method::$method,
            path: $path,
            query: $query,
            access: Access::$access,
        }
    };
}

/// Every call `tha` makes through this agent. Anything missing here is refused.
fn routes() -> [Route; 20] {
    use Value::{Any, Boolean, False, Number};
    [
        route!(GET, Read, &[Literal("_ping")], &[]),
        route!(HEAD, Read, &[Literal("_ping")], &[]),
        route!(GET, Read, &[Literal("version")], &[]),
        route!(GET, Read, &[Literal("info")], &[]),
        route!(
            GET,
            Read,
            &[Literal("events")],
            &[("since", Any), ("until", Any), ("filters", Any)]
        ),
        route!(GET, Read, &[Literal("networks")], &[]),
        route!(GET, Read, &[Literal("volumes")], &[]),
        route!(
            GET,
            Read,
            &[Literal("containers"), Literal("json")],
            &[("all", Boolean), ("size", False)]
        ),
        route!(
            GET,
            Read,
            &[Literal("containers"), Name, Literal("json")],
            &[("size", Boolean)]
        ),
        route!(
            GET,
            Read,
            &[Literal("containers"), Name, Literal("logs")],
            &[
                ("follow", Boolean),
                ("stdout", Boolean),
                ("stderr", Boolean),
                ("since", Number),
                ("until", Number),
                ("timestamps", Boolean),
                ("tail", Any),
            ]
        ),
        route!(
            GET,
            Read,
            &[Literal("containers"), Name, Literal("stats")],
            &[("stream", Boolean), ("one-shot", Boolean)]
        ),
        route!(
            GET,
            Read,
            &[Literal("images"), Literal("json")],
            &[
                ("all", Boolean),
                ("shared-size", False),
                ("digests", Boolean),
                ("manifests", Boolean),
            ]
        ),
        route!(
            GET,
            Read,
            &[Literal("images"), ImageRef, Literal("json")],
            &[]
        ),
        route!(
            GET,
            Read,
            &[Literal("images"), ImageRef, Literal("history")],
            &[]
        ),
        route!(
            POST,
            Write,
            &[Literal("containers"), Name, Literal("start")],
            &[]
        ),
        route!(
            POST,
            Write,
            &[Literal("containers"), Name, Literal("stop")],
            &[("t", Number)]
        ),
        route!(
            POST,
            Write,
            &[Literal("containers"), Name, Literal("restart")],
            &[("t", Number)]
        ),
        route!(
            DELETE,
            Delete,
            &[Literal("containers"), Name],
            &[("force", False), ("v", False), ("link", False)]
        ),
        route!(
            DELETE,
            Delete,
            &[Literal("images"), ImageId],
            &[("force", False), ("noprune", Boolean)]
        ),
        route!(
            DELETE,
            Delete,
            &[Literal("volumes"), Name],
            &[("force", False)]
        ),
    ]
}

fn valid_name(value: &str) -> bool {
    value.len() <= 255
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_image_segment(value: &str) -> bool {
    value.len() <= 255
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@'))
}

fn path_matches(pattern: &[Segment], segments: &[&str]) -> bool {
    match pattern {
        [] => segments.is_empty(),
        [ImageRef, rest @ ..] => {
            // Image references are the only multi-segment value, and always end the variable part.
            let Some(available) = segments.len().checked_sub(rest.len()) else {
                return false;
            };
            (1..=8).contains(&available)
                && segments[..available]
                    .iter()
                    .all(|value| valid_image_segment(value))
                && path_matches(rest, &segments[available..])
        }
        [first, rest @ ..] => {
            let Some((value, remaining)) = segments.split_first() else {
                return false;
            };
            let matched = match first {
                Literal(expected) => value == expected,
                Name => valid_name(value),
                ImageId => crate::platform::immutable_image_id(value),
                ImageRef => unreachable!("handled above"),
            };
            matched && path_matches(rest, remaining)
        }
    }
}

fn query_allowed(allowed: &[(&str, Value)], query: &str) -> std::result::Result<(), String> {
    if query.is_empty() {
        return Ok(());
    }
    // Engines differ on `;` and repeated keys, so neither is allowed to mean anything here.
    if query.contains(';') {
        return Err("query contains a separator the agent does not accept".into());
    }
    let mut seen = Vec::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if seen.contains(&key) {
            return Err(format!("repeated query parameter {key}"));
        }
        seen.push(key);
        let Some((_, rule)) = allowed.iter().find(|(name, _)| *name == key) else {
            return Err(format!("query parameter {key} is not allowed"));
        };
        let valid = match rule {
            Value::Any => true,
            Value::Number => {
                !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit())
            }
            Value::False => value == "false",
            Value::Boolean => matches!(value, "true" | "false"),
        };
        if !valid {
            return Err(format!("query parameter {key} has a refused value"));
        }
    }
    Ok(())
}

/// Decide one request line. `target` is the raw path and query.
pub fn check(
    allow_delete: bool,
    method: &Method,
    target: &str,
) -> std::result::Result<Access, String> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let Some(path) = path.strip_prefix('/') else {
        return Err("path must start with /".into());
    };
    // Rejecting the encoded and relative forms outright means there is exactly one spelling
    // of each route, so the agent and the engine cannot disagree about which one this is.
    if path
        .chars()
        .any(|c| matches!(c, '%' | '\\' | '#') || !c.is_ascii_graphic())
    {
        return Err("path contains refused characters".into());
    }
    let mut segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err("path contains an empty segment".into());
    }
    if segments.first().is_some_and(|first| is_version(first)) {
        segments.remove(0);
    }
    let table = routes();
    let route = table
        .iter()
        .find(|route| route.method == *method && path_matches(route.path, &segments));
    let Some(route) = route else {
        return Err("this API call is not on the allowlist".into());
    };
    if route.access == Access::Delete && !allow_delete {
        return Err("deleting is turned off on this server".into());
    }
    query_allowed(route.query, query)?;
    Ok(route.access)
}

/// `v1.53`, exactly. `libpod` paths and three-part versions are not the Docker-compatible API.
fn is_version(segment: &str) -> bool {
    segment.strip_prefix('v').is_some_and(|version| {
        version.split_once('.').is_some_and(|(major, minor)| {
            [major, minor].iter().all(|part| {
                !part.is_empty() && part.len() <= 4 && part.bytes().all(|b| b.is_ascii_digit())
            })
        })
    })
}

/// Refused outright rather than stripped: a client sending these is not `tha`.
fn screen_headers<B>(request: &Request<B>) -> std::result::Result<(), String> {
    if request.uri().scheme().is_some() || request.uri().authority().is_some() {
        return Err("absolute request targets are refused".into());
    }
    let headers = request.headers();
    for name in [header::UPGRADE, header::TRANSFER_ENCODING, header::EXPECT] {
        if headers.contains_key(&name) {
            return Err(format!("{name} requests are refused"));
        }
    }
    if headers.get_all(header::CONNECTION).iter().any(|value| {
        value
            .to_str()
            .map_or(true, |value| value.to_ascii_lowercase().contains("upgrade"))
    }) {
        return Err("connection upgrades are refused".into());
    }
    if let Some(length) = headers.get(header::CONTENT_LENGTH)
        && length.as_bytes() != b"0"
    {
        return Err("request bodies are refused".into());
    }
    Ok(())
}

pub struct Agent {
    pub allow_delete: bool,
    pub socket: PathBuf,
    /// Where writes and refusals are recorded. Stderr is not used: SSH returns it to the client.
    pub audit: fn(&str),
}

fn message_response(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::json!({ "message": message }).to_string();
    let mut response = Response::new(
        Full::new(Bytes::from(body))
            .map_err(|never: Infallible| match never {})
            .boxed(),
    );
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

/// Client-controlled text, bounded and escaped before it reaches the system log.
fn loggable(target: &str) -> String {
    target
        .chars()
        .take(MAX_LOGGED_TARGET)
        .flat_map(char::escape_default)
        .collect()
}

async fn handle(
    request: Request<Incoming>,
    agent: Arc<Agent>,
) -> Result<Response<Body>, Infallible> {
    let method = request.method().clone();
    let target = request
        .uri()
        .path_and_query()
        .map_or("", |value| value.as_str())
        .to_owned();
    let decision =
        screen_headers(&request).and_then(|()| check(agent.allow_delete, &method, &target));
    drop(request);
    let access = match decision {
        Ok(access) => access,
        Err(reason) => {
            (agent.audit)(&format!("refused {method} {}: {reason}", loggable(&target)));
            return Ok(message_response(
                StatusCode::FORBIDDEN,
                &format!("Not allowed by server: {reason}"),
            ));
        }
    };
    if access != Access::Read {
        (agent.audit)(&format!("allowed {method} {}", loggable(&target)));
    }
    Ok(match forward(&agent.socket, method, &target).await {
        Ok(response) => response,
        Err(error) => {
            (agent.audit)(&format!("engine unavailable: {error:#}"));
            message_response(StatusCode::BAD_GATEWAY, "The engine is unavailable")
        }
    })
}

/// A new request carrying only the validated line, so nothing else the client sent can pass.
async fn forward(socket: &Path, method: Method, target: &str) -> Result<Response<Body>> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("Cannot connect to {}", socket.display()))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .context("Engine handshake failed")?;
    tokio::spawn(connection);
    let request = Request::builder()
        .method(method)
        .uri(target)
        .header(header::HOST, "docker")
        .body(Empty::<Bytes>::new())
        .context("Cannot build engine request")?;
    let response = sender
        .send_request(request)
        .await
        .context("Engine request failed")?;
    let (mut parts, body) = response.into_parts();
    // hyper recomputes framing for this side of the connection.
    for name in [
        header::CONNECTION,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        parts.headers.remove(name);
    }
    Ok(Response::from_parts(parts, body.boxed()))
}

/// Serve HTTP/1 on `io` until the client disconnects. Every request is checked individually,
/// so an allowed call cannot carry a later refused one on the same connection.
pub async fn serve<I>(io: I, agent: Agent) -> Result<()>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let agent = Arc::new(agent);
    hyper::server::conn::http1::Builder::new()
        .max_buf_size(MAX_REQUEST_BUFFER)
        .serve_connection(
            TokioIo::new(io),
            service_fn(move |request| handle(request, agent.clone())),
        )
        .await
        .context("Agent connection failed")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Requested<'a> {
    List,
    Engine(&'a AgentEngine),
}

/// `SSH_ORIGINAL_COMMAND` is whatever the client asked to run, so it is only ever compared.
pub fn parse_command<'a>(config: &'a AgentConfig, command: Option<&str>) -> Result<Requested<'a>> {
    let Some(command) = command else {
        bail!("This key only runs the tha-agent; no shell is available");
    };
    // The client's full command line, which a forced command receives as text.
    let command = command.strip_prefix("tha-agent serve ").unwrap_or(command);
    if command == "list" || command == "tha-agent list" {
        return Ok(Requested::List);
    }
    config
        .engines
        .iter()
        .find(|engine| engine.name == command)
        .map(Requested::Engine)
        .context("Unknown engine; this key only runs the tha-agent")
}

/// What `list` returns to the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub engines: Vec<String>,
    pub allow_delete: bool,
}

pub fn listing(config: &AgentConfig) -> Listing {
    Listing {
        engines: config
            .engines
            .iter()
            .map(|engine| engine.name.clone())
            .collect(),
        allow_delete: config.allow_delete,
    }
}

fn syslog(message: &str) {
    let client = std::env::var("SSH_CLIENT").unwrap_or_default();
    let client = client.split(' ').next().unwrap_or_default();
    let Ok(line) = std::ffi::CString::new(format!("{} {message}", loggable(client))) else {
        return;
    };
    // SAFETY: every pointer is a NUL-terminated string that outlives its call. The ident is a
    // static literal because openlog retains it, and the message goes through a literal "%s".
    unsafe {
        libc::openlog(c"tha-agent".as_ptr(), libc::LOG_PID, libc::LOG_AUTH);
        libc::syslog(libc::LOG_INFO, c"%s".as_ptr(), line.as_ptr());
    }
}

/// The forced-command entry point. It reads no user configuration or `--config` value.
pub async fn run(argument: Option<&str>) -> Result<()> {
    let config = load_config(Path::new(CONFIG_PATH))?;
    // A forced command always has this set, so a restricted key cannot use the arguments.
    let command = std::env::var("SSH_ORIGINAL_COMMAND").ok();
    match parse_command(&config, command.as_deref().or(argument))? {
        Requested::List => {
            println!("{}", serde_json::to_string(&listing(&config))?);
            Ok(())
        }
        Requested::Engine(engine) => {
            let result = serve(
                tokio::io::join(tokio::io::stdin(), tokio::io::stdout()),
                Agent {
                    allow_delete: config.allow_delete,
                    socket: engine.socket.clone(),
                    audit: syslog,
                },
            )
            .await;
            // Tokio's stdin reader blocks a thread that a normal runtime shutdown would wait on.
            std::process::exit(i32::from(result.is_err()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef";
    const IMAGE: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn allowed(method: Method, target: &str) -> bool {
        check(true, &method, target).is_ok()
    }

    #[test]
    fn every_call_tha_makes_is_allowed_with_and_without_a_version_prefix() {
        for prefix in ["", "/v1.53"] {
            for (method, target) in [
                (Method::GET, "/_ping".to_owned()),
                (Method::HEAD, "/_ping".to_owned()),
                (Method::GET, "/version".to_owned()),
                (Method::GET, "/info".to_owned()),
                (
                    Method::GET,
                    "/events?since=1&filters=%7B%22type%22%3A%5B%22container%22%5D%7D".to_owned(),
                ),
                (Method::GET, "/networks".to_owned()),
                (Method::GET, "/volumes".to_owned()),
                (
                    Method::GET,
                    "/containers/json?all=true&size=false".to_owned(),
                ),
                (Method::GET, format!("/containers/{ID}/json?size=false")),
                (
                    Method::GET,
                    format!(
                        "/containers/{ID}/logs?stdout=true&stderr=true&follow=true&timestamps=true&tail=100&since=5"
                    ),
                ),
                (
                    Method::GET,
                    format!("/containers/{ID}/stats?stream=false&one-shot=true"),
                ),
                (
                    Method::GET,
                    "/images/json?all=true&shared-size=false&digests=false&manifests=false"
                        .to_owned(),
                ),
                (Method::GET, format!("/images/{IMAGE}/json")),
                (Method::GET, "/images/library/nginx:1.27/json".to_owned()),
                (Method::GET, format!("/images/{IMAGE}/history")),
                (Method::POST, format!("/containers/{ID}/start")),
                (Method::POST, format!("/containers/{ID}/stop?t=10")),
                (Method::POST, format!("/containers/{ID}/restart?t=10")),
                (
                    Method::DELETE,
                    format!("/containers/{ID}?force=false&v=false&link=false"),
                ),
                (
                    Method::DELETE,
                    format!("/images/{IMAGE}?force=false&noprune=true"),
                ),
                (Method::DELETE, "/volumes/data?force=false".to_owned()),
            ] {
                let target = format!("{prefix}{target}");
                assert!(allowed(method.clone(), &target), "{method} {target}");
            }
        }
    }

    #[test]
    fn dangerous_calls_are_refused() {
        for (method, target) in [
            (Method::POST, "/containers/create".to_owned()),
            (Method::POST, format!("/containers/{ID}/exec")),
            (Method::POST, "/exec/abc/start".to_owned()),
            (Method::POST, format!("/containers/{ID}/attach")),
            (Method::GET, format!("/containers/{ID}/attach/ws")),
            (Method::POST, format!("/containers/{ID}/kill")),
            (Method::POST, format!("/containers/{ID}/update")),
            (Method::POST, format!("/containers/{ID}/rename?name=x")),
            (Method::GET, format!("/containers/{ID}/archive?path=/etc")),
            (Method::PUT, format!("/containers/{ID}/archive?path=/")),
            (Method::GET, format!("/containers/{ID}/export")),
            (Method::GET, format!("/containers/{ID}/top")),
            (Method::POST, "/containers/prune".to_owned()),
            (Method::POST, "/images/create?fromImage=evil".to_owned()),
            (Method::POST, "/build".to_owned()),
            (Method::POST, "/commit".to_owned()),
            (Method::POST, "/images/prune".to_owned()),
            (Method::GET, format!("/images/{IMAGE}/get")),
            (Method::POST, format!("/images/{IMAGE}/push")),
            (Method::POST, format!("/images/{IMAGE}/tag")),
            (Method::GET, "/distribution/evil.example/x/json".to_owned()),
            (Method::POST, "/volumes/create".to_owned()),
            (Method::POST, "/volumes/prune".to_owned()),
            (Method::POST, "/networks/create".to_owned()),
            (Method::POST, "/networks/n/connect".to_owned()),
            (Method::DELETE, "/networks/n".to_owned()),
            (Method::GET, "/plugins".to_owned()),
            (Method::GET, "/secrets".to_owned()),
            (Method::POST, "/swarm/init".to_owned()),
            (Method::GET, "/system/df".to_owned()),
            (Method::POST, "/session".to_owned()),
            (Method::GET, "/v4.0.0/libpod/containers/json".to_owned()),
            (Method::GET, "/libpod/info".to_owned()),
            (Method::GET, "/unknown".to_owned()),
            (Method::GET, "/".to_owned()),
            (Method::PUT, "/containers/json".to_owned()),
            (Method::POST, "/_ping".to_owned()),
            (Method::POST, format!("/containers/{ID}/json")),
            (Method::DELETE, format!("/images/{ID}x")),
            (
                Method::DELETE,
                "/images/nginx:latest?force=false".to_owned(),
            ),
        ] {
            assert!(!allowed(method.clone(), &target), "{method} {target}");
        }
    }

    #[test]
    fn disguised_paths_are_refused() {
        for target in [
            format!("/containers/{ID}/../{ID}/exec"),
            format!("/containers/{ID}/start/../exec"),
            format!("/containers/..%2f{ID}/start"),
            "/containers/%2e%2e/start".to_owned(),
            format!("/containers/{ID}%2Fexec"),
            format!("/containers/{ID}/st%61rt"),
            format!("//containers/{ID}/start"),
            format!("/containers//{ID}/start"),
            format!("/containers/{ID}/start/"),
            format!("/v1.53/v1.53/containers/{ID}/start"),
            format!("/v1/containers/{ID}/start"),
            format!("/v1.53.0/containers/{ID}/start"),
            format!("/containers/{ID}\\start"),
            "/containers/.hidden/start".to_owned(),
            format!("containers/{ID}/start"),
            "/containers/../images/create/start".into(),
            "*".into(),
        ] {
            assert!(!allowed(Method::POST, &target), "{target}");
        }
    }

    #[test]
    fn disallowed_query_parameters_are_refused() {
        for (method, target) in [
            (Method::DELETE, format!("/containers/{ID}?force=true")),
            (Method::DELETE, format!("/containers/{ID}?force=1")),
            (Method::DELETE, format!("/containers/{ID}?force")),
            (Method::DELETE, format!("/containers/{ID}?v=true")),
            (
                Method::DELETE,
                format!("/containers/{ID}?force=false&force=true"),
            ),
            (
                Method::DELETE,
                format!("/containers/{ID}?force=false;force=true"),
            ),
            (Method::DELETE, format!("/containers/{ID}?for%63e=true")),
            (Method::DELETE, format!("/images/{IMAGE}?force=true")),
            (Method::DELETE, "/volumes/data?force=true".to_owned()),
            (
                Method::POST,
                format!("/containers/{ID}/stop?signal=SIGKILL"),
            ),
            (Method::POST, format!("/containers/{ID}/stop?t=-1")),
            (Method::POST, format!("/containers/{ID}/start?detachKeys=x")),
            (Method::GET, "/containers/json?all=maybe".to_owned()),
            (Method::GET, "/networks?filters=x".to_owned()),
        ] {
            assert!(!allowed(method.clone(), &target), "{method} {target}");
        }
    }

    #[test]
    fn deleting_requires_the_server_to_enable_it() {
        for target in [
            format!("/containers/{ID}?force=false"),
            format!("/images/{IMAGE}?force=false"),
            "/volumes/data?force=false".to_owned(),
        ] {
            assert!(check(false, &Method::DELETE, &target).is_err(), "{target}");
            assert_eq!(check(true, &Method::DELETE, &target), Ok(Access::Delete));
        }
    }

    #[test]
    fn writes_are_classified_for_the_audit_log() {
        assert_eq!(
            check(false, &Method::POST, &format!("/containers/{ID}/stop?t=10")),
            Ok(Access::Write)
        );
        assert_eq!(check(false, &Method::GET, "/info"), Ok(Access::Read));
    }

    fn config() -> AgentConfig {
        AgentConfig {
            allow_delete: false,
            engines: vec![AgentEngine {
                name: "docker".into(),
                socket: "/var/run/docker.sock".into(),
            }],
        }
    }

    #[test]
    fn only_list_and_exact_engine_names_are_accepted_as_commands() {
        let config = config();
        assert_eq!(
            parse_command(&config, Some("list")).unwrap(),
            Requested::List
        );
        assert_eq!(
            parse_command(&config, Some("tha-agent list")).unwrap(),
            Requested::List
        );
        for command in ["docker", "tha-agent serve docker"] {
            assert!(matches!(
                parse_command(&config, Some(command)).unwrap(),
                Requested::Engine(engine) if engine.name == "docker"
            ));
        }
        for hostile in [
            None,
            Some(""),
            Some("sh"),
            Some("docker "),
            Some(" docker"),
            Some("docker; sh"),
            Some("docker\nsh"),
            Some("$(sh)"),
            Some("/bin/sh -c id"),
            Some("list --all"),
            Some("tha-agent serve docker; sh"),
            Some("tha-agent serve "),
            Some("tha-agent serve list --all"),
            Some("tha-agent list; sh"),
            Some("tha-agent setup"),
        ] {
            assert!(parse_command(&config, hostile).is_err(), "{hostile:?}");
        }
    }

    #[test]
    fn engine_names_cannot_collide_with_list_or_be_duplicated() {
        for name in ["list", "", "-x", "a b", "a/b", "a;b", "é"] {
            assert!(!valid_engine_name(name), "{name}");
        }
        let mut duplicate = config();
        duplicate.engines.push(duplicate.engines[0].clone());
        assert!(duplicate.validate().is_err());
        let mut relative = config();
        relative.engines[0].socket = "docker.sock".into();
        assert!(relative.validate().is_err());
    }

    #[test]
    fn policy_files_writable_by_others_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.toml");
        std::fs::write(
            &path,
            "allow_delete = true\n[[engines]]\nname = 'docker'\nsocket = '/s'\n",
        )
        .unwrap();
        // SAFETY: geteuid has no preconditions.
        let owner = unsafe { libc::geteuid() };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_config_owned_by(&path, owner).unwrap().allow_delete);
        assert!(load_config_owned_by(&path, owner + 1).is_err());
        for mode in [0o664, 0o646] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(load_config_owned_by(&path, owner).is_err(), "{mode:o}");
        }
        let link = directory.path().join("link.toml");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load_config_owned_by(&link, owner).is_err());
    }

    #[test]
    fn a_missing_policy_says_setup_has_not_run() {
        let directory = tempfile::tempdir().unwrap();
        let error = load_config_owned_by(&directory.path().join("agent.toml"), 0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not set up yet"), "{error}");
        assert!(error.contains("tha-agent setup"), "{error}");
    }

    #[test]
    fn unknown_policy_fields_are_refused() {
        let parsed: std::result::Result<AgentConfig, _> = toml::from_str("allow_exec = true\n");
        assert!(parsed.is_err());
    }

    #[test]
    fn logged_targets_are_escaped_and_bounded() {
        let logged = loggable(&format!("a\nb{}", "x".repeat(1000)));
        assert!(!logged.contains('\n'));
        assert!(logged.len() < MAX_LOGGED_TARGET + 8);
    }
}
