//! Process identity for applications embedding the engine.
//!
//! Install before constructing any engine handle. The first path or credential
//! lookup seals the identity, so an initialized cache cannot switch owners.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Result, ensure};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHost {
    pub data_directory: PathBuf,
    pub keychain_service: String,
}

static HOST: OnceLock<Option<RuntimeHost>> = OnceLock::new();

impl RuntimeHost {
    pub fn new(data_directory: PathBuf, keychain_service: String) -> Result<Self> {
        ensure!(
            data_directory.is_absolute(),
            "host data directory must be absolute"
        );
        ensure!(
            !keychain_service.trim().is_empty(),
            "host Keychain service must not be empty"
        );
        Ok(Self {
            data_directory,
            keychain_service,
        })
    }
}

pub fn configure(host: RuntimeHost) -> Result<()> {
    configure_slot(&HOST, host)
}

fn configure_slot(slot: &OnceLock<Option<RuntimeHost>>, host: RuntimeHost) -> Result<()> {
    ensure!(
        slot.get_or_init(|| Some(host.clone())).as_ref() == Some(&host),
        "engine runtime identity is already initialized for another host"
    );
    Ok(())
}

pub fn selected() -> Option<&'static RuntimeHost> {
    HOST.get_or_init(|| None).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_identity_is_idempotent_but_cannot_change_after_use() {
        let slot = OnceLock::new();
        let host = RuntimeHost::new(
            "/tmp/pensieve-agent".into(),
            "io.vetcoders.pensieve.agent".into(),
        )
        .unwrap();
        configure_slot(&slot, host.clone()).unwrap();
        configure_slot(&slot, host.clone()).unwrap();
        let other = RuntimeHost::new("/tmp/another-app".into(), "another.app".into()).unwrap();
        assert!(configure_slot(&slot, other).is_err());
        assert_eq!(slot.get().unwrap().as_ref(), Some(&host));
        let used_default = OnceLock::from(None);
        assert!(configure_slot(&used_default, host).is_err());
    }

    #[test]
    fn incomplete_host_identity_is_rejected() {
        assert!(RuntimeHost::new("relative".into(), "app".into()).is_err());
        assert!(RuntimeHost::new("/tmp/app".into(), " ".into()).is_err());
    }
}
