//! Detached best-effort notification sounds.
#![expect(dead_code, reason = "shared by binary and library notification paths")]

use crate::desktop_notifications::WorkflowNotificationKind;
use std::io::Read;
use std::process::{Command, Stdio};

pub(crate) fn play_commands(kind: WorkflowNotificationKind, os: &str) -> Vec<Vec<String>> {
    let (mac, linux) = match kind {
        WorkflowNotificationKind::Completed => ("Glass", "complete.oga"),
        WorkflowNotificationKind::Failed => ("Basso", "dialog-error.oga"),
        WorkflowNotificationKind::ActionRequired => ("Sosumi", "dialog-warning.oga"),
        WorkflowNotificationKind::PlanReady => ("Pop", "message.oga"),
    };
    match os {
        "macos" => vec![vec![
            "afplay".into(),
            format!("/System/Library/Sounds/{mac}.aiff"),
        ]],
        "linux" => {
            let path = format!("/usr/share/sounds/freedesktop/stereo/{linux}");
            vec![
                vec!["paplay".into(), path.clone()],
                vec!["ogg123".into(), path],
            ]
        }
        _ => Vec::new(),
    }
}

pub(crate) fn play(kind: WorkflowNotificationKind) {
    let commands = play_commands(kind, std::env::consts::OS);
    play_with_commands(&commands);
}

fn play_with_commands(commands: &[Vec<String>]) {
    let Some(first) = commands.first() else { return };
    // A separate supervisor owns the Linux players, so fallback still runs if
    // the CLI exits before the first player finishes. The handshake ensures the
    // first player has been launched before returning to the caller.
    let child = if let Some(fallback) = commands.get(1) {
        let mut child = match Command::new("/bin/sh")
            .args(["-c", "if command -v \"$1\" >/dev/null 2>&1; then \"$1\" \"$2\" >/dev/null & pid=$!; printf x; wait \"$pid\" || \"$3\" \"$2\" >/dev/null; else \"$3\" \"$2\" >/dev/null & pid=$!; printf x; wait \"$pid\"; fi", "sound", &first[0], &first[1], &fallback[0]])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return,
        };
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_exact(&mut [0]);
        }
        child
    } else {
        match Command::new(&first[0])
            .args(&first[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return,
        }
    };
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn fallback_after_spawn_or_playback_failure_is_nonblocking() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let fallback = dir.path().join("fallback");
        std::fs::write(&fallback, "#!/bin/sh\necho fallback >> \"$1\"\n").unwrap();
        std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
        let failing = dir.path().join("failing");
        std::fs::write(&failing, "#!/bin/sh\nsleep 1\nexit 1\n").unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        for first in [dir.path().join("missing"), failing] {
            let before = std::fs::read_to_string(&marker).unwrap_or_default().lines().count();
            let commands = vec![vec![first.to_string_lossy().into(), marker.to_string_lossy().into()], vec![fallback.to_string_lossy().into(), marker.to_string_lossy().into()]];
            let start = Instant::now();
            play_with_commands(&commands);
            assert!(start.elapsed() < Duration::from_millis(500), "playback blocked dispatch");
            let deadline = Instant::now() + Duration::from_secs(5);
            while std::fs::read_to_string(&marker).unwrap_or_default().lines().count() == before {
                assert!(Instant::now() < deadline, "fallback did not run");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn sound_commands_cover_events_and_platforms() {
        for (kind, mac, linux) in [
            (WorkflowNotificationKind::Completed, "Glass", "complete.oga"),
            (
                WorkflowNotificationKind::Failed,
                "Basso",
                "dialog-error.oga",
            ),
            (
                WorkflowNotificationKind::ActionRequired,
                "Sosumi",
                "dialog-warning.oga",
            ),
            (WorkflowNotificationKind::PlanReady, "Pop", "message.oga"),
        ] {
            assert_eq!(
                play_commands(kind, "macos"),
                vec![vec![
                    "afplay".to_string(),
                    format!("/System/Library/Sounds/{mac}.aiff")
                ]]
            );
            let path = format!("/usr/share/sounds/freedesktop/stereo/{linux}");
            assert_eq!(
                play_commands(kind, "linux"),
                vec![
                    vec!["paplay".to_string(), path.clone()],
                    vec!["ogg123".to_string(), path]
                ]
            );
            assert!(play_commands(kind, "windows").is_empty());
            assert!(play_commands(kind, "freebsd").is_empty());
        }
    }
}
