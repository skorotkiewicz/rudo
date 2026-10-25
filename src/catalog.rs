//! Installed applications and the aliases used to identify their windows.

#![warn(missing_docs)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use gio_unix::DesktopAppInfo;
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

/// A launchable desktop application.
#[derive(Clone)]
pub struct AppRecord {
    /// Canonical desktop file ID, including the `.desktop` suffix.
    pub id: String,
    /// Localized display name.
    pub name: String,
    /// Icon supplied by the desktop entry.
    pub icon: Option<gio::Icon>,
    /// Basename of the executable, when available.
    #[allow(dead_code)]
    pub executable: Option<String>,
    app_info: gio::AppInfo,
    search_keys: Vec<String>,
}

impl AppRecord {
    /// Launches the application using its desktop entry.
    pub fn launch(&self) -> Result<(), glib::Error> {
        self.app_info.launch(&[], None::<&gio::AppLaunchContext>)
    }
}

/// Installed applications indexed by desktop ID and window identity.
/// The default catalog is empty; use [`Self::load`] to discover installed apps.
#[derive(Default)]
pub struct AppCatalog {
    apps: BTreeMap<String, AppRecord>,
    aliases: HashMap<String, String>,
    ordered_ids: Vec<String>,
}

impl AppCatalog {
    /// Loads applications visible in the current desktop environment.
    pub fn load() -> Self {
        Self::from_apps(gio::AppInfo::all())
    }

    pub(crate) fn from_apps(mut installed: Vec<gio::AppInfo>) -> Self {
        let mut apps = BTreeMap::new();
        let mut ordered = Vec::new();
        let mut aliases = HashMap::new();

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
            let name = app.display_name().to_string();

            let mut search_keys = vec![normalize_key(&id), normalize_key(&name)];
            if let Some(exec) = executable.as_deref() {
                search_keys.push(normalize_key(exec));
            }

            let record = AppRecord {
                id: id.clone(),
                name,
                icon,
                executable,
                app_info: app,
                search_keys,
            };

            ordered.push(id.clone());
            apps.insert(id, record);
        }

        // Desktop IDs and explicit window identities must win over name/Exec guesses.
        for id in &ordered {
            register_alias(&mut aliases, id, id);
        }
        for id in &ordered {
            let record = apps.get_mut(id).expect("ordered IDs belong to the catalog");
            if let Some(desktop) = record.app_info.downcast_ref::<DesktopAppInfo>() {
                for alias in [desktop.startup_wm_class(), desktop.string("X-Flatpak")]
                    .into_iter()
                    .flatten()
                {
                    register_alias(&mut aliases, &alias, id);
                    record.search_keys.push(normalize_key(&alias));
                }
            }
        }
        for id in &ordered {
            let record = &apps[id];
            if let Some(exec) = record.executable.as_deref() {
                register_alias(&mut aliases, exec, id);
            }
            register_alias(&mut aliases, &record.name, id);
        }

        Self {
            apps,
            aliases,
            ordered_ids: ordered,
        }
    }

    /// Finds an application by desktop ID or a recognized alias.
    pub fn app(&self, id: &str) -> Option<&AppRecord> {
        let canonical = self.resolve_id(id)?;
        self.apps.get(&canonical)
    }

    /// Returns up to `limit` matches, ranking exact matches before prefixes and substrings.
    /// Applications in `exclude_ids` are omitted.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        exclude_ids: &HashSet<&str>,
    ) -> Vec<&AppRecord> {
        if query.is_empty() {
            return self
                .ordered_ids
                .iter()
                .filter(|id| !exclude_ids.contains(id.as_str()))
                .filter_map(|id| self.apps.get(id))
                .take(limit)
                .collect();
        }

        let normalized_query = normalize_key(query);
        let mut results: Vec<(MatchQuality, &AppRecord)> = Vec::new();

        for id in &self.ordered_ids {
            if exclude_ids.contains(id.as_str()) {
                continue;
            }

            let Some(app) = self.apps.get(id) else {
                continue;
            };

            let quality = app
                .search_keys
                .iter()
                .filter_map(|key| {
                    if key == &normalized_query {
                        Some(MatchQuality::Exact)
                    } else if key.starts_with(&normalized_query) {
                        Some(MatchQuality::Prefix)
                    } else if key.contains(&normalized_query) {
                        Some(MatchQuality::Fuzzy)
                    } else {
                        None
                    }
                })
                .min();

            if let Some(q) = quality {
                results.push((q, app));
            }
        }

        results.sort_by_key(|(q, _)| *q);
        results
            .into_iter()
            .map(|(_, app)| app)
            .take(limit)
            .collect()
    }

    /// Resolves a compositor's app ID or WM class to a canonical desktop ID.
    pub fn resolve_id(&self, raw: &str) -> Option<String> {
        if self.apps.contains_key(raw) {
            return Some(raw.to_string());
        }

        // Fast path: check aliases without allocation first
        if let Some(id) = self.aliases.get(raw) {
            return Some(id.clone());
        }

        let desktop_id = if raw.ends_with(".desktop") {
            raw.to_string()
        } else {
            format!("{raw}.desktop")
        };

        if self.apps.contains_key(&desktop_id) {
            return Some(desktop_id);
        }

        // Check aliases with normalized key
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
        .to_lowercase()
        .trim_end_matches(".desktop")
        .replace(['_', ' '], "-")
}

fn basename(path: impl AsRef<Path>) -> Option<String> {
    path.as_ref()
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use crate::desktop_entries::DesktopEntries;
    use std::collections::HashSet;

    #[test]
    fn recognizes_obsidian_appimage_by_startup_wm_class() {
        let entries = DesktopEntries::new(&[(
            "appimagekit-obsidian.desktop",
            "Name=Obsidian AppImage\nExec=/bin/true\nStartupWMClass=obsidian",
        )]);
        let catalog = entries.catalog();
        for id in [
            "obsidian",
            "Obsidian",
            "obsidian.desktop",
            "OBSIDIAN.DESKTOP",
        ] {
            assert_eq!(
                catalog.resolve_id(id).as_deref(),
                Some("appimagekit-obsidian.desktop"),
                "window identity {id} must resolve to the AppImage launcher"
            );
        }
    }

    #[test]
    fn recognizes_flatpak_identity_from_desktop_metadata() {
        let entries = DesktopEntries::new(&[(
            "custom-obsidian.desktop",
            "Name=Notes\nExec=/bin/true\nX-Flatpak=md.obsidian.Obsidian",
        )]);
        assert_eq!(
            entries
                .catalog()
                .resolve_id("md.obsidian.Obsidian")
                .as_deref(),
            Some("custom-obsidian.desktop")
        );
    }

    #[test]
    fn explicit_window_class_wins_over_another_apps_executable() {
        let entries = DesktopEntries::new(&[
            ("a.desktop", "Name=A helper\nExec=/bin/true"),
            (
                "notes.desktop",
                "Name=Notes\nExec=/bin/false\nStartupWMClass=true",
            ),
        ]);
        assert_eq!(
            entries.catalog().resolve_id("true").as_deref(),
            Some("notes.desktop")
        );
    }

    #[test]
    fn desktop_identity_wins_over_another_apps_window_class() {
        let entries = DesktopEntries::new(&[
            (
                "a.desktop",
                "Name=A helper\nExec=/bin/true\nStartupWMClass=notes",
            ),
            ("notes.desktop", "Name=Notes\nExec=/bin/false"),
        ]);
        assert_eq!(
            entries.catalog().resolve_id("NOTES").as_deref(),
            Some("notes.desktop")
        );
    }

    #[test]
    fn ranks_the_best_matching_key_not_the_first_matching_key() {
        let entries = DesktopEntries::new(&[
            ("notes-helper.desktop", "Name=A helper\nExec=/bin/true"),
            ("org.example.notes.desktop", "Name=Notes\nExec=/bin/false"),
        ]);
        let catalog = entries.catalog();
        let results = catalog.search("notes", 10, &HashSet::new());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "org.example.notes.desktop");
    }

    #[test]
    fn search_respects_exclusions_and_limit() {
        let entries = DesktopEntries::new(&[
            ("a.desktop", "Name=A\nExec=/bin/true"),
            ("b.desktop", "Name=B\nExec=/bin/false"),
        ]);
        let catalog = entries.catalog();
        let excluded = HashSet::from(["a.desktop"]);
        assert_eq!(catalog.search("", 1, &excluded)[0].id, "b.desktop");
        assert!(
            catalog.search("", 0, &excluded).is_empty(),
            "zero limit must yield no apps"
        );
    }

    #[test]
    fn unrelated_window_identity_stays_unknown() {
        let entries = DesktopEntries::new(&[(
            "obsidian.desktop",
            "Name=Obsidian\nExec=/bin/true\nStartupWMClass=obsidian",
        )]);
        assert!(
            entries.catalog().resolve_id("electron").is_none(),
            "generic Electron IDs must not be guessed as Obsidian"
        );
    }
}
