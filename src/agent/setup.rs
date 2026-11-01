//! One-time, root-run server setup for `tha-agent`. It only touches the dedicated user, that
//! user's `authorized_keys`, and the agent policy file.
use super::{AgentConfig, AgentEngine};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{BufRead, IsTerminal, Write},
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
    /// `NAME=/path/to/socket`. When empty, a terminal is asked or an existing policy is kept.
    pub engines: Vec<String>,
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

fn valid_user(name: &str) -> bool {
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

fn is_agent_line(line: &str) -> bool {
    line.starts_with("restrict,command=\"") && line.contains("agent\" ")
}

/// New `authorized_keys` content. Any other active line would be a way around the agent, so
/// setup stops and names it instead of leaving a key that looks restricted but is not.
pub fn merge_authorized_keys(existing: &str, line: &str) -> Result<String> {
    for (number, current) in existing.lines().enumerate() {
        let current = current.trim();
        if !current.is_empty() && !current.starts_with('#') && !is_agent_line(current) {
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
        engines: if engines.is_empty() {
            existing.engines
        } else {
            engines
        },
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

/// Socket access decided from its real owner: rootful Podman and rootless engines usually
/// cannot be reached through group membership, and saying so beats an agent that always fails.
fn grant_socket(user: &str, account: &Account, engine: &AgentEngine) -> Result<()> {
    let metadata = std::fs::metadata(&engine.socket)
        .with_context(|| format!("Cannot find {}", engine.socket.display()))?;
    ensure!(
        metadata.file_type().is_socket(),
        "{} is not a Unix socket",
        engine.socket.display()
    );
    if metadata.uid() == account.uid || metadata.gid() == account.gid {
        return Ok(());
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
    run(Command::new("usermod").args(["-aG", &group, user]))?;
    println!(
        "Added {user} to group {group} for {}",
        engine.socket.display()
    );
    Ok(())
}

fn atomic_write(path: &Path, text: &str, mode: u32, owner: Option<(u32, u32)>) -> Result<()> {
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

fn install_key(account: &Account, line: &str) -> Result<()> {
    let directory = account.home.join(".ssh");
    refuse_symlink(&directory)?;
    if !directory.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    std::os::unix::fs::chown(&directory, Some(account.uid), Some(account.gid))?;
    let path = directory.join("authorized_keys");
    refuse_symlink(&path)?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("Cannot read authorized_keys"),
    };
    let merged = merge_authorized_keys(&existing, line)?;
    atomic_write(&path, &merged, 0o600, Some((account.uid, account.gid)))
}

fn ask_for_engines(existing: &AgentConfig) -> Result<Vec<AgentEngine>> {
    if !existing.engines.is_empty() {
        return Ok(Vec::new());
    }
    let available: Vec<_> = KNOWN_SOCKETS
        .iter()
        .filter(|(_, socket)| {
            std::fs::metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket())
        })
        .collect();
    ensure!(
        !available.is_empty(),
        "No known engine socket was found; pass --engine NAME=/path/to/socket"
    );
    ensure!(
        std::io::stdin().is_terminal(),
        "Pass --engine NAME=/path/to/socket when running without a terminal"
    );
    let mut chosen = Vec::new();
    for (name, socket) in available {
        print!("Expose {name} ({socket}) through the agent? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if matches!(answer.trim(), "y" | "Y" | "yes") {
            chosen.push(AgentEngine {
                name: (*name).to_owned(),
                socket: PathBuf::from(socket),
            });
        }
    }
    Ok(chosen)
}

/// Root can already reach every socket, and its keys are not ours to rewrite.
fn finish_root_setup(policy: &Path, config: &AgentConfig) -> Result<()> {
    write_policy(policy, config)?;
    println!(
        "Agent policy written. tha's traffic goes through the allowlist (delete {}).",
        if config.allow_delete { "on" } else { "off" }
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
    let engines = if requested.is_empty() {
        ask_for_engines(&existing.clone().unwrap_or_default())?
    } else {
        requested
    };
    let config = merge_config(existing, engines, options.allow_delete);
    config.validate()?;
    ensure!(
        !config.engines.is_empty(),
        "No engines were chosen, so the agent would have nothing to serve"
    );

    if root {
        return finish_root_setup(policy, &config);
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
    for engine in &config.engines {
        grant_socket(&options.user, &account, engine)?;
    }
    // Written last: the key must not start working before the policy it depends on exists.
    write_policy(policy, &config)?;
    install_key(&account, &line)?;

    println!(
        "Agent ready for user {}: {} (delete {})",
        options.user,
        config
            .engines
            .iter()
            .map(|engine| engine.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        if config.allow_delete { "on" } else { "off" }
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
        let line = format!("restrict,command=\"/t agent\" {ED25519}");
        let once = merge_authorized_keys("", &line).unwrap();
        assert_eq!(merge_authorized_keys(&once, &line).unwrap(), once);
    }

    #[test]
    fn a_second_agent_key_is_added() {
        let first = format!("restrict,command=\"/t agent\" {ED25519}\n");
        let second = "restrict,command=\"/t agent\" ssh-ed25519 OTHER";
        let merged = merge_authorized_keys(&first, second).unwrap();
        assert_eq!(merged.lines().count(), 2);
    }

    #[test]
    fn unrestricted_existing_keys_stop_setup() {
        let line = format!("restrict,command=\"/t agent\" {ED25519}");
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
    fn reruns_keep_the_existing_policy_unless_told_otherwise() {
        let existing = AgentConfig {
            allow_delete: true,
            engines: vec![AgentEngine {
                name: "docker".into(),
                socket: "/a".into(),
            }],
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
}
