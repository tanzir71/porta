use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(unix)]
use std::fs::File;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::{provider::ProviderProfile, settings::Settings, shares::Share};

const STORE_VERSION: u32 = 2;
const STORE_FILE_NAME: &str = "store.json";
const LOCK_ERROR: &str =
    "Porta's saved data is temporarily unavailable. Quit and reopen Porta, then try again.";
const PREPARE_ERROR: &str =
    "Porta couldn't prepare its data folder. Check that you have free disk space, then reopen Porta.";
const READ_ERROR: &str =
    "Porta couldn't read its saved shares. Check that Porta can access its data folder, then reopen the app.";
const PARSE_ERROR: &str = "Porta couldn't read its saved shares because the data file is damaged. Move store.json out of Porta's data folder, then reopen the app.";
const SAVE_ERROR: &str =
    "Porta couldn't save your changes. Check that you have free disk space, then try again.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    shares: Vec<Share>,
    #[serde(default)]
    settings: Settings,
    #[serde(default)]
    provider_profiles: Vec<ProviderProfile>,
}

const fn store_version() -> u32 {
    STORE_VERSION
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            shares: Vec::new(),
            settings: Settings::default(),
            provider_profiles: Vec::new(),
        }
    }
}

pub struct Store {
    path: PathBuf,
    state: Mutex<PersistedState>,
}

impl Store {
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        let data_dir = app.path().app_data_dir().map_err(|_| PREPARE_ERROR)?;
        Self::load_from_dir(&data_dir)
    }

    pub(crate) fn load_from_dir(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir).map_err(|_| PREPARE_ERROR)?;

        let path = data_dir.join(STORE_FILE_NAME);
        let temporary_path = temporary_path(&path);
        if temporary_path.exists() {
            let _ = fs::remove_file(&temporary_path);
        }

        let mut state = if path.exists() {
            let bytes = fs::read(&path).map_err(|_| READ_ERROR)?;
            serde_json::from_slice(&bytes).map_err(|_| PARSE_ERROR)?
        } else {
            let state = PersistedState::default();
            persist_state(&path, &state)?;
            state
        };
        if state.version > STORE_VERSION {
            return Err(PARSE_ERROR.to_owned());
        }
        if state.version < STORE_VERSION {
            state.version = STORE_VERSION;
            persist_state(&path, &state)?;
        }

        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn read<T>(&self, reader: impl FnOnce(&[Share], &Settings) -> T) -> Result<T, String> {
        let state = self.state.lock().map_err(|_| LOCK_ERROR)?;
        Ok(reader(&state.shares, &state.settings))
    }

    pub fn update<T>(
        &self,
        update: impl FnOnce(&mut Vec<Share>, &mut Settings) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.lock().map_err(|_| LOCK_ERROR)?;
        let mut next = state.clone();
        let output = update(&mut next.shares, &mut next.settings)?;
        persist_state(&self.path, &next)?;
        *state = next;
        Ok(output)
    }

    pub fn read_with_providers<T>(
        &self,
        reader: impl FnOnce(&[Share], &Settings, &[ProviderProfile]) -> T,
    ) -> Result<T, String> {
        let state = self.state.lock().map_err(|_| LOCK_ERROR)?;
        Ok(reader(
            &state.shares,
            &state.settings,
            &state.provider_profiles,
        ))
    }

    pub fn update_with_providers<T>(
        &self,
        update: impl FnOnce(
            &mut Vec<Share>,
            &mut Settings,
            &mut Vec<ProviderProfile>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.lock().map_err(|_| LOCK_ERROR)?;
        let mut next = state.clone();
        let output = update(
            &mut next.shares,
            &mut next.settings,
            &mut next.provider_profiles,
        )?;
        persist_state(&self.path, &next)?;
        *state = next;
        Ok(output)
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn persist_state(path: &Path, state: &PersistedState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|_| SAVE_ERROR)?;
    let temporary_path = temporary_path(path);

    let result = (|| -> std::io::Result<()> {
        let mut temporary_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)?;
        temporary_file.write_all(&bytes)?;
        temporary_file.sync_all()?;
        replace_file(&temporary_path, path)?;
        sync_parent(path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        return Err(SAVE_ERROR.to_owned());
    }

    Ok(())
}

#[cfg(unix)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> std::io::Result<()> {
    // The temporary file is flushed before Windows performs a write-through replacement.
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use tempfile::tempdir;

    use super::{temporary_path, Store, STORE_FILE_NAME};
    use crate::{
        provider::{ProviderKind, ProviderProfile, CLOUDFLARE_QUICK_ID},
        settings::Theme,
        shares::Share,
    };

    #[test]
    fn writes_atomically_and_reloads_the_last_complete_state() {
        let data_dir = tempdir().expect("temporary data directory should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("store should initialize");
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should be valid JSON");
        let share: Share = serde_json::from_value(fixture["share"].clone())
            .expect("fixture share should match the contract");

        store
            .update(|shares, settings| {
                shares.push(share.clone());
                settings.theme = Theme::Dark;
                Ok(())
            })
            .expect("state should persist");
        store
            .update(|shares, _| {
                shares[0].name = "Updated on every platform".to_owned();
                Ok(())
            })
            .expect("an existing store file should be replaced atomically");

        let store_path = data_dir.path().join(STORE_FILE_NAME);
        let saved: Value =
            serde_json::from_slice(&fs::read(&store_path).expect("saved store should be readable"))
                .expect("saved store should contain complete JSON");
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["shares"][0]["id"], share.id);
        assert_eq!(saved["shares"][0]["name"], "Updated on every platform");
        assert_eq!(saved["settings"]["theme"], "dark");
        assert!(!temporary_path(&store_path).exists());

        fs::write(temporary_path(&store_path), b"partial write")
            .expect("stale temporary file should be created");
        let reloaded =
            Store::load_from_dir(data_dir.path()).expect("last complete state should reload");
        let (shares, theme) = reloaded
            .read(|shares, settings| (shares.to_vec(), settings.theme))
            .expect("reloaded state should be readable");

        assert_eq!(shares[0].id, share.id);
        assert_eq!(shares[0].name, "Updated on every platform");
        assert_eq!(theme, Theme::Dark);
        assert!(!temporary_path(&store_path).exists());
    }

    #[test]
    fn migrates_a_1_1_store_to_cloudflare_quick_without_changing_shares() {
        let data_dir = tempdir().expect("temporary data directory should be created");
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_contract.json"))
                .expect("fixture should be valid JSON");
        let mut share = fixture["share"].clone();
        share
            .as_object_mut()
            .expect("fixture share should be an object")
            .remove("providerId");
        let mut settings = fixture["settings"].clone();
        settings
            .as_object_mut()
            .expect("fixture settings should be an object")
            .remove("defaultProviderId");
        let legacy = serde_json::json!({
            "version": 1,
            "shares": [share],
            "settings": settings
        });
        let store_path = data_dir.path().join(STORE_FILE_NAME);
        fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("legacy store should serialize"),
        )
        .expect("legacy store should be written");

        let store = Store::load_from_dir(data_dir.path()).expect("legacy store should migrate");
        let (shares, default_provider, profiles) = store
            .read_with_providers(|shares, settings, profiles| {
                (
                    shares.to_vec(),
                    settings.default_provider_id.clone(),
                    profiles.to_vec(),
                )
            })
            .expect("migrated store should be readable");
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].provider_id, None);
        assert_eq!(default_provider, CLOUDFLARE_QUICK_ID);
        assert!(profiles.is_empty());

        let saved: Value = serde_json::from_slice(
            &fs::read(&store_path).expect("migrated store should be readable from disk"),
        )
        .expect("migrated store should contain JSON");
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["settings"]["defaultProviderId"], CLOUDFLARE_QUICK_ID);
        assert!(saved["shares"][0].get("providerId").is_none());
    }

    #[test]
    fn persists_provider_profiles_without_any_secret_field() {
        let data_dir = tempdir().expect("temporary data directory should be created");
        let store = Store::load_from_dir(data_dir.path()).expect("store should initialize");
        let profile = ProviderProfile {
            id: "custom-test".to_owned(),
            name: "Custom test".to_owned(),
            kind: ProviderKind::Custom,
            executable: Some("/tmp/vendor".to_owned()),
            arguments: vec!["{origin}".to_owned()],
            public_url: None,
            url_pattern: Some("https://example\\.com".to_owned()),
            credential_env: Some("VENDOR_TOKEN".to_owned()),
            forwarded_ip_header: Some("X-Forwarded-For".to_owned()),
            local_port: None,
            created_at: "2026-07-15T00:00:00Z".to_owned(),
        };
        store
            .update_with_providers(|_, _, profiles| {
                profiles.push(profile);
                Ok(())
            })
            .expect("profile should persist");

        let saved = fs::read_to_string(data_dir.path().join(STORE_FILE_NAME))
            .expect("store should be readable");
        assert!(saved.contains("custom-test"));
        assert!(!saved.contains("\"credential\":"));
        assert!(!saved.contains("\"secret\":"));
    }
}
