use std::path::{Path, PathBuf};

use tauri::{DragDropEvent, Runtime, Webview, WebviewEvent};

const HOVER_SCRIPT: &str = "window.dispatchEvent(new CustomEvent('porta:drag-hover'));";
const CANCEL_SCRIPT: &str = "window.dispatchEvent(new CustomEvent('porta:drag-cancel'));";

pub(crate) fn handle<R: Runtime>(webview: &Webview<R>, event: &WebviewEvent) {
    let WebviewEvent::DragDrop(event) = event else {
        return;
    };

    let script = match event {
        DragDropEvent::Enter { .. } | DragDropEvent::Over { .. } => Some(HOVER_SCRIPT.to_owned()),
        DragDropEvent::Leave => Some(CANCEL_SCRIPT.to_owned()),
        DragDropEvent::Drop { paths, .. } => first_dropped_directory(paths)
            .and_then(folder_drop_script)
            .or_else(|| Some(CANCEL_SCRIPT.to_owned())),
        _ => None,
    };
    if let Some(script) = script {
        let _ = webview.eval(script);
    }
}

fn first_dropped_directory(paths: &[PathBuf]) -> Option<&Path> {
    paths
        .iter()
        .find(|path| path.is_dir())
        .map(PathBuf::as_path)
}

fn folder_drop_script(path: &Path) -> Option<String> {
    let absolute_path = path.to_str()?;
    let detail = serde_json::to_string(absolute_path).ok()?;
    Some(format!(
        "window.dispatchEvent(new CustomEvent('porta:folder-dropped', {{ detail: {detail} }}));"
    ))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::{first_dropped_directory, folder_drop_script};

    #[test]
    fn selects_the_first_real_directory_and_ignores_plain_files() {
        let root = tempdir().expect("temporary root should be created");
        let file = root.path().join("plain.txt");
        let first = root.path().join("First Folder");
        let second = root.path().join("Second Folder");
        fs::write(&file, b"file").expect("plain file should be written");
        fs::create_dir(&first).expect("first directory should be created");
        fs::create_dir(&second).expect("second directory should be created");

        assert_eq!(
            first_dropped_directory(&[file.clone(), first.clone(), second]),
            Some(first.as_path())
        );
        assert_eq!(first_dropped_directory(&[file]), None);
    }

    #[test]
    fn dispatches_the_exact_absolute_path_as_json_safe_event_detail() {
        let path = Path::new("/Users/you/Client's “Final” 文件");
        let script = folder_drop_script(path).expect("Unicode path should become a script");

        assert_eq!(
            script,
            "window.dispatchEvent(new CustomEvent('porta:folder-dropped', { detail: \"/Users/you/Client's “Final” 文件\" }));"
        );
    }
}
