//! The platform credential store used for sync identity, epoch keys, and
//! connector credentials. Service and account names stay stable on desktop.

#[cfg(not(target_os = "android"))]
mod platform {
    use keyring::{Entry, Error};

    fn entry(service: &str, account: &str) -> Result<Entry, String> {
        Entry::new(service, account).map_err(|error| error.to_string())
    }

    pub(crate) fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        match entry(service, account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(crate) fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        entry(service, account)?
            .set_password(value)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn delete(service: &str, account: &str) -> Result<(), String> {
        match entry(service, account)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(target_os = "android")]
mod platform {
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize)]
    struct KeyValue {
        value: Option<String>,
    }

    pub(crate) fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(crate::mobile::native()?
            .run_mobile_plugin::<KeyValue>(
                "readSecret",
                json!({ "service": service, "account": account }),
            )
            .map_err(|error| error.to_string())?
            .value)
    }

    pub(crate) fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        crate::mobile::native()?
            .run_mobile_plugin(
                "writeSecret",
                json!({ "service": service, "account": account, "value": value }),
            )
            .map_err(|error| error.to_string())
    }

    pub(crate) fn delete(service: &str, account: &str) -> Result<(), String> {
        crate::mobile::native()?
            .run_mobile_plugin(
                "deleteSecret",
                json!({ "service": service, "account": account }),
            )
            .map_err(|error| error.to_string())
    }
}

pub(crate) use platform::{delete, get, set};
