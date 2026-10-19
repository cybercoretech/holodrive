//! Small in-memory activity feed shared by authenticated web and device clients.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ITEMS: usize = 100;

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: u64,
    pub at: u64,
    pub kind: String,
    pub path: String,
    pub detail: String,
}

#[derive(Clone, Default)]
pub struct Store(Arc<Mutex<VecDeque<Item>>>);

impl Store {
    pub fn record(&self, kind: &str, path: &str, detail: &str) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let mut items = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if items.back().is_some_and(|last| {
            last.at == at && last.kind == kind && last.path == path && last.detail == detail
        }) {
            return;
        }
        let id = items.back().map_or(1, |last| last.id.saturating_add(1));
        items.push_back(Item {
            id,
            at,
            kind: kind.to_owned(),
            path: path.to_owned(),
            detail: detail.to_owned(),
        });
        while items.len() > MAX_ITEMS {
            items.pop_front();
        }
    }

    pub fn recent(&self) -> Vec<Item> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .rev()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Store;

    #[test]
    fn keeps_newest_first_and_caps_history() {
        let store = Store::default();
        for index in 0..125 {
            store.record("folder-change", "shared", &index.to_string());
        }
        let recent = store.recent();
        assert_eq!(recent.len(), 100);
        assert_eq!(recent[0].detail, "124");
        assert_eq!(recent.last().unwrap().detail, "25");
    }
}
