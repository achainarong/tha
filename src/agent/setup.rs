//! Root-run server setup: dedicated user/key, root-owned policy, and persistent systemd service.
use super::{AgentConfig, AgentEngine};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

const KNOWN_SOCKETS: [(&str, &str); 2] = [
    ("docker", "/var/run/docker.sock"),
    ("podman", "/run/podman/podman.sock"),
];

/// A socket in one of these is a misconfiguration, and joining it would widen the agent user.
const PRIVILEGED_GROUPS: [&str; 9] = [
    "root", "wheel", "sudo", "admin", "adm", "kmem", "disk", "shadow", "sys",
];

const KEY_TYPES: [&str; 6] = [
    "ssh-ed25519",
    "sk-ssh-ed25519@openssh.com",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
];

#[derive(Debug, Clone)]
pub struct Options {
    pub user: String,
    pub key: Option<String>,
    pub allow_delete: bool,
    pub allow_exec: bool,
    pub allow_images: bool,
    pub allow_download: bool,
    pub allow_server_download: bool,
    pub allow_compose: bool,
    pub allow_compose_edit: bool,
    pub allow_upload: bool,
    /// Where the agent posts alerts. Kept in a root-only file, not the policy.
    pub alert_webhook: Option<String>,
    pub no_alert_webhook: bool,
    /// `NAME=/path/to/socket`, to expose only these. When empty, every known socket is exposed.
    pub engines: Vec<String>,
    pub scanner_digest: Option<String>,
    pub security_interval: u64,
    pub no_security: bool,
}

/// A public key reduced to its type and data. The comment and any `authorized_keys` options
/// are dropped, so pasted text cannot add options to the line this setup writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    kind: String,
    data: String,
}

impl PublicKey {
    pub fn as_line(&self) -> String {
        format!("{} {}", self.kind, self.data)
    }
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim_end_matches('=');
    if !text.len().is_multiple_of(4) || text.len() - trimmed.len() > 2 {
        return None;
    }
    let mut bytes = Vec::with_capacity(trimmed.len() * 3 / 4);
    let (mut buffer, mut bits) = (0_u32, 0_u32);
    for byte in trimmed.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(bytes)
}

pub fn parse_public_key(text: &str) -> Result<PublicKey> {
    ensure!(
        !text.chars().any(|c| c.is_control() && c != '\n'),
        "The public key contains control characters"
    );
    let text = text.trim();
    ensure!(
        !text.contains('\n'),
        "Expected exactly one public key, on one line"
    );
    let mut parts = text.split_whitespace();
    let (Some(kind), Some(data)) = (parts.next(), parts.next()) else {
        bail!("Expected a public key such as: ssh-ed25519 AAAA... comment");
    };
    ensure!(
        KEY_TYPES.contains(&kind),
        "Unsupported public key type; expected a line from a .pub file, without options"
    );
    let decoded = base64_decode(data).context("The public key data is not valid base64")?;
    // The key blob starts with its own type, so a mismatched or truncated paste fails here.
    let declared = decoded
        .get(..4)
        .map(|length| u32::from_be_bytes([length[0], length[1], length[2], length[3]]) as usize)
        .and_then(|length| decoded.get(4..4 + length));
    ensure!(
        declared == Some(kind.as_bytes()),
        "The public key data does not match its type"
    );
    Ok(PublicKey {
        kind: kind.to_owned(),
        data: data.to_owned(),
    })
}

pub(crate) fn valid_user(name: &str) -> bool {
    name != "root"
        && name.len() <= 32
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// sshd passes this line to the user's shell, so the path is restricted to plain characters
/// rather than quoted.
pub fn authorized_line(executable: &Path, key: &PublicKey) -> Result<String> {
    let path = executable
        .to_str()
        .context("The tha binary path is not valid text")?;
    ensure!(
        path.starts_with('/')
            && path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')),
        "Install tha at a path with plain characters, such as /usr/local/bin/tha"
    );
    Ok(format!("restrict,command=\"{path}\" {}", key.as_line()))
}

/// New `authorized_keys` content. Any other active line would be a way around the agent, so
/// setup stops and names it instead of leaving a key that looks restricted but is not.
pub fn merge_authorized_keys(existing: &str, line: &str) -> Result<String> {
    let (command, key) = line
        .strip_prefix("restrict,command=\"")
        .and_then(|line| line.split_once("\" "))
        .context("Expected a restricted agent key line")?;
    let key = parse_public_key(key)?;
    ensure!(
        authorized_line(Path::new(command), &key)? == line,
        "Expected a canonical restricted agent key line"
    );
    let prefix = format!("restrict,command=\"{command}\" ");
    for (number, current) in existing.lines().enumerate() {
        let current = current.trim();
        if !current.is_empty()
            && !current.starts_with('#')
            && !current
                .strip_prefix(&prefix)
                .is_some_and(|key| parse_public_key(key).is_ok())
        {
            bail!(
                "authorized_keys line {} is not restricted to the agent, so it could bypass the allowlist. Remove it or choose a dedicated user.",
                number + 1
            );
        }
    }
    if existing.lines().any(|current| current.trim() == line) {
        return Ok(existing.to_owned());
    }
    let mut merged = existing.to_owned();
    if !merged.is_empty() && !merged.ends_with('\n') {
        merged.push('\n');
    }
    merged.push_str(line);
    merged.push('\n');
    Ok(merged)
}

pub fn parse_engine(text: &str) -> Result<AgentEngine> {
    let (name, socket) = text
        .split_once('=')
        .context("Expected NAME=/path/to/socket")?;
    let engine = AgentEngine {
        name: name.to_owned(),
        socket: PathBuf::from(socket),
    };
    AgentConfig {
        allow_delete: false,
        engines: vec![engine.clone()],
        ..Default::default()
    }
    .validate()?;
    Ok(engine)
}

/// A rerun that only adds a key must not silently change what the server already allows.
pub fn merge_config(
    existing: Option<AgentConfig>,
    engines: Vec<AgentEngine>,
    allow_delete: bool,
) -> AgentConfig {
    let existing = existing.unwrap_or_default();
    AgentConfig {
        allow_delete: allow_delete || existing.allow_delete,
        allow_exec: existing.allow_exec,
        allow_images: existing.allow_images,
        allow_download: existing.allow_download,
        allow_server_download: existing.allow_server_download,
        allow_compose: existing.allow_compose,
        allow_compose_edit: existing.allow_compose_edit,
        allow_upload: existing.allow_upload,
        engines: if engines.is_empty() {
            existing.engines
        } else {
            engines
        },
        service_uid: existing.service_uid,
        service_gid: existing.service_gid,
        security: existing.security,
    }
}

fn run(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("Cannot run {:?}", command.get_program()))?;
    ensure!(
        output.status.success(),
        "{:?} failed: {}",
        command.get_program(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct Account {
    uid: u32,
    gid: u32,
    home: PathBuf,
    shell: String,
}

fn account(user: &str) -> Result<Option<Account>> {
    let output = Command::new("getent")
        .args(["passwd", user])
        .output()
        .context("Cannot run getent")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<_> = text.trim().split(':').collect();
    let [_, _, uid, gid, _, home, shell] = fields.as_slice() else {
        bail!("Unexpected account entry for {user}");
    };
    Ok(Some(Account {
        uid: uid.parse().context("Unexpected user ID")?,
        gid: gid.parse().context("Unexpected group ID")?,
        home: PathBuf::from(home),
        shell: (*shell).to_owned(),
    }))
}

/// Plan socket access without changing groups. Provisioning must validate and install
/// restricted keys and policy before granting any engine privileges.
fn socket_group(account: &Account, engine: &AgentEngine) -> Result<Option<String>> {
    let metadata = std::fs::metadata(&engine.socket)
        .with_context(|| format!("Cannot find {}", engine.socket.display()))?;
    ensure!(
        metadata.file_type().is_socket(),
        "{} is not a Unix socket",
        engine.socket.display()
    );
    if metadata.uid() == account.uid || metadata.gid() == account.gid {
        return Ok(None);
    }
    ensure!(
        metadata.mode() & 0o020 != 0 && metadata.gid() != 0,
        "{} is only accessible to its owner (UID {}). For a rootless engine, or rootful Podman without a socket group, give the socket a dedicated group first; the agent user must not be given a root shell or that user's other keys.",
        engine.socket.display(),
        metadata.uid()
    );
    // Parent directories such as /run/user/UID are 0700, which group membership cannot cross.
    let mut parent = engine.socket.parent();
    while let Some(directory) = parent {
        let mode = std::fs::metadata(directory)?.mode();
        ensure!(
            mode & 0o001 != 0,
            "{} cannot be reached by other users because {} is private.",
            engine.socket.display(),
            directory.display()
        );
        parent = directory.parent();
    }
    let group = run(Command::new("getent")
        .arg("group")
        .arg(metadata.gid().to_string()))?;
    let group = group.split(':').next().unwrap_or_default().to_owned();
    ensure!(!group.is_empty(), "Cannot name the socket's group");
    ensure!(
        !PRIVILEGED_GROUPS.contains(&group.as_str()),
        "{} belongs to group {group}, which grants far more than engine access. Give the socket a dedicated group such as docker first.",
        engine.socket.display()
    );
    Ok(Some(group))
}

pub(super) fn atomic_write(
    path: &Path,
    text: &str,
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<()> {
    let directory = path.parent().context("Path has no parent directory")?;
    let temporary = tempfile::Builder::new()
        .prefix(".tha-setup-")
        .tempfile_in(directory)?;
    temporary.as_file().write_all(text.as_bytes())?;
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(mode))?;
    if let Some((uid, gid)) = owner {
        std::os::unix::fs::fchown(temporary.as_file(), Some(uid), Some(gid))?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(())
}

fn refuse_symlink(path: &Path) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        ensure!(
            !metadata.file_type().is_symlink(),
            "{} must not be a symlink",
            path.display()
        );
    }
    Ok(())
}

fn prepare_key(account: &Account, line: &str) -> Result<String> {
    let directory = account.home.join(".ssh");
    refuse_symlink(&directory)?;
    let path = directory.join("authorized_keys");
    refuse_symlink(&path)?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("Cannot read authorized_keys"),
    };
    merge_authorized_keys(&existing, line)
}

fn install_key(account: &Account, line: &str) -> Result<()> {
    // Revalidate immediately before installation, without modifying invalid existing files.
    let merged = prepare_key(account, line)?;
    let directory = account.home.join(".ssh");
    if !directory.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    std::os::unix::fs::chown(&directory, Some(account.uid), Some(account.gid))?;
    let path = directory.join("authorized_keys");
    atomic_write(&path, &merged, 0o600, Some((account.uid, account.gid)))
}

/// Every known socket on this server, added to what is already exposed. An existing entry
/// is never dropped, so a rerun cannot silently take an engine away.
pub fn merge_found(existing: &[AgentEngine], found: Vec<AgentEngine>) -> Vec<AgentEngine> {
    let mut engines = existing.to_vec();
    for engine in found {
        if !engines.iter().any(|known| known.name == engine.name) {
            engines.push(engine);
        }
    }
    engines
}

fn find_engines() -> Vec<AgentEngine> {
    KNOWN_SOCKETS
        .iter()
        .filter(|(_, socket)| {
            std::fs::metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket())
        })
        .map(|(name, socket)| AgentEngine {
            name: (*name).to_owned(),
            socket: PathBuf::from(socket),
        })
        .collect()
}

/// Root can already reach every socket, and its keys are not ours to rewrite.
fn finish_root_setup(policy: &Path, config: &AgentConfig) -> Result<()> {
    write_policy(policy, config)?;
    println!(
        "Agent policy written. tha's traffic goes through the allowlist (delete {}, exec {}, images {}, downloads {}, server downloads {}, compose {}, compose edit {}, uploads {}).",
        if config.allow_delete { "on" } else { "off" },
        if config.allow_exec { "on" } else { "off" },
        if config.allow_images { "on" } else { "off" },
        if config.allow_download { "on" } else { "off" },
        if config.allow_server_download {
            "on"
        } else {
            "off"
        },
        if config.allow_compose { "on" } else { "off" },
        if config.allow_compose_edit {
            "on"
        } else {
            "off"
        },
        if config.allow_upload { "on" } else { "off" },
    );
    println!(
        "Root's SSH keys are not restricted: anyone holding one still has full root access. Use a dedicated user to limit the key itself."
    );
    Ok(())
}

fn write_policy(policy: &Path, config: &AgentConfig) -> Result<()> {
    let parent = policy.parent().context("Policy path has no parent")?;
    if !parent.exists() {
        std::fs::DirBuilder::new().mode(0o755).create(parent)?;
    }
    atomic_write(policy, &toml::to_string(config)?, 0o644, Some((0, 0)))
}

/// A rerun without either option leaves an existing webhook alone, like the other opt-ins.
fn apply_webhook(
    path: &Path,
    url: Option<&str>,
    remove: bool,
    owner: Option<(u32, u32)>,
) -> Result<()> {
    if remove {
        match std::fs::remove_file(path) {
            Ok(()) => println!("Alert webhook removed."),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    } else if let Some(url) = url {
        super::alerts::validate_webhook(url)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(path, &format!("{url}\n"), 0o600, owner)?;
        println!("Alert webhook saved. This server now sends alerts to it over HTTPS.");
    }
    Ok(())
}

pub fn run_setup(options: &Options) -> Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "tha-agent setup supports Linux servers only"
    );
    // SAFETY: geteuid has no preconditions.
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "tha-agent setup must run as root"
    );
    apply_webhook(
        Path::new(super::alerts::WEBHOOK_PATH),
        options.alert_webhook.as_deref(),
        options.no_alert_webhook,
        Some((0, 0)),
    )?;
    let root = options.user == "root";
    ensure!(
        root || valid_user(&options.user),
        "Choose a user name made of lowercase letters, digits, - and _"
    );
    let key = match (&options.key, root) {
        (Some(_), true) => bail!(
            "Root's keys are not changed by setup, so --key does not apply. Use a dedicated user to restrict a key."
        ),
        (Some(key), false) => Some(parse_public_key(key)?),
        (None, true) => None,
        (None, false) => bail!("--key is required for a dedicated user"),
    };
    let executable = std::env::current_exe()?
        .canonicalize()
        .context("Cannot resolve the tha binary")?;
    let metadata = std::fs::metadata(&executable)?;
    super::service::validate_executable(&executable)?;
    ensure!(
        metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "{} must be owned by root and not writable by others, because the forced command runs it",
        executable.display()
    );
    let line = key
        .as_ref()
        .map(|key| authorized_line(&executable, key))
        .transpose()?;

    let policy = Path::new(super::CONFIG_PATH);
    let existing = if policy.exists() {
        Some(super::load_config(policy)?)
    } else {
        None
    };
    let requested = options
        .engines
        .iter()
        .map(|engine| parse_engine(engine))
        .collect::<Result<Vec<_>>>()?;
    let explicit = !requested.is_empty();
    let engines = if explicit {
        requested
    } else {
        merge_found(
            existing.as_ref().map_or(&[][..], |config| &config.engines),
            find_engines(),
        )
    };
    let mut config = merge_config(existing, engines, options.allow_delete);
    config.allow_exec |= options.allow_exec;
    config.allow_images |= options.allow_images;
    config.allow_download |= options.allow_download;
    config.allow_server_download |= options.allow_server_download;
    config.allow_compose |= options.allow_compose;
    config.allow_compose_edit |= options.allow_compose_edit;
    config.allow_upload |= options.allow_upload;
    configure_security(&mut config.security, options);
    config.validate()?;
    ensure!(
        !config.engines.is_empty(),
        "No known engine socket was found; pass --engine NAME=/path/to/socket"
    );

    if root {
        // Root is always accepted; adding a root login must not revoke an existing
        // dedicated user's access to the service.
        finish_root_setup(policy, &config)?;
        return super::service::install(&executable);
    }
    let line = line.context("A dedicated user always has a key line")?;
    let account = match account(&options.user)? {
        Some(account) => account,
        None => {
            // A locked password, and a real shell because sshd runs forced commands through it.
            run(Command::new("useradd").args([
                "--create-home",
                "--shell",
                "/bin/sh",
                &options.user,
            ]))
            .context("Cannot create the user; create it yourself and run setup again")?;
            account(&options.user)?.context("The new user could not be found")?
        }
    };
    ensure!(
        account.uid != 0,
        "The agent user must not have root's user ID"
    );
    ensure!(
        !account.shell.ends_with("nologin") && !account.shell.ends_with("false"),
        "User {} has the shell {}, which cannot run sshd's forced command",
        options.user,
        account.shell
    );
    finish_dedicated_setup(options, &account, &line, policy, config)?;
    super::service::install(&executable)
}

fn configure_security(policy: &mut crate::security::agent::Policy, options: &Options) {
    // Normal setup enables the fixed, automatically verified scanner. Keep an
    // explicit opt-out, without making users discover or maintain SHA values.
    policy.enabled = !options.no_security;
    policy.scanner_digest = options.scanner_digest.clone().unwrap_or_default();
    policy.interval_seconds = options.security_interval;
}

fn finish_dedicated_setup(
    options: &Options,
    account: &Account,
    line: &str,
    policy: &Path,
    mut config: AgentConfig,
) -> Result<()> {
    // Reject unsafe existing authentication before touching policy or engine privileges.
    prepare_key(account, line)?;
    let explicit = !options.engines.is_empty();
    // An engine asked for by name must work. One merely found must not block the others, as a
    // root-only Podman socket beside a normal Docker one would.
    let mut reachable = Vec::new();
    let mut groups = Vec::new();
    let mut skipped = None;
    for engine in std::mem::take(&mut config.engines) {
        match socket_group(account, &engine) {
            Ok(group) => {
                if let Some(group) = group
                    && !groups.contains(&group)
                {
                    groups.push(group);
                }
                reachable.push(engine);
            }
            Err(error) if !explicit => {
                println!("Skipping {}: {error:#}", engine.name);
                skipped = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    if reachable.is_empty() {
        return Err(skipped.unwrap_or_else(|| anyhow::anyhow!("No engine could be exposed")));
    }
    config.engines = reachable;
    config.service_uid = account.uid;
    config.service_gid = account.gid;
    // Policy precedes key activation; both must succeed before any group mutation.
    // A later group failure can leave only validated, agent-restricted keys in place.
    write_policy(policy, &config)?;
    install_key(account, line)?;
    for group in groups {
        run(Command::new("usermod").args(["-aG", &group, &options.user]))?;
        println!("Added {} to group {group} for engine access", options.user);
    }

    println!(
        "Agent ready for user {}: {} (delete {}, exec {}, images {}, downloads {}, server downloads {}, compose {}, compose edit {}, uploads {})",
        options.user,
        config
            .engines
            .iter()
            .map(|engine| engine.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        if config.allow_delete { "on" } else { "off" },
        if config.allow_exec { "on" } else { "off" },
        if config.allow_images { "on" } else { "off" },
        if config.allow_download { "on" } else { "off" },
        if config.allow_server_download {
            "on"
        } else {
            "off"
        },
        if config.allow_compose { "on" } else { "off" },
        if config.allow_compose_edit {
            "on"
        } else {
            "off"
        },
        if config.allow_upload { "on" } else { "off" },
    );
    println!(
        "If sshd uses AllowUsers or AllowGroups, also allow {}.",
        options.user
    );
    println!(
        "Do not give {} any other SSH key, password or sudo rule; they would bypass the agent.",
        options.user
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_setup_enables_automatic_scanner_verification_without_a_pin() {
        let mut policy = crate::security::agent::Policy::default();
        let mut options = Options {
            user: "root".into(),
            key: None,
            allow_delete: false,
            allow_exec: false,
            allow_images: false,
            allow_download: false,
            allow_server_download: false,
            allow_compose: false,
            allow_compose_edit: false,
            allow_upload: false,
            alert_webhook: None,
            no_alert_webhook: false,
            engines: vec![],
            scanner_digest: None,
            security_interval: 86400,
            no_security: false,
        };
        configure_security(&mut policy, &options);
        assert!(policy.enabled);
        assert!(policy.scanner_digest.is_empty());
        assert!(policy.validate().is_ok());
        options.no_security = true;
        configure_security(&mut policy, &options);
        assert!(!policy.enabled);
    }

    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBUo4CVCKfmH3UjnNfr0F2hsmKRKEFmCJfNDIyVGxsHq";

    #[test]
    fn a_public_key_loses_its_comment() {
        let key = parse_public_key(&format!("{ED25519} randy@laptop\n")).unwrap();
        let line = authorized_line(Path::new("/usr/local/bin/tha-agent"), &key).unwrap();
        assert_eq!(
            line,
            format!("restrict,command=\"/usr/local/bin/tha-agent\" {ED25519}")
        );
    }

    #[test]
    fn pasted_keys_cannot_add_options_or_lines() {
        for hostile in [
            format!("command=\"sh\" {ED25519}"),
            format!("no-pty {ED25519}"),
            format!("{ED25519}\n{ED25519}"),
            format!("{ED25519}\ncommand=\"sh\" {ED25519}"),
            format!("{ED25519}\rx"),
            "ssh-ed25519".to_owned(),
            "ssh-ed25519 not*base64".to_owned(),
            "ssh-ed25519 AAAA".to_owned(),
            "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIBUo4CVCKfmH3UjnNfr0F2hsmKRKEFmCJfNDIyVGxsHq"
                .to_owned(),
            String::new(),
        ] {
            assert!(parse_public_key(&hostile).is_err(), "{hostile:?}");
        }
    }

    #[test]
    fn binary_paths_needing_quotes_are_refused() {
        let key = parse_public_key(ED25519).unwrap();
        for path in [
            "relative/tha-agent",
            "/opt/my tha",
            "/opt/tha\"; sh",
            "/opt/$(sh)",
        ] {
            assert!(authorized_line(Path::new(path), &key).is_err(), "{path}");
        }
    }

    #[test]
    fn rerunning_with_the_same_key_changes_nothing() {
        let line = format!("restrict,command=\"/usr/local/bin/tha-agent\" {ED25519}");
        let once = merge_authorized_keys("", &line).unwrap();
        assert_eq!(merge_authorized_keys(&once, &line).unwrap(), once);
    }

    #[test]
    fn a_second_agent_key_is_added() {
        let first = format!("restrict,command=\"/usr/local/bin/tha-agent\" {ED25519}\n");
        let second = format!(
            "restrict,command=\"/usr/local/bin/tha-agent\" {}",
            ED25519.replace("xsHq", "xsHr")
        );
        let merged = merge_authorized_keys(&first, &second).unwrap();
        assert_eq!(merged.lines().count(), 2);
    }

    #[test]
    fn unrestricted_existing_keys_stop_setup() {
        let line = format!("restrict,command=\"/usr/local/bin/tha-agent\" {ED25519}");
        for existing in [
            format!("{ED25519} admin@host\n"),
            format!("command=\"/t agent\" {ED25519}\n"),
            format!("restrict {ED25519}\n"),
            format!("# note\n{ED25519}\n"),
        ] {
            assert!(
                merge_authorized_keys(&existing, &line).is_err(),
                "{existing}"
            );
        }
        assert!(merge_authorized_keys("# only a comment\n\n", &line).is_ok());
    }

    #[test]
    fn security_setup_rejects_keys_before_attempting_socket_access() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".ssh")).unwrap();
        let keys = home.path().join(".ssh/authorized_keys");
        std::fs::write(&keys, ED25519).unwrap();
        std::fs::set_permissions(
            home.path().join(".ssh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let account = Account {
            uid: 12345,
            gid: 12345,
            home: home.path().into(),
            shell: "/bin/sh".into(),
        };
        let options = Options {
            user: "tha".into(),
            key: Some(ED25519.into()),
            allow_delete: false,
            allow_exec: false,
            allow_images: false,
            allow_download: false,
            allow_server_download: false,
            allow_compose: false,
            allow_compose_edit: false,
            allow_upload: false,
            alert_webhook: None,
            no_alert_webhook: false,
            engines: vec!["docker=/missing-security-test.sock".into()],
            scanner_digest: None,
            security_interval: 86400,
            no_security: true,
        };
        let config = AgentConfig {
            allow_delete: false,
            engines: vec![parse_engine(&options.engines[0]).unwrap()],
            ..Default::default()
        };
        let line = authorized_line(
            Path::new("/usr/local/bin/tha-agent"),
            &parse_public_key(ED25519).unwrap(),
        )
        .unwrap();
        let policy = home.path().join("policy.toml");
        let error = finish_dedicated_setup(&options, &account, &line, &policy, config).unwrap_err();
        assert!(
            error.to_string().contains("authorized_keys line 1"),
            "{error:#}"
        );
        assert!(!policy.exists());
        assert_eq!(std::fs::read_to_string(keys).unwrap(), ED25519);
        assert_eq!(
            std::fs::metadata(home.path().join(".ssh")).unwrap().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn security_existing_keys_must_use_the_exact_trusted_command() {
        let line = authorized_line(
            Path::new("/usr/local/bin/tha-agent"),
            &parse_public_key(ED25519).unwrap(),
        )
        .unwrap();
        for command in [
            "/bin/sh -i #agent",
            "/tmp/evil-agent",
            "/usr/local/bin/tha-agent;sh #agent",
            "/usr/local/bin/tha-agent --extra #agent",
        ] {
            let existing = format!("restrict,command=\"{command}\" {ED25519}");
            assert!(
                merge_authorized_keys(&existing, &line).is_err(),
                "{command}"
            );
        }
        let extra_option =
            format!("restrict,command=\"/bin/sh\",command=\"/usr/local/bin/tha-agent\" {ED25519}");
        assert!(merge_authorized_keys(&extra_option, &line).is_err());
        let commented = format!("{line} workstation comment\n");
        assert!(merge_authorized_keys(&commented, &line).is_ok());
        for malformed in [
            format!("restrict,command=\"/bin/sh -i #agent\" {ED25519}"),
            format!("restrict,command=\"/usr/local/bin/tha-agent\",pty {ED25519}"),
        ] {
            assert!(merge_authorized_keys("", &malformed).is_err());
        }
    }

    #[test]
    fn reruns_keep_the_existing_policy_unless_told_otherwise() {
        let existing = AgentConfig {
            allow_delete: true,
            allow_exec: true,
            allow_images: false,
            allow_download: false,
            allow_server_download: false,
            allow_compose: false,
            allow_compose_edit: false,
            allow_upload: false,
            engines: vec![AgentEngine {
                name: "docker".into(),
                socket: "/a".into(),
            }],
            ..Default::default()
        };
        let kept = merge_config(Some(existing.clone()), Vec::new(), false);
        assert_eq!(kept, existing);
        let replaced = merge_config(
            Some(existing),
            vec![AgentEngine {
                name: "podman".into(),
                socket: "/b".into(),
            }],
            false,
        );
        assert_eq!(replaced.engines[0].name, "podman");
    }

    fn engine(name: &str) -> AgentEngine {
        AgentEngine {
            name: name.into(),
            socket: format!("/{name}.sock").into(),
        }
    }

    #[test]
    fn found_engines_are_added_without_dropping_existing_ones() {
        let merged = merge_found(
            &[engine("custom")],
            vec![engine("docker"), engine("podman")],
        );
        let names: Vec<_> = merged.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["custom", "docker", "podman"]);
    }

    #[test]
    fn an_engine_already_exposed_keeps_its_configured_socket() {
        let existing = AgentEngine {
            name: "docker".into(),
            socket: "/custom/docker.sock".into(),
        };
        let merged = merge_found(std::slice::from_ref(&existing), vec![engine("docker")]);
        assert_eq!(merged, [existing]);
    }

    #[test]
    fn root_and_odd_user_names_are_refused() {
        for name in ["root", "", "-o", "a b", "a;b", "A", "../x"] {
            assert!(!valid_user(name), "{name}");
        }
        assert!(valid_user("tha"));
        assert!(valid_user("tha-agent"));
    }

    #[test]
    fn engine_arguments_are_validated() {
        assert_eq!(parse_engine("docker=/s").unwrap().name, "docker");
        for hostile in ["docker", "list=/s", "docker=relative", "a b=/s"] {
            assert!(parse_engine(hostile).is_err(), "{hostile}");
        }
    }

    #[test]
    fn the_webhook_is_saved_privately_kept_on_reruns_and_removed_on_request() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("alert-webhook");
        assert!(apply_webhook(&path, Some("http://insecure"), false, None).is_err());
        assert!(!path.exists());
        apply_webhook(&path, Some("https://hooks.example/a"), false, None).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        apply_webhook(&path, None, false, None).unwrap();
        assert!(path.exists(), "A rerun without the option keeps it");
        apply_webhook(&path, None, true, None).unwrap();
        assert!(!path.exists());
        apply_webhook(&path, None, true, None).unwrap();
    }
}
