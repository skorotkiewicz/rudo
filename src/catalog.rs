use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use gtk4::gio;
use gtk4::gio::prelude::*;
use gtk4::glib;

/// Match quality for search ranking (lower = better match)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchQuality {
    Exact = 0,
    Prefix = 1,
    Fuzzy = 2,
}

#[derive(Clone)]
pub struct AppRecord {
    pub id: String,
    pub name: String,
    pub icon: Option<gio::Icon>,
    #[allow(dead_code)]
    pub startup_wm_class: Option<String>,
    #[allow(dead_code)]
    pub executable: Option<String>,
    app_info: gio::AppInfo,
    // Pre-normalized searchable fields to avoid repeated normalization
    search_keys: Vec<String>,
}

impl AppRecord {
    pub fn launch(&self) -> Result<(), glib::Error> {
        self.app_info.launch(&[], None::<&gio::AppLaunchContext>)
    }
}

pub struct AppCatalog {
    apps: BTreeMap<String, AppRecord>,
    aliases: HashMap<String, String>,
    ordered_ids: Vec<String>,
}

impl AppCatalog {
    pub fn load() -> Self {
        let mut apps = BTreeMap::new();
        let mut ordered = Vec::new();
        let mut aliases = HashMap::new();

        let mut installed = gio::AppInfo::all();
        installed.sort_by_cached_key(|app| app.display_name().to_string().to_lowercase());

        for app in installed {
            if !app.should_show() {
                continue;
            }

            let Some(id) = app.id().map(|id| id.to_string()) else {
                continue;
            };

            let icon = app.icon();
            let executable = basename(app.executable());
            let startup_wm_class = None;
            let name = app.display_name().to_string();

            // Pre-normalize searchable fields for faster matching
            let mut search_keys = vec![normalize_key(&id), normalize_key(&name)];
            if let Some(wm_class) = startup_wm_class.as_deref() {
                search_keys.push(normalize_key(wm_class));
            }
            if let Some(exec) = executable.as_deref() {
                search_keys.push(normalize_key(exec));
            }

            let record = AppRecord {
                id: id.clone(),
                name,
                icon,
                startup_wm_class: startup_wm_class.clone(),
                executable: executable.clone(),
                app_info: app,
                search_keys,
            };

            register_alias(&mut aliases, &id, &id);
            if let Some(wm_class) = startup_wm_class.as_deref() {
                register_alias(&mut aliases, wm_class, &id);
            }
            if let Some(exec) = executable.as_deref() {
                register_alias(&mut aliases, exec, &id);
            }
            register_alias(&mut aliases, &record.name, &id);

            ordered.push(id.clone());
            apps.insert(id, record);
        }

        Self {
            apps,
            aliases,
            ordered_ids: ordered,
        }
    }

    pub fn app(&self, id: &str) -> Option<AppRecord> {
        let canonical = self.resolve_id(id)?;
        self.apps.get(&canonical).cloned()
    }

    pub fn resolve(&self, raw_app_id: Option<&str>) -> Option<AppRecord> {
        let raw = raw_app_id?;
        let canonical = self.resolve_id(raw)?;
        self.apps.get(&canonical).cloned()
    }

    pub fn search(
        &self,
        query: &str,
        limit: usize,
        exclude_ids: &HashSet<String>,
    ) -> Vec<AppRecord> {
        // Fast path: empty query returns all apps (used by picker)
        if query.is_empty() {
            return self
                .ordered_ids
                .iter()
                .filter(|id| !exclude_ids.contains(*id))
                .filter_map(|id| self.apps.get(id))
                .take(limit)
                .cloned()
                .collect();
        }

        let normalized_query = normalize_key(query);
        let mut results: Vec<(MatchQuality, AppRecord)> = Vec::new();

        for id in &self.ordered_ids {
            if exclude_ids.contains(id) {
                continue;
            }

            let Some(app) = self.apps.get(id) else {
                continue;
            };

            // Determine match quality using pre-normalized search keys
            let quality = app.search_keys.iter().find_map(|key| {
                if key == &normalized_query {
                    Some(MatchQuality::Exact)
                } else if key.starts_with(&normalized_query) {
                    Some(MatchQuality::Prefix)
                } else if key.contains(&normalized_query) {
                    Some(MatchQuality::Fuzzy)
                } else {
                    None
                }
            });

            if let Some(q) = quality {
                results.push((q, app.clone()));
            }
        }

        // Sort by match quality (lower ordinal = better match)
        results.sort_by_key(|(q, _)| *q);
        results
            .into_iter()
            .map(|(_, app)| app)
            .take(limit)
            .collect()
    }

    fn resolve_id(&self, raw: &str) -> Option<String> {
        if self.apps.contains_key(raw) {
            return Some(raw.to_string());
        }

        let desktop_id = if raw.ends_with(".desktop") {
            raw.to_string()
        } else {
            format!("{raw}.desktop")
        };

        if self.apps.contains_key(&desktop_id) {
            return Some(desktop_id);
        }

        self.aliases.get(&normalize_key(raw)).cloned()
    }
}

fn register_alias(aliases: &mut HashMap<String, String>, alias: &str, id: &str) {
    let normalized = normalize_key(alias);
    if normalized.is_empty() {
        return;
    }

    aliases.entry(normalized).or_insert_with(|| id.to_string());
}

fn normalize_key(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(".desktop")
        .replace(['_', ' '], "-")
        .to_lowercase()
}

fn basename(path: impl AsRef<Path>) -> Option<String> {
    path.as_ref()
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
}
