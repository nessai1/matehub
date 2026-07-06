//! Персистентный список хабов пользователя (JSON в app config dir).
//!
//! Namely: список URL'ов, к которым юзер подключался, последний — первым.
//! Это dev-этап «выбора хабов»; интеграция с SaaS-реестром (general,
//! matehub.io) — отдельный этап, когда у general появится desktop-API.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const MAX_HUBS: usize = 20;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HubList {
    pub hubs: Vec<String>,
}

pub struct HubStore {
    path: PathBuf,
}

impl HubStore {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            path: config_dir.join("hubs.json"),
        }
    }

    pub fn load(&self) -> Vec<String> {
        let Ok(raw) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        match serde_json::from_str::<HubList>(&raw) {
            Ok(list) => list.hubs,
            Err(e) => {
                tracing::warn!("corrupt hubs.json ({e}), starting fresh");
                Vec::new()
            }
        }
    }

    /// Поднимает `url` в начало списка (MRU) и сохраняет.
    pub fn remember(&self, url: &str) -> std::io::Result<()> {
        let mut hubs = self.load();
        hubs.retain(|h| h != url);
        hubs.insert(0, url.to_string());
        hubs.truncate(MAX_HUBS);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&HubList { hubs })?;
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_is_mru_and_dedups() {
        let dir = std::env::temp_dir().join(format!("matehub-hubs-test-{}", std::process::id()));
        let store = HubStore::new(dir.clone());
        store.remember("https://a.example.com").unwrap();
        store.remember("https://b.example.com").unwrap();
        store.remember("https://a.example.com").unwrap();
        assert_eq!(
            store.load(),
            vec![
                "https://a.example.com".to_string(),
                "https://b.example.com".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let store = HubStore::new(std::env::temp_dir().join("matehub-hubs-nonexistent"));
        assert!(store.load().is_empty());
    }
}
