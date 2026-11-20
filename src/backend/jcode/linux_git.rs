//! After workspace binds, mount order is marker files in `read_only`, the
//! synthetic common-dir skeleton in `hidden_directories`, exact host entries
//! in `read_only_after_masks`, then config overlays. The skeleton has only Git
//! mountpoint placeholders; config pairs are `(source, destination)`, shared
//! before worktree config. Alternates, partial storage, submodules, includes,
//! filters, unknown extensions, and nonstandard layouts are rejected.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Default)]
pub(super) struct GitMounts {
    pub(super) read_only: Vec<PathBuf>,
    pub(super) hidden_directories: Vec<(PathBuf, PathBuf)>,
    pub(super) read_only_after_masks: Vec<PathBuf>,
    pub(super) config_overlays: Vec<(PathBuf, PathBuf)>,
}

pub(super) fn prepare(workspace: &Path, scratch: &Path) -> Result<GitMounts, String> {
    if fs::canonicalize(workspace).map_err(|_| "cannot resolve workspace")? != workspace {
        return Err("workspace must be a canonical path".into());
    }
    let markers = find_repo_markers(workspace)?;
    if markers.is_empty() {
        return Ok(GitMounts::default());
    }
    let scratch = fs::canonicalize(scratch).map_err(|_| "cannot resolve Git scratch directory")?;
    let mut mounts = GitMounts::default();
    for (index, (root, marker)) in markers.iter().enumerate() {
        if scratch.starts_with(root) || root.starts_with(&scratch) {
            return Err("Git scratch directory must be outside every repository root".into());
        }
        let repo_mounts = prepare_one(root, marker, &scratch, index)?;
        for (_, common) in &repo_mounts.hidden_directories {
            if mounts
                .hidden_directories
                .iter()
                .any(|(_, existing)| common.starts_with(existing) || existing.starts_with(common))
            {
                return Err(
                    "overlapping or repeated Git common directories are unsupported".into(),
                );
            }
        }
        mounts.read_only.extend(repo_mounts.read_only);
        mounts
            .hidden_directories
            .extend(repo_mounts.hidden_directories);
        mounts
            .read_only_after_masks
            .extend(repo_mounts.read_only_after_masks);
        mounts.config_overlays.extend(repo_mounts.config_overlays);
    }
    Ok(mounts)
}

fn prepare_one(
    root: &Path,
    marker: &Path,
    scratch: &Path,
    index: usize,
) -> Result<GitMounts, String> {
    // Validate a linked worktree backlink before Git can follow its .git pointer.
    let (git_dir, common_dir, linked) = marker_paths(root, marker)?;
    validate_layout(root, &git_dir, &common_dir, linked)?;
    let config = common_dir.join("config");
    let common_settings = read_settings(&config, scratch, false)?;
    let worktree_config = git_dir.join("config.worktree");
    let worktree_settings = match fs::symlink_metadata(&worktree_config) {
        Ok(metadata) if metadata.file_type().is_file() => {
            Some(read_settings(&worktree_config, scratch, true)?)
        }
        Ok(_) => return Err("worktree config is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("cannot inspect worktree config".into()),
    };
    if rev_path(root, scratch, &["--show-toplevel"])? != root
        || rev_path(root, scratch, &["--absolute-git-dir"])? != git_dir
        || rev_path(
            root,
            scratch,
            &["--path-format=absolute", "--git-common-dir"],
        )? != common_dir
        || rev_value(root, scratch, "--is-inside-work-tree")? != b"true"
        || rev_value(root, scratch, "--is-bare-repository")? != b"false"
    {
        return Err("Git metadata does not match the worktree layout".into());
    }
    if common_settings.object_format.as_deref() == Some("sha256")
        && rev_value(root, scratch, "--show-object-format=storage")? != b"sha256"
    {
        return Err("Git does not support this repository object format".into());
    }
    if git_has_gitlinks(root, scratch)? {
        return Err("Git submodules are unsupported by the metadata skeleton".into());
    }
    if !rev_value(root, scratch, "--shared-index-path")?.is_empty() {
        return Err("Git split indexes are unsupported by the metadata skeleton".into());
    }

    let (skeleton, read_only_after_masks) = make_skeleton(
        scratch,
        &common_dir,
        &git_dir,
        linked,
        worktree_settings.is_some(),
        index,
    )?;
    let mut config_overlays = vec![(
        write_overlay(
            scratch,
            &format!("local-{index}"),
            &config,
            render_config(&common_settings),
        )?,
        config,
    )];
    if let Some(settings) = worktree_settings {
        config_overlays.push((
            write_overlay(
                scratch,
                &format!("worktree-{index}"),
                &worktree_config,
                render_config(&settings),
            )?,
            worktree_config,
        ));
    }
    Ok(GitMounts {
        read_only: if linked {
            vec![marker.to_path_buf()]
        } else {
            Vec::new()
        },
        hidden_directories: vec![(skeleton, common_dir)],
        read_only_after_masks,
        config_overlays,
    })
}

fn find_repo_markers(workspace: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut markers = Vec::new();
    for root in workspace.ancestors() {
        add_marker(&mut markers, root)?;
        if !markers.is_empty() {
            break;
        }
    }
    let mut entries = walkdir::WalkDir::new(workspace)
        .follow_links(false)
        .into_iter();
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|_| "cannot inspect nested workspace directories")?;
        if entry.file_name() == std::ffi::OsStr::new(".git") {
            if let Some(root) = entry.path().parent() {
                let marker = entry.path().to_path_buf();
                if !markers.iter().any(|(_, existing)| existing == &marker) {
                    markers.push((root.to_path_buf(), marker));
                }
            }
            if entry.file_type().is_dir() {
                entries.skip_current_dir();
            }
        }
    }
    markers.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(markers)
}

fn add_marker(markers: &mut Vec<(PathBuf, PathBuf)>, root: &Path) -> Result<(), String> {
    let marker = root.join(".git");
    match fs::symlink_metadata(&marker) {
        Ok(_) => {
            if !markers.iter().any(|(_, existing)| existing == &marker) {
                markers.push((root.to_path_buf(), marker));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect Git worktree marker".into()),
    }
    Ok(())
}

fn marker_paths(root: &Path, marker: &Path) -> Result<(PathBuf, PathBuf, bool), String> {
    let metadata =
        fs::symlink_metadata(marker).map_err(|_| "cannot inspect Git worktree marker")?;
    if metadata.file_type().is_dir() {
        return Ok((marker.to_path_buf(), marker.to_path_buf(), false));
    }
    if !metadata.file_type().is_file() {
        return Err("Git worktree marker must be a regular directory or file".into());
    }
    let git_dir = pointer_target(marker, b"gitdir: ", root)?;
    require_dir(&git_dir, "linked worktree metadata")?;
    if pointer_target(&git_dir.join("gitdir"), b"", &git_dir)? != marker {
        return Err("linked worktree backlink does not identify this workspace".into());
    }
    let common_dir = pointer_target(&git_dir.join("commondir"), b"", &git_dir)?;
    require_dir(&common_dir, "common Git metadata")?;
    Ok((git_dir, common_dir, true))
}

fn pointer_target(file: &Path, prefix: &[u8], relative_to: &Path) -> Result<PathBuf, String> {
    require_file(file, "Git metadata pointer")?;
    let bytes = fs::read(file).map_err(|_| "cannot read Git metadata pointer")?;
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let Some(value) = bytes.strip_prefix(prefix) else {
        return Err("invalid Git metadata pointer".into());
    };
    if value.is_empty() || value.contains(&b'\n') || value.contains(&b'\r') {
        return Err("invalid Git metadata pointer".into());
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(value.to_vec()));
    fs::canonicalize(if path.is_absolute() {
        path
    } else {
        relative_to.join(path)
    })
    .map_err(|_| "cannot resolve Git metadata pointer".into())
}

fn make_skeleton(
    scratch: &Path,
    common: &Path,
    git_dir: &Path,
    linked: bool,
    worktree_config_exists: bool,
    index: usize,
) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let skeleton = scratch.join(format!("git-metadata-{index}"));
    fs::create_dir(&skeleton).map_err(|_| "cannot create Git metadata skeleton")?;
    let mut mounts = vec![
        common.join("objects"),
        common.join("refs"),
        common.join("HEAD"),
    ];
    for path in [common.join("packed-refs"), common.join("info/exclude")] {
        if optional_file(&path)? {
            mounts.push(path);
        }
    }
    let index = git_dir.join("index");
    if optional_file(&index)? {
        mounts.push(index);
    }
    if linked {
        mounts.push(git_dir.join("HEAD"));
        mounts.push(git_dir.join("gitdir"));
        mounts.push(git_dir.join("commondir"));
    }
    let config = common.join("config");
    let worktree_config = git_dir.join("config.worktree");
    let mut placeholders = vec![config];
    if worktree_config_exists {
        placeholders.push(worktree_config);
    }
    for path in &mounts {
        make_placeholder(&skeleton, common, path)?;
    }
    for path in placeholders {
        make_placeholder(&skeleton, common, &path)?;
    }
    Ok((skeleton, mounts))
}

fn make_placeholder(skeleton: &Path, common: &Path, path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "cannot inspect Git metadata entry")?;
    let target = skeleton.join(
        path.strip_prefix(common)
            .map_err(|_| "Git metadata entry is outside the common directory")?,
    );
    if metadata.file_type().is_dir() {
        fs::create_dir_all(target).map_err(|_| "cannot create Git metadata placeholder")?;
    } else if metadata.file_type().is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "cannot create Git metadata placeholder")?;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|_| "cannot create Git metadata placeholder")?;
    } else {
        return Err("Git metadata entry is not a regular file or directory".into());
    }
    Ok(())
}

fn optional_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err("Git metadata entry is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("cannot inspect optional Git metadata entry".into()),
    }
}

#[derive(Default)]
struct ConfigSettings {
    repository_format_version: Option<u32>,
    filemode: Option<bool>,
    symlinks: Option<bool>,
    ignorecase: Option<bool>,
    object_format: Option<String>,
    worktree_config: Option<bool>,
}

fn read_settings(path: &Path, scratch: &Path, worktree: bool) -> Result<ConfigSettings, String> {
    require_file(path, "repository config")?;
    if !config_keys(
        path,
        "^(include|includeif)\\.|^filter\\.|^core\\.(autocrlf|eol|attributesfile|excludesfile|fsmonitor|ignorestat|trustctime|checkstat|sparsecheckout|splitindex)$|^diff\\.(external|.*\\.textconv)$",
        scratch,
    )?
    .is_empty()
    {
        return Err("unsupported Git status or diff configuration".into());
    }
    let extensions = config_keys(path, "^extensions\\.", scratch)?;
    if worktree {
        if !extensions.is_empty()
            || config_value(path, "core.repositoryformatversion", Some("int"), scratch)?.is_some()
        {
            return Err("unsupported worktree-specific Git configuration".into());
        }
    } else {
        for key in extensions {
            let key = key.to_ascii_lowercase();
            if key != "extensions.objectformat" && key != "extensions.worktreeconfig" {
                return Err("unsupported Git repository extension".into());
            }
        }
    }
    if config_value(path, "core.worktree", None, scratch)?.is_some()
        || config_bool(path, "core.bare", scratch)? == Some(true)
    {
        return Err("unsupported Git worktree configuration".into());
    }
    let mut settings = ConfigSettings {
        filemode: config_bool(path, "core.filemode", scratch)?,
        symlinks: config_bool(path, "core.symlinks", scratch)?,
        ignorecase: config_bool(path, "core.ignorecase", scratch)?,
        ..ConfigSettings::default()
    };
    if worktree {
        return Ok(settings);
    }
    settings.repository_format_version =
        config_value(path, "core.repositoryformatversion", Some("int"), scratch)?
            .map(|value| parse_integer(&value))
            .transpose()?;
    let format_version = settings.repository_format_version.unwrap_or(0);
    settings.object_format = config_value(path, "extensions.objectformat", None, scratch)?
        .map(|value| {
            String::from_utf8(value)
                .map(|value| value.to_ascii_lowercase())
                .map_err(|_| "invalid Git object format".to_string())
        })
        .transpose()?;
    if settings
        .object_format
        .as_deref()
        .is_some_and(|format| format != "sha256")
    {
        return Err("unsupported Git object format".into());
    }
    settings.worktree_config = config_bool(path, "extensions.worktreeconfig", scratch)?;
    if format_version == 0
        && (settings.object_format.is_some() || settings.worktree_config == Some(true))
    {
        return Err("Git extension requires repository format version 1".into());
    }
    if format_version > 1 {
        return Err("unsupported Git repository format".into());
    }
    Ok(settings)
}

fn config_bool(path: &Path, key: &str, scratch: &Path) -> Result<Option<bool>, String> {
    config_value(path, key, Some("bool"), scratch)?
        .map(|value| match value.as_slice() {
            b"true" => Ok(true),
            b"false" => Ok(false),
            _ => Err("invalid typed Git boolean".into()),
        })
        .transpose()
}

fn parse_integer(value: &[u8]) -> Result<u32, String> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "invalid typed Git integer".into())
}

fn git_config(
    path: &Path,
    scratch: &Path,
    args: &[&str],
    kind: Option<&str>,
) -> Result<Output, String> {
    let mut command = clean_git(scratch);
    command
        .arg("config")
        .arg("--no-includes")
        .arg("--file")
        .arg(path);
    if let Some(kind) = kind {
        command.arg(format!("--type={kind}"));
    }
    command
        .args(args)
        .output()
        .map_err(|_| "cannot run Git config".into())
}

fn config_value(
    path: &Path,
    key: &str,
    kind: Option<&str>,
    scratch: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let output = git_config(path, scratch, &["--get", key], kind)?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err("cannot parse repository configuration".into());
    }
    let value = output
        .stdout
        .strip_suffix(b"\n")
        .ok_or("invalid Git config output")?;
    if value.contains(&b'\n') {
        return Err("invalid Git config output".into());
    }
    Ok(Some(value.to_vec()))
}

fn config_keys(path: &Path, pattern: &str, scratch: &Path) -> Result<Vec<String>, String> {
    let output = git_config(
        path,
        scratch,
        &["--name-only", "--get-regexp", pattern],
        None,
    )?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        return Err("cannot parse repository configuration".into());
    }
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|key| !key.is_empty())
        .map(|key| String::from_utf8(key.to_vec()).map_err(|_| "invalid Git config key".into()))
        .collect()
}

fn rev_parse(workspace: &Path, scratch: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = clean_git(scratch)
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .args(args)
        .output()
        .map_err(|_| "cannot run Git discovery".to_string())?;
    if !output.status.success() {
        return Err("Git worktree discovery failed".into());
    }
    Ok(output.stdout)
}
fn git_has_gitlinks(workspace: &Path, scratch: &Path) -> Result<bool, String> {
    let output = clean_git(scratch)
        .arg("-C")
        .arg(workspace)
        .args(["ls-files", "--stage", "-z"])
        .output()
        .map_err(|_| "cannot inspect Git index".to_string())?;
    if !output.status.success() {
        return Err("cannot inspect Git index".into());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .any(|entry| entry.starts_with(b"160000 ")))
}

fn rev_value(workspace: &Path, scratch: &Path, arg: &str) -> Result<Vec<u8>, String> {
    let output = rev_parse(workspace, scratch, &[arg])?;
    let value = output.strip_suffix(b"\n").unwrap_or(&output);
    if value.contains(&b'\n') {
        return Err("invalid Git metadata output".into());
    }
    Ok(value.to_vec())
}

fn rev_path(workspace: &Path, scratch: &Path, args: &[&str]) -> Result<PathBuf, String> {
    let output = rev_parse(workspace, scratch, args)?;
    let value = output.strip_suffix(b"\n").unwrap_or(&output);
    let path = PathBuf::from(std::ffi::OsString::from_vec(value.to_vec()));
    if !path.is_absolute() {
        return Err("Git returned a non-absolute metadata path".into());
    }
    fs::canonicalize(path).map_err(|_| "cannot resolve Git metadata path".into())
}

fn clean_git(scratch: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("HOME", scratch)
        .env("XDG_CONFIG_HOME", scratch)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn validate_layout(root: &Path, git_dir: &Path, common: &Path, linked: bool) -> Result<(), String> {
    require_dir(common, "common Git metadata")?;
    let objects = common.join("objects");
    require_dir(&objects, "Git object storage")?;
    require_dir(&objects.join("info"), "Git object info")?;
    require_dir(&common.join("info"), "Git metadata info")?;
    require_dir(&objects.join("pack"), "Git pack storage")?;
    require_dir(&common.join("refs"), "Git refs")?;
    require_file(&common.join("HEAD"), "Git HEAD")?;
    require_file(&common.join("config"), "repository config")?;
    require_file(&git_dir.join("HEAD"), "worktree HEAD")?;
    if linked {
        let worktrees = common.join("worktrees");
        require_dir(&worktrees, "linked worktree metadata")?;
        if git_dir.parent() != Some(worktrees.as_path()) {
            return Err("linked worktree metadata is outside the standard layout".into());
        }
        require_file(&git_dir.join("gitdir"), "linked worktree backlink")?;
        require_file(&git_dir.join("commondir"), "common metadata pointer")?;
    } else if git_dir != common || root.join(".git") != git_dir {
        return Err("Git metadata is outside the standard layout".into());
    }
    for path in [
        objects.join("info/alternates"),
        objects.join("info/http-alternates"),
        common.join("shallow"),
    ] {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err("unsupported external or partial Git object storage".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot inspect Git object storage".into()),
        }
    }
    if optional_file(&common.join("info/attributes"))? {
        return Err("repository info attributes are unsupported".into());
    }
    for entry in fs::read_dir(objects.join("pack")).map_err(|_| "cannot inspect Git packs")? {
        let entry = entry.map_err(|_| "cannot inspect Git packs")?;
        let kind = entry
            .file_type()
            .map_err(|_| "cannot inspect Git pack entry")?;
        if kind.is_symlink()
            || entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "promisor")
        {
            return Err("unsupported symlinked or partial Git object storage".into());
        }
    }
    Ok(())
}

fn require_dir(path: &Path, description: &str) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .map_err(|_| format!("cannot inspect {description}"))?
        .file_type()
        .is_dir()
    {
        return Err(format!("{description} is not a regular directory"));
    }
    Ok(())
}

fn require_file(path: &Path, description: &str) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .map_err(|_| format!("cannot inspect {description}"))?
        .file_type()
        .is_file()
    {
        return Err(format!("{description} is not a regular file"));
    }
    Ok(())
}

fn render_config(settings: &ConfigSettings) -> Vec<u8> {
    let mut config = String::new();
    if let Some(version) = settings.repository_format_version {
        push_setting(
            &mut config,
            "core",
            "repositoryformatversion",
            &version.to_string(),
        );
    }
    for (key, value) in [
        ("filemode", settings.filemode),
        ("symlinks", settings.symlinks),
        ("ignorecase", settings.ignorecase),
    ] {
        if let Some(value) = value {
            push_setting(
                &mut config,
                "core",
                key,
                if value { "true" } else { "false" },
            );
        }
    }
    if let Some(format) = &settings.object_format {
        push_setting(&mut config, "extensions", "objectformat", format);
    }
    if let Some(value) = settings.worktree_config {
        push_setting(
            &mut config,
            "extensions",
            "worktreeconfig",
            if value { "true" } else { "false" },
        );
    }
    config.into_bytes()
}

fn push_setting(config: &mut String, section: &str, key: &str, value: &str) {
    use std::fmt::Write as _;
    let _ = writeln!(config, "[{section}]");
    let _ = writeln!(config, "\t{key} = {value}");
}

fn write_overlay(
    scratch: &Path,
    label: &str,
    original: &Path,
    mut content: Vec<u8>,
) -> Result<PathBuf, String> {
    if content.is_empty()
        && fs::metadata(original)
            .map_err(|_| "cannot inspect repository config")?
            .len()
            != 0
    {
        content.extend_from_slice(b"# non-allowlisted repository settings omitted\n");
    }
    let path = scratch.join(format!("git-config-{label}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| "cannot create sanitized Git config")?;
    file.write_all(&content)
        .map_err(|_| "cannot write sanitized Git config")?;
    Ok(path)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "fixture setup failures should stop the test at their source"
)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cruise-linux-git-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git(cwd: &Path, args: &[&str]) -> Output {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Git Mount Test")
            .env("GIT_AUTHOR_EMAIL", "git-mount@example.invalid")
            .env("GIT_COMMITTER_NAME", "Git Mount Test")
            .env("GIT_COMMITTER_EMAIL", "git-mount@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn init_repo(path: &Path) {
        fs::create_dir(path).unwrap();
        git(path, &["init", "-q"]);
        fs::write(path.join("tracked"), "original\n").unwrap();
        git(path, &["add", "tracked"]);
        git(path, &["commit", "-qm", "initial"]);
    }

    #[test]
    fn normal_nested_worktree_uses_sanitized_config_and_preserves_status_states() {
        let temp = TempDir::new();
        let repo = temp.0.join("repo");
        init_repo(&repo);
        git(&repo, &["config", "core.filemode", "false"]);
        git(&repo, &["config", "credential.helper", "raw-config-secret"]);
        let nested = repo.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(repo.join("tracked"), "staged\n").unwrap();
        git(&repo, &["add", "tracked"]);
        fs::write(repo.join("tracked"), "unstaged\n").unwrap();
        fs::write(repo.join("untracked"), "new\n").unwrap();

        let status = git(&nested, &["status", "--porcelain"]);
        let status = String::from_utf8(status.stdout).unwrap();
        assert!(status.lines().any(|line| line.starts_with("MM tracked")));
        assert!(status.lines().any(|line| line.starts_with("?? untracked")));

        let scratch = temp.0.join("scratch");
        fs::create_dir(&scratch).unwrap();
        let mounts = prepare(&nested, &scratch).unwrap();
        assert_eq!(mounts.read_only, Vec::<PathBuf>::new());
        let (skeleton, destination) = &mounts.hidden_directories[0];
        assert_eq!(destination, &repo.join(".git"));
        assert!(!skeleton.join("worktrees").exists());
        assert!(!skeleton.join("modules").exists());
        assert!(
            mounts
                .read_only_after_masks
                .contains(&repo.join(".git/objects"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&repo.join(".git/refs"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&repo.join(".git/HEAD"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&repo.join(".git/index"))
        );
        assert_eq!(mounts.config_overlays.len(), 1);
        let (source, destination) = &mounts.config_overlays[0];
        assert_eq!(destination, &repo.join(".git/config"));
        let sanitized = fs::read_to_string(source).unwrap();
        assert!(sanitized.contains("filemode = false"));
        assert!(!sanitized.contains("raw-config-secret"));
    }

    #[test]
    fn nested_workspace_does_not_admit_its_outer_repository() {
        let temp = TempDir::new();
        let outer = temp.0.join("outer");
        init_repo(&outer);
        let workspace = outer.join("nested");
        init_repo(&workspace);
        let scratch = temp.0.join("scratch");
        fs::create_dir(&scratch).unwrap();
        let mounts = prepare(&workspace, &scratch).unwrap();
        assert_eq!(mounts.hidden_directories.len(), 1);
        assert_eq!(mounts.hidden_directories[0].1, workspace.join(".git"));
    }

    #[test]
    fn nested_independent_and_linked_repositories_get_separate_masks() {
        let temp = TempDir::new();
        let workspace = temp.0.join("workspace");
        init_repo(&workspace);
        git(&workspace, &["config", "credential.helper", "outer-secret"]);

        let independent = workspace.join("independent");
        init_repo(&independent);
        git(
            &independent,
            &["config", "credential.helper", "independent-secret"],
        );

        let shared = temp.0.join("shared");
        init_repo(&shared);
        git(&shared, &["config", "core.repositoryformatversion", "1"]);
        git(&shared, &["config", "extensions.worktreeConfig", "true"]);
        let linked = workspace.join("linked");
        git(
            &shared,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        git(
            &linked,
            &["config", "--worktree", "credential.helper", "linked-secret"],
        );

        let scratch = temp.0.join("scratch");
        fs::create_dir(&scratch).unwrap();
        let mounts = prepare(&workspace, &scratch).unwrap();
        let mut common_dirs: Vec<_> = mounts
            .hidden_directories
            .iter()
            .map(|(_, destination)| destination.clone())
            .collect();
        common_dirs.sort();
        let mut expected = vec![
            workspace.join(".git"),
            independent.join(".git"),
            shared.join(".git"),
        ];
        expected.sort();
        assert_eq!(common_dirs, expected);
        assert_eq!(mounts.config_overlays.len(), 4);
        assert_eq!(mounts.read_only, vec![linked.join(".git")]);
        for (source, _) in &mounts.config_overlays {
            let sanitized = fs::read_to_string(source).unwrap();
            for secret in ["outer-secret", "independent-secret", "linked-secret"] {
                assert!(!sanitized.contains(secret));
            }
        }
    }

    #[test]
    fn rejects_repeated_common_directory_views() {
        let temp = TempDir::new();
        let repo = temp.0.join("repo");
        init_repo(&repo);
        git(&repo, &["config", "core.repositoryformatversion", "1"]);
        git(&repo, &["config", "extensions.worktreeConfig", "true"]);
        let linked = repo.join("linked");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let scratch = temp.0.join("scratch");
        fs::create_dir(&scratch).unwrap();

        let Err(error) = prepare(&repo, &scratch) else {
            panic!("repeated common directory view was accepted");
        };
        assert!(error.contains("overlapping or repeated"));
    }

    #[test]
    fn linked_worktree_protects_shared_and_private_metadata() {
        let temp = TempDir::new();
        let repo = temp.0.join("repo");
        init_repo(&repo);
        let linked = temp.0.join("linked");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        git(&repo, &["config", "core.repositoryformatversion", "1"]);
        git(&repo, &["config", "extensions.worktreeConfig", "true"]);
        git(
            &linked,
            &["config", "--worktree", "core.ignorecase", "false"],
        );
        git(
            &linked,
            &[
                "config",
                "--worktree",
                "credential.helper",
                "worktree-secret",
            ],
        );
        let nested = linked.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(linked.join("tracked"), "staged linked\n").unwrap();
        git(&linked, &["add", "tracked"]);
        fs::write(linked.join("tracked"), "unstaged linked\n").unwrap();
        fs::write(linked.join("untracked"), "new linked\n").unwrap();
        let status = String::from_utf8(git(&nested, &["status", "--porcelain"]).stdout).unwrap();
        assert!(status.lines().any(|line| line.starts_with("MM tracked")));
        assert!(status.lines().any(|line| line.starts_with("?? untracked")));
        let scratch = temp.0.join("scratch");
        fs::create_dir(&scratch).unwrap();

        let mounts = prepare(&nested, &scratch).unwrap();
        let git_dir = PathBuf::from(
            String::from_utf8(git(&linked, &["rev-parse", "--absolute-git-dir"]).stdout)
                .unwrap()
                .trim(),
        );
        let common_dir = repo.join(".git");
        assert_eq!(mounts.read_only, vec![linked.join(".git")]);
        let (skeleton, destination) = &mounts.hidden_directories[0];
        assert_eq!(destination, &common_dir);
        let admin = git_dir.strip_prefix(&common_dir).unwrap();
        assert!(skeleton.join(admin).is_dir());
        assert!(mounts.read_only_after_masks.contains(&git_dir.join("HEAD")));
        assert!(
            mounts
                .read_only_after_masks
                .contains(&git_dir.join("index"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&git_dir.join("gitdir"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&git_dir.join("commondir"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&common_dir.join("objects"))
        );
        assert!(
            mounts
                .read_only_after_masks
                .contains(&common_dir.join("refs"))
        );
        assert!(!mounts.read_only_after_masks.contains(&git_dir));
        assert!(!skeleton.join("modules").exists());
        assert_eq!(mounts.config_overlays.len(), 2);
        let shared = fs::read_to_string(&mounts.config_overlays[0].0).unwrap();
        let private = fs::read_to_string(&mounts.config_overlays[1].0).unwrap();
        assert!(shared.contains("worktreeconfig = true"));
        assert!(private.contains("ignorecase = false"));
        assert!(!shared.contains("worktree-secret"));
        assert!(!private.contains("worktree-secret"));
        let sibling = temp.0.join("sibling");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                sibling.to_str().unwrap(),
                "HEAD",
            ],
        );
        git(
            &sibling,
            &[
                "config",
                "--worktree",
                "credential.helper",
                "sibling-secret",
            ],
        );
        let sibling_dir = PathBuf::from(
            String::from_utf8(git(&sibling, &["rev-parse", "--absolute-git-dir"]).stdout)
                .unwrap()
                .trim(),
        );
        assert!(
            fs::read_to_string(sibling_dir.join("config.worktree"))
                .unwrap()
                .contains("sibling-secret")
        );
        assert!(
            !skeleton
                .join(sibling_dir.strip_prefix(&common_dir).unwrap())
                .exists()
        );
        assert_eq!(fs::read_dir(skeleton.join("worktrees")).unwrap().count(), 1);
    }

    #[test]
    fn rejects_a_dot_git_pointer_without_a_matching_backlink() {
        let temp = TempDir::new();
        let unrelated = temp.0.join("unrelated");
        init_repo(&unrelated);
        git(
            &unrelated,
            &["config", "credential.helper", "external-secret"],
        );
        let workspace = temp.0.join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(
            workspace.join(".git"),
            format!("gitdir: {}/.git\n", unrelated.display()),
        )
        .unwrap();
        let scratch = temp.0.join("scratch");
        fs::create_dir(&scratch).unwrap();

        let Err(error) = prepare(&workspace, &scratch) else {
            panic!("unrelated Git metadata pointer was accepted");
        };
        assert!(!error.contains("external-secret"));
        assert!(fs::read_dir(&scratch).unwrap().next().is_none());
    }

    #[test]
    fn returns_empty_mounts_for_a_non_repository() {
        let temp = TempDir::new();
        let workspace = temp.0.join("workspace");
        let scratch = temp.0.join("scratch");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&scratch).unwrap();
        assert_eq!(
            prepare(&workspace, &scratch).unwrap().read_only,
            Vec::<PathBuf>::new()
        );
    }
}
