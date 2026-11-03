use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Укажите название от 1 до 60 символов без управляющих знаков.")]
    Name,
    #[error("Код должен содержать ровно 9 цифр.")]
    Code,
    #[error("Устройство с таким кодом уже сохранено.")]
    Duplicate,
    #[error("Устройство не найдено. Обновите список.")]
    NotFound,
    #[error("База создана более новой версией Асси. Обновите приложение.")]
    NewerSchema,
    #[error("Не удалось прочитать сохранённые данные.")]
    InvalidData,
    #[error("Ошибка локального хранилища: {0}")]
    Database(#[from] rusqlite::Error),
}

macro_rules! text_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $text)] $variant),+ }
        impl $name {
            fn as_str(self) -> &'static str { match self { $(Self::$variant => $text),+ } }
            fn parse(value: &str) -> Result<Self, StoreError> {
                match value { $($text => Ok(Self::$variant)),+, _ => Err(StoreError::InvalidData) }
            }
        }
    }
}
text_enum!(Os { Windows => "Windows", Mac => "macOS", Linux => "Linux", Android => "Android" });
text_enum!(Group { Personal => "personal", Work => "work" });
text_enum!(Theme { Dark => "dark", Light => "light" });

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInput {
    pub name: String,
    pub code: String,
    pub os: Os,
    pub group: Group,
}
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub code: String,
    pub os: Os,
    pub group: Group,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub theme: Theme,
    pub device_name: String,
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub installation_id: String,
    pub settings: Settings,
    pub devices: Vec<Device>,
}

pub struct Store {
    db: Connection,
}

fn validate_name(name: &str) -> Result<&str, StoreError> {
    let value = name.trim();
    if value.is_empty() || value.chars().count() > 60 || value.chars().any(char::is_control) {
        return Err(StoreError::Name);
    }
    Ok(value)
}

impl Store {
    pub fn open(path: impl AsRef<Path>, default_name: &str) -> Result<Self, StoreError> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(3))?;
        let tx = db.transaction()?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(StoreError::NewerSchema);
        }
        if version == 0 {
            let name = validate_name(default_name)?;
            tx.execute_batch(
                "CREATE TABLE profile (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                installation_id TEXT NOT NULL,
                device_name TEXT NOT NULL,
                theme TEXT NOT NULL CHECK(theme IN ('dark','light')));
                CREATE TABLE devices (
                id TEXT PRIMARY KEY, name TEXT NOT NULL,
                code TEXT NOT NULL UNIQUE CHECK(length(code)=9 AND code NOT GLOB '*[^0-9]*'),
                os TEXT NOT NULL CHECK(os IN ('Windows','macOS','Linux','Android')),
                device_group TEXT NOT NULL CHECK(device_group IN ('personal','work')));",
            )?;
            tx.execute(
                "INSERT INTO profile VALUES (1, ?1, ?2, 'dark')",
                params![Uuid::new_v4().to_string(), name],
            )?;
            tx.pragma_update(None, "user_version", 1)?;
        }
        tx.commit()?;
        let store = Self { db };
        store.snapshot()?;
        Ok(store)
    }

    pub fn snapshot(&self) -> Result<Snapshot, StoreError> {
        let (installation_id, name, theme): (String, String, String) = self.db.query_row(
            "SELECT installation_id, device_name, theme FROM profile WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Uuid::parse_str(&installation_id).map_err(|_| StoreError::InvalidData)?;
        validate_name(&name)?;
        let mut stmt = self
            .db
            .prepare("SELECT id, name, code, os, device_group FROM devices ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut devices = Vec::new();
        for row in rows {
            let (id, name, code, os, group) = row?;
            Uuid::parse_str(&id).map_err(|_| StoreError::InvalidData)?;
            validate_name(&name)?;
            if code.len() != 9 || !code.bytes().all(|c| c.is_ascii_digit()) {
                return Err(StoreError::InvalidData);
            }
            devices.push(Device {
                id,
                name,
                code,
                os: Os::parse(&os)?,
                group: Group::parse(&group)?,
            });
        }
        Ok(Snapshot {
            installation_id,
            settings: Settings {
                theme: Theme::parse(&theme)?,
                device_name: name,
            },
            devices,
        })
    }

    pub fn add_device(&mut self, input: DeviceInput) -> Result<Device, StoreError> {
        let name = validate_name(&input.name)?.to_string();
        // Only ordinary spaces are accepted as formatting; letters must not silently disappear.
        let code = input.code.replace(' ', "");
        if code.len() != 9 || !code.bytes().all(|c| c.is_ascii_digit()) {
            return Err(StoreError::Code);
        }
        let tx = self.db.transaction()?;
        let existing = tx
            .query_row("SELECT id FROM devices WHERE code=?1", [&code], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        if existing.is_some() {
            return Err(StoreError::Duplicate);
        }
        let device = Device {
            id: Uuid::new_v4().to_string(),
            name,
            code,
            os: input.os,
            group: input.group,
        };
        tx.execute(
            "INSERT INTO devices (id,name,code,os,device_group) VALUES (?1,?2,?3,?4,?5)",
            params![
                device.id,
                device.name,
                device.code,
                device.os.as_str(),
                device.group.as_str()
            ],
        )?;
        tx.commit()?;
        Ok(device)
    }

    pub fn remove_device(&mut self, id: &str) -> Result<(), StoreError> {
        if self.db.execute("DELETE FROM devices WHERE id=?1", [id])? == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub fn update_settings(&mut self, settings: Settings) -> Result<Settings, StoreError> {
        let name = validate_name(&settings.device_name)?.to_string();
        self.db.execute(
            "UPDATE profile SET device_name=?1, theme=?2 WHERE singleton=1",
            params![name, settings.theme.as_str()],
        )?;
        Ok(Settings {
            device_name: name,
            ..settings
        })
    }
}
