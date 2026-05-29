use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::indexer::IndexerEntry;

/// Per-project runtime toolchain configuration.
///
/// These paths are used only for SCIP-IO child processes. They do not mutate
/// the user's persistent PATH, JAVA_HOME, or shell profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolchainsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go: Option<ToolchainHomeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<ToolchainHomeConfig>,
}

impl ToolchainsConfig {
    pub fn is_empty(&self) -> bool {
        self.go.is_none() && self.java.is_none()
    }

    fn home_for(&self, kind: ToolchainKind) -> Option<&Path> {
        match kind {
            ToolchainKind::Go => self.go.as_ref(),
            ToolchainKind::Java => self.java.as_ref(),
        }
        .and_then(|config| config.home.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolchainHomeConfig {
    pub home: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolchainKind {
    Go,
    Java,
}

impl ToolchainKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Java => "java",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Go => "Go",
            Self::Java => "Java",
        }
    }

    fn executable_base_name(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Java => "java",
        }
    }

    fn config_table(self) -> &'static str {
        match self {
            Self::Go => "toolchains.go",
            Self::Java => "toolchains.java",
        }
    }

    fn install_hint(self) -> &'static str {
        match self {
            Self::Go => "Install Go or set [toolchains.go].home to the Go installation root",
            Self::Java => {
                "Install a JDK or set [toolchains.java].home to the JDK installation root"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolchainSource {
    ProjectConfig,
    Environment,
    Path,
    CommonLocation,
    Missing,
}

impl ToolchainSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectConfig => "project-config",
            Self::Environment => "environment",
            Self::Path => "path",
            Self::CommonLocation => "common-location",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainEnvironment {
    pub kind: ToolchainKind,
    pub home: Option<PathBuf>,
    pub executable: PathBuf,
    pub prepend_paths: Vec<PathBuf>,
    pub env_vars: BTreeMap<String, OsString>,
}

impl ToolchainEnvironment {
    pub fn apply_to_command(&self, cmd: &mut tokio::process::Command) -> Result<()> {
        if !self.prepend_paths.is_empty() {
            let path = path_with_prepended(&self.prepend_paths, env::var_os("PATH").as_deref())?;
            cmd.env("PATH", path);
        }
        for (name, value) in &self.env_vars {
            cmd.env(name, value);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainPreflight {
    pub kind: ToolchainKind,
    pub available: bool,
    pub source: ToolchainSource,
    pub home: Option<PathBuf>,
    pub executable: Option<PathBuf>,
    pub message: String,
    pub environment: Option<ToolchainEnvironment>,
}

impl ToolchainPreflight {
    fn available(
        kind: ToolchainKind,
        source: ToolchainSource,
        home: Option<PathBuf>,
        executable: PathBuf,
    ) -> Self {
        let bin_dir = executable.parent().map(Path::to_path_buf);
        let mut env_vars = BTreeMap::new();
        if kind == ToolchainKind::Java
            && let Some(home) = &home
        {
            env_vars.insert("JAVA_HOME".to_string(), home.as_os_str().to_os_string());
        }
        let environment = ToolchainEnvironment {
            kind,
            home: home.clone(),
            executable: executable.clone(),
            prepend_paths: bin_dir.into_iter().collect(),
            env_vars,
        };
        let message = match &home {
            Some(home) => format!("{} found at {}", kind.display_name(), home.display()),
            None => format!(
                "{} executable found at {}",
                kind.display_name(),
                executable.display()
            ),
        };
        Self {
            kind,
            available: true,
            source,
            home,
            executable: Some(executable),
            message,
            environment: Some(environment),
        }
    }

    fn missing(kind: ToolchainKind, source: ToolchainSource, message: impl Into<String>) -> Self {
        Self {
            kind,
            available: false,
            source,
            home: None,
            executable: None,
            message: message.into(),
            environment: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolchainResolver {
    path_env: Option<OsString>,
    go_root: Option<OsString>,
    java_home: Option<OsString>,
    common_go_homes: Vec<PathBuf>,
    common_java_homes: Vec<PathBuf>,
}

impl ToolchainResolver {
    pub fn from_current_process() -> Self {
        Self {
            path_env: env::var_os("PATH"),
            go_root: env::var_os("GOROOT"),
            java_home: env::var_os("JAVA_HOME"),
            common_go_homes: default_common_go_homes(),
            common_java_homes: default_common_java_homes(),
        }
    }

    pub fn resolve(&self, kind: ToolchainKind, config: &ToolchainsConfig) -> ToolchainPreflight {
        if let Some(configured_home) = config.home_for(kind) {
            return self
                .resolve_from_home(kind, configured_home, ToolchainSource::ProjectConfig)
                .unwrap_or_else(|| {
                    ToolchainPreflight::missing(
                        kind,
                        ToolchainSource::ProjectConfig,
                        format!(
                            "Configured [{}].home does not contain {}",
                            kind.config_table(),
                            expected_binary_description(kind)
                        ),
                    )
                });
        }

        let env_home = match kind {
            ToolchainKind::Go => self.go_root.as_deref(),
            ToolchainKind::Java => self.java_home.as_deref(),
        };
        if let Some(env_home) = env_home {
            let home = PathBuf::from(env_home);
            if let Some(preflight) =
                self.resolve_from_home(kind, &home, ToolchainSource::Environment)
            {
                return preflight;
            }
        }

        if let Some(preflight) = self.resolve_from_path(kind) {
            return preflight;
        }

        for home in self.common_homes(kind) {
            if let Some(preflight) =
                self.resolve_from_home(kind, home, ToolchainSource::CommonLocation)
            {
                return preflight;
            }
        }

        ToolchainPreflight::missing(
            kind,
            ToolchainSource::Missing,
            format!(
                "{} was not found. {}",
                kind.display_name(),
                kind.install_hint()
            ),
        )
    }

    fn resolve_from_home(
        &self,
        kind: ToolchainKind,
        home: &Path,
        source: ToolchainSource,
    ) -> Option<ToolchainPreflight> {
        let resolved_home = validated_toolchain_home(kind, home)?;
        let bin_dir = resolved_home.join("bin");
        executable_names(kind.executable_base_name())
            .into_iter()
            .map(|name| bin_dir.join(name))
            .find(|candidate| candidate.is_file())
            .map(|executable| {
                ToolchainPreflight::available(kind, source, Some(resolved_home), executable)
            })
    }

    fn resolve_from_path(&self, kind: ToolchainKind) -> Option<ToolchainPreflight> {
        let executable =
            find_executable_on_path(kind.executable_base_name(), self.path_env.as_ref())?;
        let home = match kind {
            ToolchainKind::Go => infer_home_from_executable(&executable),
            // PATH Java is a usable runtime, but shims such as /usr/bin/java do
            // not identify a valid JAVA_HOME. Leave JAVA_HOME untouched unless
            // config/environment/common-location supplied an actual Java home.
            ToolchainKind::Java => None,
        };
        Some(ToolchainPreflight::available(
            kind,
            ToolchainSource::Path,
            home,
            executable,
        ))
    }

    fn common_homes(&self, kind: ToolchainKind) -> &[PathBuf] {
        match kind {
            ToolchainKind::Go => &self.common_go_homes,
            ToolchainKind::Java => &self.common_java_homes,
        }
    }

    #[cfg(test)]
    fn empty_for_tests() -> Self {
        Self {
            path_env: None,
            go_root: None,
            java_home: None,
            common_go_homes: Vec::new(),
            common_java_homes: Vec::new(),
        }
    }

    #[cfg(test)]
    fn with_path_env(mut self, path_env: OsString) -> Self {
        self.path_env = Some(path_env);
        self
    }
}

pub fn required_toolchain_for_indexer(indexer_name: &str) -> Option<ToolchainKind> {
    match indexer_name {
        "scip-go" => Some(ToolchainKind::Go),
        "scip-java" => Some(ToolchainKind::Java),
        _ => None,
    }
}

pub fn toolchain_preflight_for_indexer(
    entry: &IndexerEntry,
    config: &ToolchainsConfig,
) -> Option<ToolchainPreflight> {
    let kind = required_toolchain_for_indexer(&entry.indexer_name)?;
    Some(ToolchainResolver::from_current_process().resolve(kind, config))
}

pub fn require_toolchain_environment_for_indexer(
    entry: &IndexerEntry,
    config: &ToolchainsConfig,
) -> Result<Option<ToolchainEnvironment>> {
    let Some(preflight) = toolchain_preflight_for_indexer(entry, config) else {
        return Ok(None);
    };

    if !preflight.available {
        bail!(
            "{} requires {} on PATH: {}",
            entry.indexer_name,
            preflight.kind.display_name(),
            preflight.message
        );
    }

    Ok(preflight.environment)
}

pub fn path_with_prepended(
    prepend_paths: &[PathBuf],
    existing_path: Option<&OsStr>,
) -> Result<OsString> {
    let mut paths = Vec::with_capacity(prepend_paths.len() + 8);
    paths.extend(prepend_paths.iter().cloned());
    if let Some(existing_path) = existing_path {
        paths.extend(env::split_paths(existing_path));
    }
    env::join_paths(paths).context("Failed to construct child process PATH")
}

fn find_executable_on_path(name: &str, path_env: Option<&OsString>) -> Option<PathBuf> {
    let path_env = path_env?;
    for dir in env::split_paths(path_env) {
        for executable_name in executable_names(name) {
            let candidate = dir.join(executable_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn infer_home_from_executable(executable: &Path) -> Option<PathBuf> {
    let bin_dir = executable.parent()?;
    let bin_name = bin_dir.file_name()?.to_string_lossy();
    if !bin_name.eq_ignore_ascii_case("bin") {
        return None;
    }
    bin_dir.parent().map(Path::to_path_buf)
}

fn validated_toolchain_home(kind: ToolchainKind, home: &Path) -> Option<PathBuf> {
    if toolchain_home_contains_executable(kind, home) {
        return Some(home.to_path_buf());
    }
    if kind == ToolchainKind::Java {
        let bundle_home = home.join("Contents").join("Home");
        if toolchain_home_contains_executable(kind, &bundle_home) {
            return Some(bundle_home);
        }
    }
    None
}

fn toolchain_home_contains_executable(kind: ToolchainKind, home: &Path) -> bool {
    let bin_dir = home.join("bin");
    executable_names(kind.executable_base_name())
        .into_iter()
        .any(|name| bin_dir.join(name).is_file())
}

fn executable_names(base: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{base}.exe"),
            format!("{base}.cmd"),
            format!("{base}.bat"),
            base.to_string(),
        ]
    } else {
        vec![base.to_string()]
    }
}

fn expected_binary_description(kind: ToolchainKind) -> String {
    let names = executable_names(kind.executable_base_name())
        .into_iter()
        .map(|name| format!("bin/{name}"))
        .collect::<Vec<_>>()
        .join(" or ");
    format!("one of {names}")
}

fn default_common_go_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    push_windows_program_file(&mut homes, "ProgramFiles", "Go");
    push_windows_program_file(&mut homes, "ProgramFiles(x86)", "Go");

    if cfg!(target_os = "macos") {
        homes.push(PathBuf::from("/usr/local/go"));
        homes.push(PathBuf::from("/opt/homebrew/opt/go/libexec"));
    } else if cfg!(unix) {
        homes.push(PathBuf::from("/usr/local/go"));
        homes.push(PathBuf::from("/usr/lib/go"));
    }

    homes
}

fn default_common_java_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    for home_var in ["JAVA_HOME"] {
        if let Some(home) = env::var_os(home_var) {
            homes.push(PathBuf::from(home));
        }
    }

    if cfg!(windows) {
        for base in [
            ("ProgramFiles", "Eclipse Adoptium"),
            ("ProgramFiles", "Java"),
            ("ProgramFiles", "Microsoft"),
            ("ProgramFiles", "Amazon Corretto"),
            ("ProgramFiles(x86)", "Java"),
        ] {
            if let Some(root) = env::var_os(base.0) {
                let root = PathBuf::from(root).join(base.1);
                push_child_dirs_with_prefix(&mut homes, &root, &["jdk", "jre"]);
            }
        }
    } else if cfg!(target_os = "macos") {
        push_child_dirs_with_prefix(
            &mut homes,
            &PathBuf::from("/Library/Java/JavaVirtualMachines"),
            &["jdk", "temurin", "zulu", "corretto"],
        );
        homes.push(PathBuf::from("/opt/homebrew/opt/openjdk"));
    } else if cfg!(unix) {
        push_child_dirs_with_prefix(
            &mut homes,
            &PathBuf::from("/usr/lib/jvm"),
            &["java", "jdk", "jre"],
        );
    }

    homes
}

fn push_windows_program_file(homes: &mut Vec<PathBuf>, env_var: &str, child: &str) {
    if cfg!(windows)
        && let Some(root) = env::var_os(env_var)
    {
        homes.push(PathBuf::from(root).join(child));
    }
}

fn push_child_dirs_with_prefix(homes: &mut Vec<PathBuf>, root: &Path, prefixes: &[&str]) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if prefixes.iter().any(|prefix| lower.starts_with(prefix)) {
            homes.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_fake_executable(home: &Path, base: &str) -> PathBuf {
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let executable = bin.join(executable_names(base).remove(0));
        std::fs::write(&executable, "").unwrap();
        executable
    }

    #[test]
    fn configured_go_home_adds_bin_to_child_path() {
        let dir = TempDir::new().unwrap();
        let go_home = dir.path().join("Go");
        let executable = write_fake_executable(&go_home, "go");
        let config = ToolchainsConfig {
            go: Some(ToolchainHomeConfig {
                home: Some(go_home.clone()),
            }),
            java: None,
        };

        let preflight = ToolchainResolver::empty_for_tests().resolve(ToolchainKind::Go, &config);

        assert!(preflight.available);
        assert_eq!(preflight.source, ToolchainSource::ProjectConfig);
        assert_eq!(preflight.home.as_deref(), Some(go_home.as_path()));
        assert_eq!(preflight.executable.as_deref(), Some(executable.as_path()));
        let env = preflight.environment.unwrap();
        assert_eq!(env.prepend_paths, vec![go_home.join("bin")]);
        assert!(!env.env_vars.contains_key("GOROOT"));
    }

    #[test]
    fn configured_java_home_sets_java_home_and_path() {
        let dir = TempDir::new().unwrap();
        let java_home = dir.path().join("jdk-21");
        let executable = write_fake_executable(&java_home, "java");
        let config = ToolchainsConfig {
            go: None,
            java: Some(ToolchainHomeConfig {
                home: Some(java_home.clone()),
            }),
        };

        let preflight = ToolchainResolver::empty_for_tests().resolve(ToolchainKind::Java, &config);

        assert!(preflight.available);
        assert_eq!(preflight.source, ToolchainSource::ProjectConfig);
        assert_eq!(preflight.home.as_deref(), Some(java_home.as_path()));
        assert_eq!(preflight.executable.as_deref(), Some(executable.as_path()));
        let env = preflight.environment.unwrap();
        assert_eq!(env.prepend_paths, vec![java_home.join("bin")]);
        assert_eq!(
            env.env_vars.get("JAVA_HOME"),
            Some(&java_home.as_os_str().to_os_string())
        );
    }

    #[test]
    fn path_discovery_infers_home_from_bin_executable() {
        let dir = TempDir::new().unwrap();
        let go_home = dir.path().join("go");
        let executable = write_fake_executable(&go_home, "go");
        let path = env::join_paths([go_home.join("bin")]).unwrap();

        let preflight = ToolchainResolver::empty_for_tests()
            .with_path_env(path)
            .resolve(ToolchainKind::Go, &ToolchainsConfig::default());

        assert!(preflight.available);
        assert_eq!(preflight.source, ToolchainSource::Path);
        assert_eq!(preflight.home.as_deref(), Some(go_home.as_path()));
        assert_eq!(preflight.executable.as_deref(), Some(executable.as_path()));
    }

    #[test]
    fn path_java_does_not_infer_java_home_from_shims() {
        let dir = TempDir::new().unwrap();
        let shim_root = dir.path().join("usr");
        let executable = write_fake_executable(&shim_root, "java");
        let path = env::join_paths([shim_root.join("bin")]).unwrap();

        let preflight = ToolchainResolver::empty_for_tests()
            .with_path_env(path)
            .resolve(ToolchainKind::Java, &ToolchainsConfig::default());

        assert!(preflight.available);
        assert_eq!(preflight.source, ToolchainSource::Path);
        assert_eq!(preflight.home, None);
        assert_eq!(preflight.executable.as_deref(), Some(executable.as_path()));
        assert!(
            !preflight
                .environment
                .unwrap()
                .env_vars
                .contains_key("JAVA_HOME")
        );
    }

    #[test]
    fn configured_java_bundle_uses_contents_home() {
        let dir = TempDir::new().unwrap();
        let bundle = dir.path().join("Temurin-21.jdk");
        let java_home = bundle.join("Contents").join("Home");
        let executable = write_fake_executable(&java_home, "java");
        let config = ToolchainsConfig {
            go: None,
            java: Some(ToolchainHomeConfig { home: Some(bundle) }),
        };

        let preflight = ToolchainResolver::empty_for_tests().resolve(ToolchainKind::Java, &config);

        assert!(preflight.available);
        assert_eq!(preflight.home.as_deref(), Some(java_home.as_path()));
        assert_eq!(preflight.executable.as_deref(), Some(executable.as_path()));
        assert_eq!(
            preflight.environment.unwrap().env_vars.get("JAVA_HOME"),
            Some(&java_home.as_os_str().to_os_string())
        );
    }

    #[test]
    fn child_path_prepends_toolchain_bin_before_existing_path() {
        let dir = TempDir::new().unwrap();
        let toolchain_bin = dir.path().join("go").join("bin");
        let existing = dir.path().join("existing-bin");
        let existing_path = env::join_paths([existing.clone()]).unwrap();

        let merged = path_with_prepended(
            std::slice::from_ref(&toolchain_bin),
            Some(existing_path.as_os_str()),
        )
        .unwrap();
        let paths = env::split_paths(&merged).collect::<Vec<_>>();

        assert_eq!(paths[0], toolchain_bin);
        assert_eq!(paths[1], existing);
    }

    #[test]
    fn required_toolchains_are_capability_gated_by_indexer() {
        assert_eq!(
            required_toolchain_for_indexer("scip-go"),
            Some(ToolchainKind::Go)
        );
        assert_eq!(
            required_toolchain_for_indexer("scip-java"),
            Some(ToolchainKind::Java)
        );
        assert_eq!(required_toolchain_for_indexer("scip-python"), None);
    }
}
