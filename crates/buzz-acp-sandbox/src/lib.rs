use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const DEFAULT_REAL_ACP: &str = "buzz-acp";
const DEFAULT_BWRAP: &str = "bwrap";
const SANDBOX_HOME: &str = "/home/buzz";
const SANDBOX_TMP: &str = "/tmp";
const REAL_ACP_TARGET: &str = "/run/buzz/bin/buzz-acp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    Auto,
    Required,
    Disabled,
}

impl SandboxMode {
    pub fn parse(raw: &str) -> Result<Self, SandboxError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            "disabled" => Ok(Self::Disabled),
            other => Err(SandboxError::InvalidMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub access: BindAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    pub root: PathBuf,
    pub sandbox_id: String,
    pub real_acp: PathBuf,
    pub bwrap: PathBuf,
    pub cwd: PathBuf,
    pub extra_binds: Vec<BindMount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub linux: bool,
    pub bwrap_available: bool,
}

impl Platform {
    pub fn current(bwrap: &Path) -> Self {
        Self {
            linux: cfg!(target_os = "linux"),
            bwrap_available: command_available(bwrap),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub dirs_to_create: Vec<PathBuf>,
    pub sandboxed: bool,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    InvalidMode(String),
    InvalidBind(String),
    MissingHome,
    InvalidCwd(String),
    MissingRealAcp(String),
    MissingBubblewrap(PathBuf),
    UnsupportedPlatform,
    Io { path: PathBuf, message: String },
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMode(mode) => write!(
                f,
                "invalid BUZZ_SANDBOX_MODE {mode:?}; expected auto, required, or disabled"
            ),
            Self::InvalidBind(bind) => write!(
                f,
                "invalid BUZZ_SANDBOX_BIND entry {bind:?}; expected /source:ro, /source:rw, or /source=/target:ro"
            ),
            Self::MissingHome => write!(
                f,
                "HOME is not set; set BUZZ_SANDBOX_ROOT to choose a sandbox root explicitly"
            ),
            Self::InvalidCwd(cwd) => write!(
                f,
                "invalid BUZZ_SANDBOX_CWD {cwd:?}; expected an absolute sandbox path"
            ),
            Self::MissingRealAcp(command) => {
                write!(f, "could not locate real ACP harness {command:?}")
            }
            Self::MissingBubblewrap(path) => {
                write!(f, "Bubblewrap command not found: {}", path.display())
            }
            Self::UnsupportedPlatform => write!(
                f,
                "buzz-acp-sandbox requires Linux unless BUZZ_SANDBOX_MODE=auto or disabled"
            ),
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

impl Error for SandboxError {}

impl SandboxConfig {
    pub fn from_env() -> Result<Self, SandboxError> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    pub fn from_lookup<F>(get: F) -> Result<Self, SandboxError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mode = SandboxMode::parse(get("BUZZ_SANDBOX_MODE").as_deref().unwrap_or("auto"))?;
        let root = match get("BUZZ_SANDBOX_ROOT") {
            Some(root) if !root.trim().is_empty() => PathBuf::from(root),
            _ => default_sandbox_root(&get)?,
        };
        let sandbox_id = sandbox_id(&get);
        let real_acp = resolve_real_acp(get("BUZZ_REAL_ACP"), get("PATH"))?;
        let bwrap = PathBuf::from(
            get("BUZZ_BWRAP")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_BWRAP.to_string()),
        );
        let cwd = match get("BUZZ_SANDBOX_CWD").filter(|value| !value.trim().is_empty()) {
            Some(cwd) if Path::new(&cwd).is_absolute() => PathBuf::from(cwd),
            Some(cwd) => return Err(SandboxError::InvalidCwd(cwd)),
            None => PathBuf::from(SANDBOX_HOME),
        };
        let extra_binds = parse_bind_list(get("BUZZ_SANDBOX_BIND").as_deref().unwrap_or(""))?;

        Ok(Self {
            mode,
            root,
            sandbox_id,
            real_acp,
            bwrap,
            cwd,
            extra_binds,
        })
    }
}

pub fn build_launch_plan(
    config: &SandboxConfig,
    acp_args: &[String],
    platform: Platform,
) -> Result<LaunchPlan, SandboxError> {
    match config.mode {
        SandboxMode::Disabled => {
            return Ok(direct_plan(
                config,
                acp_args,
                Some("sandbox disabled".to_string()),
            ));
        }
        SandboxMode::Auto if !platform.linux => {
            return Ok(direct_plan(
                config,
                acp_args,
                Some("sandbox unavailable on this platform".to_string()),
            ));
        }
        SandboxMode::Auto if !platform.bwrap_available => {
            return Ok(direct_plan(
                config,
                acp_args,
                Some("Bubblewrap not found".to_string()),
            ));
        }
        SandboxMode::Required if !platform.linux => return Err(SandboxError::UnsupportedPlatform),
        SandboxMode::Required if !platform.bwrap_available => {
            return Err(SandboxError::MissingBubblewrap(config.bwrap.clone()));
        }
        SandboxMode::Auto | SandboxMode::Required => {}
    }

    let agent_root = config.root.join(&config.sandbox_id);
    let host_home = agent_root.join("home");
    let host_tmp = agent_root.join("tmp");
    let mut dirs_to_create = vec![host_home.clone(), host_tmp.clone()];

    let mut args = vec![
        "--die-with-parent".to_string(),
        "--unshare-user".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
        "--unshare-uts".to_string(),
        "--new-session".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--dir".to_string(),
        "/run".to_string(),
        "--dir".to_string(),
        "/run/buzz".to_string(),
        "--dir".to_string(),
        "/run/buzz/bin".to_string(),
        "--dir".to_string(),
        "/home".to_string(),
        "--bind".to_string(),
        path_arg(&host_home),
        SANDBOX_HOME.to_string(),
        "--bind".to_string(),
        path_arg(&host_tmp),
        SANDBOX_TMP.to_string(),
        "--setenv".to_string(),
        "HOME".to_string(),
        SANDBOX_HOME.to_string(),
        "--setenv".to_string(),
        "TMPDIR".to_string(),
        SANDBOX_TMP.to_string(),
        "--chdir".to_string(),
        path_arg(&config.cwd),
    ];

    add_default_ro_binds(&mut args);
    add_real_acp_bind(&mut args, &config.real_acp);
    for bind in &config.extra_binds {
        add_extra_bind(&mut args, bind);
    }

    args.push("--".to_string());
    args.push(REAL_ACP_TARGET.to_string());
    args.extend(acp_args.iter().cloned());

    dirs_to_create.sort();
    dirs_to_create.dedup();

    Ok(LaunchPlan {
        program: config.bwrap.clone(),
        args,
        dirs_to_create,
        sandboxed: true,
        fallback_reason: None,
    })
}

pub fn create_plan_dirs(plan: &LaunchPlan) -> Result<(), SandboxError> {
    for dir in &plan.dirs_to_create {
        fs::create_dir_all(dir).map_err(|err| SandboxError::Io {
            path: dir.clone(),
            message: err.to_string(),
        })?;
    }
    Ok(())
}

pub fn parse_bind_list(raw: &str) -> Result<Vec<BindMount>, SandboxError> {
    let mut binds = Vec::new();
    for entry in raw
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((path, mode)) = entry.rsplit_once(':') else {
            return Err(SandboxError::InvalidBind(entry.to_string()));
        };
        if path.trim().is_empty() {
            return Err(SandboxError::InvalidBind(entry.to_string()));
        }
        let (source, target) = match path.split_once('=') {
            Some((source, target)) => (source, target),
            None => (path, path),
        };
        if source.trim().is_empty()
            || target.trim().is_empty()
            || !Path::new(source).is_absolute()
            || !Path::new(target).is_absolute()
        {
            return Err(SandboxError::InvalidBind(entry.to_string()));
        }
        let access = match mode {
            "ro" => BindAccess::ReadOnly,
            "rw" => BindAccess::ReadWrite,
            _ => return Err(SandboxError::InvalidBind(entry.to_string())),
        };
        binds.push(BindMount {
            source: PathBuf::from(source),
            target: PathBuf::from(target),
            access,
        });
    }
    Ok(binds)
}

fn direct_plan(
    config: &SandboxConfig,
    acp_args: &[String],
    fallback_reason: Option<String>,
) -> LaunchPlan {
    LaunchPlan {
        program: config.real_acp.clone(),
        args: acp_args.to_vec(),
        dirs_to_create: Vec::new(),
        sandboxed: false,
        fallback_reason,
    }
}

fn default_sandbox_root<F>(get: &F) -> Result<PathBuf, SandboxError>
where
    F: Fn(&str) -> Option<String>,
{
    let home = get("HOME").filter(|value| !value.trim().is_empty());
    home.map(|home| PathBuf::from(home).join(".config/buzz/sandboxes"))
        .ok_or(SandboxError::MissingHome)
}

fn sandbox_id<F>(get: &F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    for key in [
        "BUZZ_SANDBOX_ID",
        "BUZZ_ACP_AGENT_OWNER",
        "BUZZ_MANAGED_AGENT",
        "BUZZ_AGENT_NAME",
    ] {
        if let Some(value) = get(key).filter(|value| !value.trim().is_empty()) {
            return safe_component(&value);
        }
    }

    if let Some(secret) = get("BUZZ_PRIVATE_KEY")
        .or_else(|| get("BUZZ_ACP_PRIVATE_KEY"))
        .filter(|value| !value.trim().is_empty())
    {
        return format!("agent-{:016x}", stable_hash(&secret));
    }

    "default".to_string()
}

fn resolve_real_acp(
    configured: Option<String>,
    path: Option<String>,
) -> Result<PathBuf, SandboxError> {
    let raw = configured
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REAL_ACP.to_string());
    let command = PathBuf::from(&raw);
    if command.components().count() > 1 || command.is_absolute() {
        return if command.exists() {
            Ok(command)
        } else {
            Err(SandboxError::MissingRealAcp(raw))
        };
    }

    find_on_path(&raw, path.as_deref()).ok_or(SandboxError::MissingRealAcp(raw))
}

fn command_available(command: &Path) -> bool {
    if command.components().count() > 1 || command.is_absolute() {
        return is_executable(command);
    }
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    find_on_path_os(command.as_os_str(), &path).is_some()
}

fn find_on_path(command: &str, path: Option<&str>) -> Option<PathBuf> {
    let path = path?;
    find_on_path_os(OsStr::new(command), OsStr::new(path))
}

fn find_on_path_os(command: &OsStr, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn add_default_ro_binds(args: &mut Vec<String>) {
    for path in [
        "/usr",
        "/bin",
        "/lib",
        "/lib64",
        "/etc",
        "/nix",
        "/run/current-system",
        "/run/opengl-driver",
        "/run/systemd/resolve",
        "/run/resolvconf",
    ] {
        let path = Path::new(path);
        if path.exists() {
            add_parent_dirs(args, path);
            args.push("--ro-bind".to_string());
            args.push(path_arg(path));
            args.push(path_arg(path));
        }
    }
}

fn add_real_acp_bind(args: &mut Vec<String>, real_acp: &Path) {
    args.push("--ro-bind".to_string());
    args.push(path_arg(real_acp));
    args.push(REAL_ACP_TARGET.to_string());
}

fn add_extra_bind(args: &mut Vec<String>, bind: &BindMount) {
    add_parent_dirs(args, &bind.target);
    args.push(match bind.access {
        BindAccess::ReadOnly => "--ro-bind".to_string(),
        BindAccess::ReadWrite => "--bind".to_string(),
    });
    args.push(path_arg(&bind.source));
    args.push(path_arg(&bind.target));
}

fn add_parent_dirs(args: &mut Vec<String>, path: &Path) {
    let mut ancestors: Vec<&Path> = path
        .ancestors()
        .skip(1)
        .take_while(|ancestor| *ancestor != Path::new("/"))
        .collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if ancestor == Path::new(SANDBOX_HOME) {
            continue;
        }
        let dir = path_arg(ancestor);
        if !has_dir_arg(args, &dir) {
            args.push("--dir".to_string());
            args.push(dir);
        }
    }
}

fn has_dir_arg(args: &[String], dir: &str) -> bool {
    args.windows(2)
        .any(|window| window[0] == "--dir" && window[1] == dir)
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn safe_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "default".to_string()
    } else {
        out.to_string()
    }
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config_with(entries: &[(&str, &str)]) -> SandboxConfig {
        let mut env = HashMap::from([
            ("HOME".to_string(), "/home/alice".to_string()),
            ("PATH".to_string(), "/bin:/usr/bin".to_string()),
            ("BUZZ_BWRAP".to_string(), "/usr/bin/bwrap".to_string()),
            (
                "BUZZ_SANDBOX_ROOT".to_string(),
                "/tmp/buzz-sandboxes".to_string(),
            ),
            ("BUZZ_SANDBOX_ID".to_string(), "agent-1".to_string()),
        ]);
        for (key, value) in entries {
            env.insert((*key).to_string(), (*value).to_string());
        }
        SandboxConfig {
            mode: SandboxMode::parse(
                env.get("BUZZ_SANDBOX_MODE")
                    .map(String::as_str)
                    .unwrap_or("auto"),
            )
            .expect("mode"),
            root: PathBuf::from(env.get("BUZZ_SANDBOX_ROOT").expect("sandbox root")),
            sandbox_id: env
                .get("BUZZ_SANDBOX_ID")
                .filter(|value| !value.is_empty())
                .map(|value| safe_component(value))
                .unwrap_or_else(|| {
                    format!(
                        "agent-{:016x}",
                        stable_hash(env.get("BUZZ_PRIVATE_KEY").expect("secret"))
                    )
                }),
            real_acp: PathBuf::from("/usr/bin/buzz-acp"),
            bwrap: PathBuf::from(env.get("BUZZ_BWRAP").expect("bwrap")),
            cwd: env
                .get("BUZZ_SANDBOX_CWD")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(SANDBOX_HOME)),
            extra_binds: parse_bind_list(
                env.get("BUZZ_SANDBOX_BIND")
                    .map(String::as_str)
                    .unwrap_or(""),
            )
            .expect("binds"),
        }
    }

    #[test]
    fn command_generation_includes_expected_bwrap_flags() {
        let config = config_with(&[]);
        let plan = build_launch_plan(
            &config,
            &["--respond-to".into(), "anyone".into()],
            Platform {
                linux: true,
                bwrap_available: true,
            },
        )
        .expect("plan");

        assert!(plan.sandboxed);
        assert_eq!(plan.program, PathBuf::from("/usr/bin/bwrap"));
        assert!(plan.args.contains(&"--unshare-pid".to_string()));
        assert!(plan.args.contains(&"--unshare-ipc".to_string()));
        assert_contains_triplet(
            &plan.args,
            "--bind",
            "/tmp/buzz-sandboxes/agent-1/home",
            SANDBOX_HOME,
        );
        assert_contains_triplet(&plan.args, "--setenv", "HOME", SANDBOX_HOME);
        assert_contains_triplet(&plan.args, "--setenv", "TMPDIR", SANDBOX_TMP);
        assert_contains_pair(&plan.args, "--chdir", SANDBOX_HOME);
        assert_contains_triplet(
            &plan.args,
            "--ro-bind",
            "/usr/bin/buzz-acp",
            REAL_ACP_TARGET,
        );

        let separator = plan.args.iter().position(|arg| arg == "--").expect("--");
        assert_eq!(plan.args[separator + 1], REAL_ACP_TARGET);
        assert_eq!(&plan.args[separator + 2..], ["--respond-to", "anyone"]);
    }

    #[test]
    fn secrets_are_not_added_to_argv() {
        let config = config_with(&[
            ("BUZZ_SANDBOX_ID", ""),
            ("BUZZ_ACP_AGENT_OWNER", ""),
            ("BUZZ_MANAGED_AGENT", ""),
            ("BUZZ_AGENT_NAME", ""),
            ("BUZZ_PRIVATE_KEY", "nsec1supersecret"),
        ]);
        let plan = build_launch_plan(
            &config,
            &[],
            Platform {
                linux: true,
                bwrap_available: true,
            },
        )
        .expect("plan");
        let argv = plan.args.join(" ");
        assert!(!argv.contains("nsec1supersecret"));
        assert!(!argv.contains("BUZZ_PRIVATE_KEY"));
        assert!(config.sandbox_id.starts_with("agent-"));
    }

    #[test]
    fn modes_have_distinct_behavior() {
        let disabled = config_with(&[("BUZZ_SANDBOX_MODE", "disabled")]);
        let disabled_plan = build_launch_plan(
            &disabled,
            &["models".into()],
            Platform {
                linux: true,
                bwrap_available: true,
            },
        )
        .expect("disabled plan");
        assert!(!disabled_plan.sandboxed);
        assert_eq!(disabled_plan.program, PathBuf::from("/usr/bin/buzz-acp"));
        assert_eq!(disabled_plan.args, ["models"]);

        let auto = config_with(&[("BUZZ_SANDBOX_MODE", "auto")]);
        let auto_plan = build_launch_plan(
            &auto,
            &[],
            Platform {
                linux: true,
                bwrap_available: false,
            },
        )
        .expect("auto plan");
        assert!(!auto_plan.sandboxed);
        assert_eq!(
            auto_plan.fallback_reason.as_deref(),
            Some("Bubblewrap not found")
        );

        let required = config_with(&[("BUZZ_SANDBOX_MODE", "required")]);
        let required_error = build_launch_plan(
            &required,
            &[],
            Platform {
                linux: true,
                bwrap_available: false,
            },
        )
        .expect_err("required mode should fail");
        assert!(matches!(required_error, SandboxError::MissingBubblewrap(_)));
    }

    #[test]
    fn bind_parser_accepts_ro_rw_and_targeted_paths() {
        assert_eq!(
            parse_bind_list("/repo:ro,/work:rw,/host/auth.json=/home/buzz/.codex/auth.json:ro")
                .expect("binds"),
            vec![
                BindMount {
                    source: PathBuf::from("/repo"),
                    target: PathBuf::from("/repo"),
                    access: BindAccess::ReadOnly,
                },
                BindMount {
                    source: PathBuf::from("/work"),
                    target: PathBuf::from("/work"),
                    access: BindAccess::ReadWrite,
                },
                BindMount {
                    source: PathBuf::from("/host/auth.json"),
                    target: PathBuf::from("/home/buzz/.codex/auth.json"),
                    access: BindAccess::ReadOnly,
                },
            ]
        );
    }

    #[test]
    fn sandbox_cwd_can_target_a_bound_workspace() {
        let config = config_with(&[
            ("BUZZ_SANDBOX_CWD", "/workspace/buzz"),
            ("BUZZ_SANDBOX_BIND", "/host/buzz=/workspace/buzz:rw"),
        ]);
        let plan = build_launch_plan(
            &config,
            &[],
            Platform {
                linux: true,
                bwrap_available: true,
            },
        )
        .expect("plan");

        assert_contains_pair(&plan.args, "--chdir", "/workspace/buzz");
        assert_contains_triplet(&plan.args, "--bind", "/host/buzz", "/workspace/buzz");
    }

    #[test]
    fn sandbox_cwd_must_be_absolute() {
        let err = SandboxConfig::from_lookup(|key| match key {
            "HOME" => Some("/home/alice".to_string()),
            "PATH" => Some("/bin:/usr/bin".to_string()),
            "BUZZ_REAL_ACP" => Some("/bin/sh".to_string()),
            "BUZZ_SANDBOX_CWD" => Some("relative".to_string()),
            _ => None,
        })
        .expect_err("relative cwd should fail");

        assert!(matches!(err, SandboxError::InvalidCwd(_)));
    }

    #[test]
    fn bind_parser_rejects_malformed_inputs() {
        for raw in [
            "relative:ro",
            "/repo",
            "/repo:bad",
            ":ro",
            "/repo:",
            "/source=relative:ro",
            "relative=/target:ro",
            "/source=:ro",
        ] {
            assert!(
                matches!(parse_bind_list(raw), Err(SandboxError::InvalidBind(_))),
                "{raw:?} should be invalid"
            );
        }
    }

    fn assert_contains_triplet(args: &[String], a: &str, b: &str, c: &str) {
        assert!(
            args.windows(3)
                .any(|window| window[0] == a && window[1] == b && window[2] == c),
            "missing [{a:?}, {b:?}, {c:?}] in {args:?}"
        );
    }

    fn assert_contains_pair(args: &[String], a: &str, b: &str) {
        assert!(
            args.windows(2)
                .any(|window| window[0] == a && window[1] == b),
            "missing [{a:?}, {b:?}] in {args:?}"
        );
    }
}
