use assi_core::{DeviceInput, Group, Os, Settings, Store, StoreError, Theme};
fn input() -> DeviceInput {
    DeviceInput {
        name: "  Рабочий ПК  ".into(),
        code: "123 456 789".into(),
        os: Os::Windows,
        group: Group::Work,
    }
}

#[test]
fn persists_identity_book_and_settings_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite3");
    let id;
    {
        let mut store = Store::open(&path, "Этот Mac").unwrap();
        id = store.snapshot().unwrap().installation_id;
        let device = store.add_device(input()).unwrap();
        assert_eq!(device.name, "Рабочий ПК");
        store
            .update_settings(Settings {
                theme: Theme::Light,
                device_name: "Мой ноутбук".into(),
            })
            .unwrap();
    }
    let store = Store::open(&path, "Ignored").unwrap();
    let data = store.snapshot().unwrap();
    assert_eq!(data.installation_id, id);
    assert_eq!(data.devices.len(), 1);
    assert_eq!(data.devices[0].code, "123456789");
    assert_eq!(data.settings.theme, Theme::Light);
    assert_eq!(data.settings.device_name, "Мой ноутбук");
}
#[test]
fn rejects_invalid_input_and_duplicate_without_mutation() {
    let mut store = Store::open(":memory:", "Тест").unwrap();
    for bad in [
        "",
        "12345678",
        "123456789x",
        "１２３４５６７８９",
        "123\n456789",
    ] {
        let mut value = input();
        value.code = bad.into();
        assert!(matches!(store.add_device(value), Err(StoreError::Code)));
    }
    for bad in [" ".into(), "а".repeat(61), "bad\0name".into()] {
        let mut value = input();
        value.name = bad;
        assert!(matches!(store.add_device(value), Err(StoreError::Name)));
    }
    assert!(store.snapshot().unwrap().devices.is_empty());
    store.add_device(input()).unwrap();
    assert!(matches!(
        store.add_device(input()),
        Err(StoreError::Duplicate)
    ));
    assert_eq!(store.snapshot().unwrap().devices.len(), 1);
}
#[test]
fn remove_is_persistent_and_sql_text_is_data() {
    let mut store = Store::open(":memory:", "Тест").unwrap();
    let mut value = input();
    value.name = "'); DROP TABLE devices; --".into();
    let d = store.add_device(value).unwrap();
    assert_eq!(store.snapshot().unwrap().devices[0].name, d.name);
    store.remove_device(&d.id).unwrap();
    assert!(store.snapshot().unwrap().devices.is_empty());
    assert!(matches!(
        store.remove_device(&d.id),
        Err(StoreError::NotFound)
    ));
}
#[test]
fn newer_schema_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    assert!(matches!(
        Store::open(&path, "Тест"),
        Err(StoreError::NewerSchema)
    ));
    let db = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        99
    );
}
#[test]
fn invalid_settings_do_not_partially_change_theme() {
    let mut store = Store::open(":memory:", "Тест").unwrap();
    assert!(store
        .update_settings(Settings {
            theme: Theme::Light,
            device_name: " ".into()
        })
        .is_err());
    assert_eq!(store.snapshot().unwrap().settings.theme, Theme::Dark);
}
#[test]
fn ipc_rejects_unknown_platform_and_group() {
    assert!(serde_json::from_str::<DeviceInput>(
        r#"{"name":"a","code":"123456789","os":"Unknown","group":"personal"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<DeviceInput>(
        r#"{"name":"a","code":"123456789","os":"Linux","group":"admin"}"#
    )
    .is_err());
}
