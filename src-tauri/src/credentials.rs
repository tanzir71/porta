use keyring::{Entry, Error};

const SERVICE: &str = "com.porta.app";
const READ_ERROR: &str =
    "Porta couldn't read this share's password from Keychain. Unlock your Mac, then try again.";
const WRITE_ERROR: &str =
    "Porta couldn't save this password in Keychain. Unlock your Mac, then try again.";
const DELETE_ERROR: &str =
    "Porta couldn't remove this password from Keychain. Unlock your Mac, then try again.";

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
