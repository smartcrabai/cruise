use std::io::{self, Write};

use crate::cli::DocsCommand;

const COMMON: &str = include_str!("../docs/e2e/common.md");

struct FrameworkPolicy {
    id: &'static str,
    label: &'static str,
    text: &'static str,
}

const FRAMEWORKS: &[FrameworkPolicy] = &[
    FrameworkPolicy {
        id: "browser",
        label: "Browser",
        text: include_str!("../docs/e2e/browser.md"),
    },
    FrameworkPolicy {
        id: "browser-extension",
        label: "Browser extension",
        text: include_str!("../docs/e2e/browser-extension.md"),
    },
    FrameworkPolicy {
        id: "swiftui",
        label: "SwiftUI / Apple native iOS and macOS",
        text: include_str!("../docs/e2e/swiftui.md"),
    },
    FrameworkPolicy {
        id: "android",
        label: "Native Android",
        text: include_str!("../docs/e2e/android.md"),
    },
    FrameworkPolicy {
        id: "react-native",
        label: "React Native / Expo",
        text: include_str!("../docs/e2e/react-native.md"),
    },
    FrameworkPolicy {
        id: "electron",
        label: "Electron desktop",
        text: include_str!("../docs/e2e/electron.md"),
    },
    FrameworkPolicy {
        id: "tauri",
        label: "Tauri native desktop",
        text: include_str!("../docs/e2e/tauri.md"),
    },
    FrameworkPolicy {
        id: "slint",
        label: "Slint desktop and embedded UI",
        text: include_str!("../docs/e2e/slint.md"),
    },
];

pub fn run(command: &DocsCommand) -> i32 {
    let mut stdout = io::stdout().lock();
    let result = match command {
        DocsCommand::E2e { framework: None } => write_common(&mut stdout),
        DocsCommand::E2e {
            framework: Some(id),
        } => {
            let Some(policy) = FRAMEWORKS.iter().find(|policy| policy.id == id) else {
                eprintln!(
                    "cruise: unknown E2E framework `{id}`; run `cruise docs e2e` to list valid framework IDs"
                );
                return 2;
            };
            stdout.write_all(policy.text.as_bytes())
        }
    };

    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("cruise: could not write E2E documentation: {error}");
            1
        }
    }
}

fn write_common(output: &mut impl Write) -> io::Result<()> {
    output.write_all(COMMON.as_bytes())?;
    write!(output, "\n## Framework policies\n\n")?;
    for policy in FRAMEWORKS {
        writeln!(output, "- `{}` — {}", policy.id, policy.label)?;
    }
    writeln!(
        output,
        "\nUse `cruise docs e2e <id>` to retrieve one framework policy."
    )
}
