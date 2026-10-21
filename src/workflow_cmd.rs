//! `cruise workflow` subcommands: built-in list/eject and GitHub workflow packages.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use console::{measure_text_width, style};

use crate::builtin_workflows::{BUILTIN_WORKFLOWS, BuiltinWorkflow, find_builtin};
use crate::error::{CruiseError, Result};
use crate::workflow_packages::{
    self as packages, InstalledPackage, PreparedPackage, WorkflowPackageError,
};

/// Print the built-in workflows, then the installed workflow packages.
///
/// # Errors
///
/// Returns an error when the installed packages cannot be read.
pub fn list() -> Result<()> {
    let installed = packages::list_installed().map_err(to_cruise)?;
    let home = home::home_dir();
    let cwd = std::env::current_dir().ok();
    let display_path = |p: &Path| match &cwd {
        Some(cwd) => crate::resolver::shorten_display_path(p, cwd, home.as_deref()),
        None => p.display().to_string(),
    };
    for line in render_list(BUILTIN_WORKFLOWS, &installed, display_path) {
        println!("{line}");
    }
    Ok(())
}

/// Render the list output as lines (pure; no I/O).
fn render_list(
    builtins: &[BuiltinWorkflow],
    installed: &[InstalledPackage],
    display_path: impl Fn(&Path) -> String,
) -> Vec<String> {
    use crate::batch_dashboard::sanitize_terminal_text as clean;

    struct Row {
        name: String,
        source: String,
        sha: String,
        path: String,
    }
    let rows: Vec<Row> = installed
        .iter()
        .map(|p| Row {
            name: clean(&p.name),
            source: clean(&format!(
                "{}/{}/{}@{}",
                p.owner, p.repo, p.workflow_path, p.requested_ref
            )),
            sha: clean(&p.resolved_commit.chars().take(7).collect::<String>()),
            path: clean(&display_path(&p.config_path)),
        })
        .collect();

    let name_width = builtins
        .iter()
        .map(|w| measure_text_width(w.name))
        .chain(rows.iter().map(|r| measure_text_width(&r.name)))
        .max()
        .unwrap_or(0);
    let source_width = rows
        .iter()
        .map(|r| measure_text_width(&r.source))
        .max()
        .unwrap_or(0);

    let mut lines = vec![style("Built-in workflows").bold().to_string()];
    for w in builtins {
        let name = style(format!("{:<name_width$}", w.name)).cyan().bold();
        if w.name == crate::builtin_workflows::DEFAULT_BUILTIN_NAME {
            lines.push(format!(
                "  {name}  {}  {}",
                w.description,
                style("(default)").green()
            ));
        } else {
            lines.push(format!("  {name}  {}", w.description));
        }
    }
    lines.push(String::new());
    lines.push(style("Installed packages").bold().to_string());
    if rows.is_empty() {
        lines.push(format!(
            "  {}  Install one with: {}",
            style("(none)").dim(),
            style("cruise workflow add <owner/repo[/path][@ref]>").cyan()
        ));
    }
    let indent = " ".repeat(2 + name_width + 2);
    for r in &rows {
        let name = style(format!("{:<name_width$}", r.name)).cyan().bold();
        let source = format!("{:<source_width$}", r.source);
        lines.push(format!("  {name}  {source}  {}", style(&r.sha).dim()));
        lines.push(format!("{indent}{}", style(&r.path).dim()));
    }
    lines
}

/// Eject a built-in workflow to the project (`./.cruise/`) or user workflows directory.
///
/// # Errors
///
/// Returns an error when the destination cannot be determined or ejecting fails.
pub fn eject(name: &str, project: bool) -> Result<()> {
    let dir = if project {
        std::env::current_dir()?.join(".cruise")
    } else {
        crate::paths::workflows_dir()?
    };
    let path = eject_builtin(name, &dir)?;
    println!("Ejected '{name}' to {}", path.display());
    Ok(())
}

/// Copy a built-in workflow to `<destination_dir>/<name>.yaml` without overwriting.
///
/// # Errors
///
/// Fails for unknown names, existing files, or I/O errors.
pub fn eject_builtin(name: &str, destination_dir: &Path) -> Result<PathBuf> {
    let workflow = find_builtin(name)
        .ok_or_else(|| CruiseError::Other(format!("unknown built-in workflow: {name}")))?;
    std::fs::create_dir_all(destination_dir)?;
    let path = destination_dir.join(format!("{}.yaml", workflow.name));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    if let Err(e) = file
        .write_all(workflow.yaml.as_bytes())
        .and_then(|()| file.flush())
    {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    Ok(path)
}

/// Install a workflow package from GitHub.
///
/// # Errors
///
/// Returns an error when the spec is invalid, fetching fails, or confirmation is refused.
pub fn add(spec: &str, name: Option<String>, yes: bool) -> Result<()> {
    let spec = packages::parse_package_spec(spec).map_err(to_cruise)?;
    let name = name.unwrap_or_else(|| packages::default_name(&spec));
    let prepared = packages::prepare_install(&spec, &name).map_err(to_cruise)?;
    install(prepared, yes, "Installed")
}

/// Update an installed workflow package.
///
/// # Errors
///
/// Returns an error when the package is unknown, fetching fails, or confirmation is refused.
pub fn update(name: &str, yes: bool) -> Result<()> {
    let prepared = packages::prepare_update(name).map_err(to_cruise)?;
    install(prepared, yes, "Updated")
}

/// Remove an installed workflow package.
///
/// # Errors
///
/// Returns an error when the package is unknown or was edited by hand.
pub fn remove(name: &str) -> Result<()> {
    packages::remove(name).map_err(to_cruise)?;
    println!("Removed {name}");
    Ok(())
}

#[expect(clippy::needless_pass_by_value, reason = "used directly with map_err")]
fn to_cruise(e: WorkflowPackageError) -> CruiseError {
    CruiseError::Other(e.to_string())
}

/// Print the preview, then require confirmation (`--yes` or an interactive `y`).
fn confirm(prepared: &PreparedPackage, yes: bool) -> Result<()> {
    println!("{}", prepared.preview());
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(to_cruise(WorkflowPackageError::ConfirmationRequired));
    }
    print!("Install this workflow? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(to_cruise(WorkflowPackageError::ConfirmationRequired))
    }
}

fn install(prepared: PreparedPackage, yes: bool, verb: &str) -> Result<()> {
    confirm(&prepared, yes)?;
    let installed = packages::commit_install(prepared).map_err(to_cruise)?;
    println!(
        "{verb} {} ({})",
        installed.name,
        installed.config_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CruiseError;

    const FULL_SHA: &str = "1111111111111111111111111111111111111111";

    fn pkg(name: &str, requested_ref: &str, commit: &str) -> InstalledPackage {
        InstalledPackage {
            name: name.to_string(),
            owner: "org".to_string(),
            repo: "repo".to_string(),
            workflow_path: "cruise.yaml".to_string(),
            requested_ref: requested_ref.to_string(),
            resolved_commit: commit.to_string(),
            config_path: PathBuf::from(format!("/cfg/{name}.yaml")),
        }
    }

    fn render(installed: &[InstalledPackage]) -> Vec<String> {
        render_list(BUILTIN_WORKFLOWS, installed, |p| p.display().to_string())
            .iter()
            .map(|l| console::strip_ansi_codes(l).into_owned())
            .collect()
    }

    fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
        lines
            .iter()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle}: {lines:?}"))
    }

    #[test]
    fn render_list_without_packages_shows_sections_and_hint() {
        let lines = render(&[]);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("Built-in workflows")
        );
        assert!(lines.iter().any(|l| l == "Installed packages"));
        assert!(lines.iter().any(|l| l.contains("(none)")));
        assert!(lines.iter().any(|l| l.contains("cruise workflow add")));
        for workflow in BUILTIN_WORKFLOWS {
            assert!(
                line_with(&lines, workflow.description).contains(workflow.name),
                "{} missing from listing: {lines:?}",
                workflow.name
            );
        }
    }

    #[test]
    fn render_list_aligns_builtin_descriptions() {
        let lines = render(&[]);
        let cols: Vec<usize> = BUILTIN_WORKFLOWS
            .iter()
            .map(|w| {
                line_with(&lines, w.description)
                    .find(w.description)
                    .unwrap_or_else(|| panic!("missing {}", w.name))
            })
            .collect();
        assert!(cols.windows(2).all(|w| w[0] == w[1]), "{cols:?}");
    }

    #[test]
    fn render_list_marks_only_default_builtin() {
        let lines = render(&[]);
        let marked: Vec<&String> = lines.iter().filter(|l| l.contains("(default)")).collect();
        assert_eq!(marked.len(), 1, "{lines:?}");
        assert!(
            marked[0]
                .trim_start()
                .starts_with(crate::builtin_workflows::DEFAULT_BUILTIN_NAME)
        );
    }

    #[test]
    fn render_list_package_shows_source_short_sha_and_indented_path() {
        let lines = render(&[pkg("alpha", "main", FULL_SHA)]);
        let joined = lines.join("\n");
        assert!(joined.contains("org/repo/cruise.yaml@main"), "{joined}");
        assert!(joined.contains(&FULL_SHA[..7]));
        assert!(!joined.contains(FULL_SHA), "{joined}");
        let idx = lines
            .iter()
            .position(|l| l.contains("org/repo/cruise.yaml@main"))
            .unwrap_or_else(|| panic!("{joined}"));
        let path_line = lines
            .get(idx + 1)
            .unwrap_or_else(|| panic!("no path line: {joined}"));
        assert!(path_line.trim_start().starts_with("/cfg/alpha.yaml"));
        let name_col = lines[idx].find("alpha").unwrap_or(usize::MAX);
        let name_width = BUILTIN_WORKFLOWS
            .iter()
            .map(|w| w.name.len())
            .max()
            .unwrap_or(0)
            .max("alpha".len());
        assert_eq!(
            path_line.len() - path_line.trim_start().len(),
            name_col + name_width + 2
        );
    }

    #[test]
    fn render_list_shares_name_column_across_sections() {
        let long = "a-very-long-package-name";
        let lines = render(&[pkg(long, "main", FULL_SHA)]);
        let refactor = crate::builtin_workflows::find_builtin("refactor")
            .unwrap_or_else(|| panic!("refactor missing"));
        let col = line_with(&lines, refactor.description)
            .find(refactor.description)
            .unwrap_or(0);
        assert_eq!(col, 2 + long.len() + 2, "{lines:?}");
    }

    #[test]
    fn render_list_tolerates_short_commit() {
        let lines = render(&[pkg("alpha", "main", "abc")]);
        assert!(line_with(&lines, "org/repo").contains("abc"));
    }

    #[test]
    fn render_list_neutralizes_control_characters() {
        let clean = render(&[pkg("alpha", "main", FULL_SHA)]).len();
        let dirty = render(&[pkg("alpha", "main\nX", FULL_SHA)]);
        assert_eq!(dirty.len(), clean, "{dirty:?}");
        assert!(dirty.iter().all(|l| !l.contains('\n') && !l.contains('\r')));
    }

    #[test]
    fn render_list_has_no_trailing_spaces() {
        for installed in [
            vec![],
            vec![pkg("alpha", "main", FULL_SHA), pkg("z", "v1", "abc")],
        ] {
            for line in render(&installed) {
                assert!(!line.ends_with(' '), "trailing space: {line:?}");
            }
        }
    }

    #[test]
    fn eject_builtin_copies_each_catalog_workflow_to_new_destination() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        for workflow in BUILTIN_WORKFLOWS {
            let dest = tmp.path().join(workflow.name);
            let path = eject_builtin(workflow.name, &dest).unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(path, dest.join(format!("{}.yaml", workflow.name)));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap_or_default(),
                workflow.yaml
            );
        }
    }

    #[test]
    fn eject_builtin_does_not_overwrite_existing_file() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let existing = tmp.path().join("add-tests.yaml");
        std::fs::write(&existing, b"mine: true\n").unwrap_or_else(|e| panic!("{e:?}"));
        let err = eject_builtin("add-tests", tmp.path()).err();
        match err {
            Some(CruiseError::IoError(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected AlreadyExists IoError, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&existing).unwrap_or_default(),
            b"mine: true\n"
        );
    }

    #[test]
    fn eject_builtin_rejects_removed_names_and_other_unknown_names_without_writing() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let dest = tmp.path().join("out");
        for name in [
            "default",
            "simple",
            "review",
            "nope",
            "../escape",
            "simple/../x",
        ] {
            assert!(matches!(
                eject_builtin(name, &dest).err(),
                Some(CruiseError::Other(_))
            ));
        }
        assert!(!tmp.path().join("escape.yaml").exists());
        let entries = std::fs::read_dir(&dest).map_or(0, Iterator::count);
        assert_eq!(entries, 0);
    }
}
