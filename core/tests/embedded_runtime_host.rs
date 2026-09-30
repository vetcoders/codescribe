use codescribe_core::agent::ThreadStore;
use codescribe_core::config::{
    Config, UserSettings, agent_turn_lease_path, install_interlock_path,
};
use codescribe_core::config::{
    keychain,
    runtime_host::{self, RuntimeHost},
};

#[test]
fn embedded_paths_and_credential_namespace_belong_to_the_host() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("Pensieve/Agent");
    runtime_host::configure(
        RuntimeHost::new(data.clone(), "test.pensieve.credentials".into()).unwrap(),
    )
    .unwrap();
    assert_eq!(Config::config_dir(), data);
    assert_eq!(Config::env_path(), data.join(".env"));
    assert_eq!(UserSettings::settings_dir(), data);
    assert!(install_interlock_path().starts_with(&data));
    assert!(agent_turn_lease_path().starts_with(&data));
    assert_eq!(keychain::credential_service(), "test.pensieve.credentials");
    let threads = ThreadStore::new().unwrap();
    assert!(
        threads
            .thread_file_path("document-turn")
            .unwrap()
            .starts_with(&data)
    );
}
