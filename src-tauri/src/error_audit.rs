const SOURCES: &[(&str, &str)] = &[
    ("commands.rs", include_str!("commands.rs")),
    ("credentials.rs", include_str!("credentials.rs")),
    ("lib.rs", include_str!("lib.rs")),
    ("server.rs", include_str!("server.rs")),
    ("store.rs", include_str!("store.rs")),
    ("tray.rs", include_str!("tray.rs")),
    ("tunnel.rs", include_str!("tunnel.rs")),
];

const ERROR_PREFIXES: &[&str] = &[
    "Porta can't",
    "Porta could",
    "Porta saved",
    "Porta's saved",
    "Couldn't",
    "That ",
    "The tunnel",
    "The upload",
    "This ",
    "Uploads ",
    "One file",
    "Choose ",
    "Enter ",
    "Give ",
];

const ACTION_CUES: &[&str] = &[
    "ask ",
    "check ",
    "choose ",
    "close ",
    "continue",
    "copy ",
    "edit ",
    "enter ",
    "move ",
    "open ",
    "pick ",
    "quit ",
    "reinstall ",
    "rename ",
    "reopen ",
    "restart ",
    "return ",
    "toggle ",
    "try again",
    "turn ",
    "unlock ",
    "wait ",
];

#[test]
fn user_facing_errors_stay_plain_english_actionable_and_debug_free() {
    let mut audited = 0;
    for (name, source) in SOURCES {
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            !production_source.contains("\"Error:"),
            "{name} uses Error:"
        );
        assert!(
            !production_source.contains("\"error:"),
            "{name} uses error:"
        );
        assert!(
            !production_source.contains("{error:?}"),
            "{name} exposes a debug error"
        );

        for line in production_source.lines() {
            let Some(start) = line.find('"') else {
                continue;
            };
            let Some(end) = line.rfind('"').filter(|end| *end > start) else {
                continue;
            };
            let message = &line[start + 1..end];
            if !ERROR_PREFIXES
                .iter()
                .any(|prefix| message.starts_with(prefix))
                || message == "This folder is empty"
            {
                continue;
            }

            audited += 1;
            let lower = message.to_lowercase();
            assert!(
                ACTION_CUES.iter().any(|cue| lower.contains(cue)),
                "{name} has no next action: {message}"
            );
            assert!(
                !message.contains('/') && !message.contains("\\"),
                "{name} exposes a filesystem path: {message}"
            );
        }
    }
    assert!(
        audited >= 30,
        "the audit unexpectedly found only {audited} errors"
    );
}
