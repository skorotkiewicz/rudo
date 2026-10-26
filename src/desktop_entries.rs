//! Temporary desktop entries for tests that must not depend on installed applications.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gio_unix::DesktopAppInfo;
use gtk4::gio;
use gtk4::prelude::*;

use crate::catalog::AppCatalog;

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

pub(crate) struct DesktopEntries {
    directory: PathBuf,
    apps: Vec<gio::AppInfo>,
}

impl DesktopEntries {
    pub(crate) fn new(entries: &[(&str, &str)]) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "rudo-catalog-test-{}-{}",
            std::process::id(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).expect("test directory must be unique");
        let apps = entries
            .iter()
            .map(|(id, entry)| {
                let path = directory.join(id);
                fs::write(
                    &path,
                    format!("[Desktop Entry]\nType=Application\n{entry}\n"),
                )
                .expect("desktop fixture must be writable");
                DesktopAppInfo::from_filename(path)
                    .expect("fixture must be a valid desktop entry")
                    .upcast()
            })
            .collect();
        Self { directory, apps }
    }

    pub(crate) fn catalog(&self) -> AppCatalog {
        AppCatalog::from_apps(self.apps.clone())
    }
}

impl Drop for DesktopEntries {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).expect("test directory must be removable");
    }
}
