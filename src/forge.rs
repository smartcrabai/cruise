//! Local forge (GitHub / GitLab) detection and repository locator parsing.

use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForgeKind {
    GitHub,
    GitLab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgeContext {
    pub kind: ForgeKind,
    pub host: String,
    pub repository: String,
}

fn err<T>(msg: String) -> Result<T> {
    Err(CruiseError::Other(msg))
}

/// Split a remote URL into `(host, path)`; supports HTTPS, `ssh://` and scp style.
fn split_remote(url: &str) -> Result<(String, String)> {
    let url = url.trim();
    if url.is_empty() || url.chars().any(char::is_whitespace) {
        return err(format!("invalid remote URL: `{url}`"));
    }
    let (authority, path, has_scheme) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        (authority, path, true)
    } else {
        let Some((authority, path)) = url.split_once(':') else {
            return err(format!("invalid remote URL: `{url}`"));
        };
        (authority, path, false)
    };
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if has_scheme {
        host_port.split(':').next().unwrap_or("")
    } else {
        host_port
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return err(format!(
            "remote URL has no host or repository path: `{url}`"
        ));
    }
    Ok((host.to_ascii_lowercase(), path.to_string()))
}

fn validate_path(path: &str, min_segments: usize) -> Result<()> {
    let segments: Vec<&str> = path.split('/').collect();
    let ok = segments.len() >= min_segments
        && segments.iter().all(|seg| {
            !seg.is_empty()
                && !seg.starts_with('-')
                && *seg != "."
                && *seg != ".."
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
    if ok {
        Ok(())
    } else {
        err(format!("invalid repository path: `{path}`"))
    }
}

/// Resolve a git remote URL (HTTPS, `ssh://`, scp style) into a forge context.
pub(crate) fn resolve_remote(url: &str, override_kind: Option<ForgeKind>) -> Result<ForgeContext> {
    let (host, repository) = split_remote(url)?;
    validate_path(&repository, 2)?;
    let kind = match (override_kind, host.as_str()) {
        (Some(kind), _) => kind,
        (None, "github.com") => ForgeKind::GitHub,
        (None, "gitlab.com") => ForgeKind::GitLab,
        (None, _) => {
            return err(format!(
                "unsupported git host `{host}`: set CRUISE_FORGE=github|gitlab to choose a forge"
            ));
        }
    };
    Ok(ForgeContext {
        kind,
        host,
        repository,
    })
}

/// Parse a `CRUISE_FORGE` value (`github` / `gitlab`, case-insensitive; empty means unset).
pub(crate) fn parse_forge_override(value: Option<&str>) -> Result<Option<ForgeKind>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v.eq_ignore_ascii_case("github") => Ok(Some(ForgeKind::GitHub)),
        Some(v) if v.eq_ignore_ascii_case("gitlab") => Ok(Some(ForgeKind::GitLab)),
        Some(v) => err(format!(
            "invalid CRUISE_FORGE `{v}`: expected github or gitlab"
        )),
    }
}

/// Read the `CRUISE_FORGE` environment variable.
pub(crate) fn forge_override_from_env() -> Result<Option<ForgeKind>> {
    parse_forge_override(std::env::var("CRUISE_FORGE").ok().as_deref())
}

/// Resolve a `--repo` locator using `CRUISE_FORGE` and `GITLAB_HOST` from the environment.
pub(crate) fn resolve_repo_locator_env(locator: &str) -> Result<ForgeContext> {
    let forced = forge_override_from_env()?;
    resolve_repo_locator(locator, forced)
}

/// Verify that the CLI for `kind` (`gh` / `glab`) is available.
pub(crate) fn ensure_forge_available(kind: ForgeKind) -> Result<()> {
    match kind {
        ForgeKind::GitHub => crate::worktree_pr::ensure_gh_available(),
        ForgeKind::GitLab => {
            let ok = std::process::Command::new("glab")
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .output()
                .is_ok_and(|o| o.status.success());
            if ok {
                Ok(())
            } else {
                err(
                    "glab CLI is not installed. Install it from https://gitlab.com/gitlab-org/cli"
                        .to_string(),
                )
            }
        }
    }
}

/// Determine the forge of a session, falling back for sessions saved before `forge` existed.
pub(crate) fn session_forge(session: &crate::session::SessionState) -> ForgeKind {
    if let Some(kind) = session.forge {
        return kind;
    }
    if session
        .pr_url
        .as_deref()
        .is_some_and(|u| u.contains("/-/merge_requests/"))
    {
        return ForgeKind::GitLab;
    }
    if let Some(repo) = session.repo.as_deref()
        && let Ok(ctx) = resolve_repo_locator_env(repo)
    {
        return ctx.kind;
    }
    let dir = session
        .worktree_path
        .as_deref()
        .unwrap_or(&session.base_dir);
    if let Ok(url) = origin_url(dir)
        && let Ok(ctx) = resolve_remote(&url, forge_override_from_env().ok().flatten())
    {
        return ctx.kind;
    }
    ForgeKind::GitHub
}

/// Read the `origin` remote URL of the git checkout at `dir`.
pub(crate) fn origin_url(dir: &std::path::Path) -> std::result::Result<String, String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("failed to run git remote get-url origin: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "failed to determine GitHub repository: git remote get-url origin failed: {}",
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolve a `--repo` locator into a forge context.
pub(crate) fn resolve_repo_locator(
    locator: &str,
    override_kind: Option<ForgeKind>,
) -> Result<ForgeContext> {
    resolve_repo_locator_with_host(
        locator,
        override_kind,
        std::env::var("GITLAB_HOST").ok().as_deref(),
    )
}

fn resolve_repo_locator_with_host(
    locator: &str,
    override_kind: Option<ForgeKind>,
    gitlab_host: Option<&str>,
) -> Result<ForgeContext> {
    let locator = locator.trim();
    if locator.contains("://") || locator.contains('@') {
        return resolve_remote(locator, override_kind);
    }
    let kind = match (override_kind, locator.split('/').count()) {
        (Some(kind), _) => kind,
        (None, 2) => ForgeKind::GitHub,
        (None, _) => {
            return err(format!(
                "`{locator}` is not owner/repo: use a full URL or set CRUISE_FORGE=gitlab"
            ));
        }
    };
    validate_path(locator, 2)?;
    let host = match kind {
        ForgeKind::GitHub => "github.com".to_string(),
        ForgeKind::GitLab => gitlab_host
            .map(|h| {
                let h = h.trim();
                h.split_once("://")
                    .map_or(h, |(_, r)| r)
                    .trim_end_matches('/')
                    .to_string()
            })
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "gitlab.com".to_string()),
    };
    Ok(ForgeContext {
        kind,
        host,
        repository: locator.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CruiseError;

    fn session_with_origin(origin: &str) -> (tempfile::TempDir, crate::session::SessionState) {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", origin]] {
            let ok = std::process::Command::new("git")
                .args(&args)
                .current_dir(dir.path())
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "git {args:?} failed");
        }
        let state = crate::session::SessionState::new(
            "s".to_string(),
            dir.path().to_path_buf(),
            crate::session_config::SessionConfigRef::BuiltinSnapshot { name: None },
            "task".to_string(),
        );
        (dir, state)
    }

    #[test]
    fn test_session_forge_falls_back_to_origin_remote() {
        let (_d, mut s) = session_with_origin("https://gitlab.com/g/sub/p.git");
        s.forge = None;
        s.repo = None;
        assert_eq!(session_forge(&s), ForgeKind::GitLab);
        let (_d2, mut s2) = session_with_origin("https://github.com/o/r.git");
        s2.forge = None;
        s2.repo = None;
        assert_eq!(session_forge(&s2), ForgeKind::GitHub);
    }

    fn ctx(kind: ForgeKind, host: &str, repo: &str) -> ForgeContext {
        ForgeContext {
            kind,
            host: host.to_string(),
            repository: repo.to_string(),
        }
    }

    #[test]
    fn test_remote_github_https_with_git_suffix() {
        let got = resolve_remote("https://github.com/owner/repo.git", None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got, ctx(ForgeKind::GitHub, "github.com", "owner/repo"));
    }

    #[test]
    fn test_remote_github_scp_style() {
        let got = resolve_remote("git@github.com:owner/repo.git", None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got, ctx(ForgeKind::GitHub, "github.com", "owner/repo"));
    }

    #[test]
    fn test_remote_github_ssh_scheme() {
        let got = resolve_remote("ssh://git@github.com/owner/repo.git", None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got, ctx(ForgeKind::GitHub, "github.com", "owner/repo"));
    }

    #[test]
    fn test_remote_gitlab_com_nested_namespace() {
        let got = resolve_remote("https://gitlab.com/group/sub/project.git", None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            got,
            ctx(ForgeKind::GitLab, "gitlab.com", "group/sub/project")
        );
    }

    #[test]
    fn test_remote_gitlab_scp_nested_namespace() {
        let got = resolve_remote("git@gitlab.com:group/sub/project.git", None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            got,
            ctx(ForgeKind::GitLab, "gitlab.com", "group/sub/project")
        );
    }

    #[test]
    fn test_remote_self_managed_with_gitlab_override() {
        let got = resolve_remote(
            "https://git.example.com/team/app.git",
            Some(ForgeKind::GitLab),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got, ctx(ForgeKind::GitLab, "git.example.com", "team/app"));
    }

    #[test]
    fn test_remote_unknown_host_without_override_is_rejected() {
        let Err(err) = resolve_remote("https://bitbucket.org/team/app.git", None) else {
            panic!("expected error");
        };
        assert!(matches!(err, CruiseError::Other(_)), "{err:?}");
    }

    #[test]
    fn test_remote_explicit_override_wins_over_known_host() {
        let got = resolve_remote("https://github.com/owner/repo", Some(ForgeKind::GitLab))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got.kind, ForgeKind::GitLab);
    }

    #[test]
    fn test_remote_malformed_inputs_are_rejected() {
        for bad in [
            "",
            "https://",
            "https://gitlab.com/",
            "https://gitlab.com",
            "not a url",
        ] {
            let Err(err) = resolve_remote(bad, Some(ForgeKind::GitLab)) else {
                panic!("expected error");
            };
            assert!(matches!(err, CruiseError::Other(_)), "{bad}: {err:?}");
        }
    }

    #[test]
    fn test_locator_github_shortcut_stays_github() {
        let got = resolve_repo_locator("owner/repo", None).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got, ctx(ForgeKind::GitHub, "github.com", "owner/repo"));
    }

    #[test]
    fn test_locator_namespace_path_requires_gitlab_override() {
        let got = resolve_repo_locator("group/sub/project", Some(ForgeKind::GitLab))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got.kind, ForgeKind::GitLab);
        assert_eq!(got.repository, "group/sub/project");
        assert!(resolve_repo_locator("group/sub/project", None).is_err());
    }

    #[test]
    fn test_locator_full_gitlab_url_keeps_host_and_path() {
        let got = resolve_repo_locator("https://gitlab.com/group/sub/project", None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            got,
            ctx(ForgeKind::GitLab, "gitlab.com", "group/sub/project")
        );
    }

    #[test]
    fn test_locator_self_managed_url_with_override() {
        let got = resolve_repo_locator("https://git.example.com/team/app", Some(ForgeKind::GitLab))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(got, ctx(ForgeKind::GitLab, "git.example.com", "team/app"));
    }

    #[test]
    fn test_locator_gitlab_host_default_scheme_and_empty() {
        let host = |h: Option<&str>| {
            resolve_repo_locator_with_host("g/p", Some(ForgeKind::GitLab), h)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .host
        };
        assert_eq!(host(None), "gitlab.com");
        assert_eq!(host(Some("https://git.example.com/")), "git.example.com");
        assert_eq!(host(Some("  ")), "gitlab.com");
    }

    #[test]
    fn test_parse_forge_override() {
        assert_eq!(parse_forge_override(None).ok(), Some(None));
        assert_eq!(
            parse_forge_override(Some("GitLab")).ok(),
            Some(Some(ForgeKind::GitLab))
        );
        assert_eq!(
            parse_forge_override(Some("github")).ok(),
            Some(Some(ForgeKind::GitHub))
        );
        assert!(parse_forge_override(Some("bitbucket")).is_err());
    }

    #[test]
    fn test_locator_invalid_specs_are_rejected() {
        for bad in [
            "",
            "-owner/repo",
            "owner/-repo",
            "owner",
            "owner//repo",
            "../x",
        ] {
            let Err(err) = resolve_repo_locator(bad, None) else {
                panic!("expected error");
            };
            assert!(matches!(err, CruiseError::Other(_)), "{bad}: {err:?}");
        }
    }
}
