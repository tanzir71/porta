use keyring::{Entry, Error};

const SERVICE: &str = "com.porta.app";

#[cfg(target_os = "windows")]
const READ_ERROR: &str =
    "Porta couldn't read this share's password from Credential Manager. Unlock Windows, then try again.";
#[cfg(target_os = "windows")]
const WRITE_ERROR: &str =
    "Porta couldn't save this password in Credential Manager. Unlock Windows, then try again.";
#[cfg(target_os = "windows")]
const DELETE_ERROR: &str =
    "Porta couldn't remove this password from Credential Manager. Unlock Windows, then try again.";

#[cfg(target_os = "macos")]
const READ_ERROR: &str =
    "Porta couldn't read this share's password from Keychain. Unlock your Mac, then try again.";
#[cfg(target_os = "macos")]
const WRITE_ERROR: &str =
    "Porta couldn't save this password in Keychain. Unlock your Mac, then try again.";
#[cfg(target_os = "macos")]
const DELETE_ERROR: &str =
    "Porta couldn't remove this password from Keychain. Unlock your Mac, then try again.";

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const READ_ERROR: &str =
    "Porta couldn't read this share's password from your credential store. Unlock your computer, then try again.";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const WRITE_ERROR: &str =
    "Porta couldn't save this password in your credential store. Unlock your computer, then try again.";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DELETE_ERROR: &str =
    "Porta couldn't remove this password from your credential store. Unlock your computer, then try again.";

pub fn get_password(share_id: &str) -> Result<Option<String>, String> {
    let entry = entry(share_id, READ_ERROR)?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(Error::NoEntry) => Ok(None),
        Err(_) => Err(READ_ERROR.to_owned()),
    }
}

pub fn replace_password(share_id: &str, password: Option<&str>) -> Result<(), String> {
    let entry = entry(
        share_id,
        if password.is_some() {
            WRITE_ERROR
        } else {
            DELETE_ERROR
        },
    )?;

    match password {
        Some(password) => entry
            .set_password(password)
            .map_err(|_| WRITE_ERROR.to_owned()),
        None => match entry.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(_) => Err(DELETE_ERROR.to_owned()),
        },
    }
}

fn entry(share_id: &str, message: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, share_id).map_err(|_| message.to_owned())
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::{get_password, replace_password};

    #[test]
    fn stores_reads_and_removes_a_password_in_windows_credential_manager() {
        let share_id = format!("porta-windows-test-{}", uuid::Uuid::new_v4());
        replace_password(&share_id, Some("correct horse battery staple"))
            .expect("Windows Credential Manager should save a password");
        assert_eq!(
            get_password(&share_id).expect("Windows Credential Manager should read a password"),
            Some("correct horse battery staple".to_owned())
        );
        replace_password(&share_id, None)
            .expect("Windows Credential Manager should remove a password");
        assert_eq!(
            get_password(&share_id).expect("removed password lookup should succeed"),
            None
        );
    }
}
