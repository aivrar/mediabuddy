use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub images: PathBuf,
    pub originals: PathBuf,
    pub thumbs: PathBuf,
    pub videos: PathBuf,
    pub videos_originals: PathBuf,
    pub videos_thumbs: PathBuf,
    pub db: PathBuf,
    pub config: PathBuf,
    pub config_file: PathBuf,
    pub theme_file: PathBuf,
    pub models: PathBuf,
    pub logs: PathBuf,
}

/// True if a file can actually be created in `dir`. Directory creation alone
/// is not proof: an existing read-only folder passes `create_dir_all`.
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        // An explicit override is used as given; failing there is an error
        // the user should see rather than a silent move elsewhere.
        if let Ok(custom) = std::env::var("MEDIABUDDY_DATA_DIR") {
            return Self::ensure_dirs(&PathBuf::from(custom));
        }
        if let Ok(custom) = std::env::var("IMAGEBUDDY_DATA_DIR") {
            // Backwards compat for the old name.
            return Self::ensure_dirs(&PathBuf::from(custom));
        }

        // Portable default: `data/` beside the executable.
        let exe = std::env::current_exe()?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| AppError::other("executable has no parent directory"))?;
        let portable = exe_dir.join("data");
        let portable_err = match Self::ensure_dirs(&portable) {
            Ok(paths) if dir_is_writable(&paths.root) => return Ok(paths),
            Ok(_) => "directory is not writable".to_string(),
            Err(e) => e.to_string(),
        };

        // The executable lives somewhere read-only (a per-machine install
        // under Program Files, for example). Fall back to the per-user
        // application data directory instead of failing to start.
        let fallback = directories::BaseDirs::new()
            .map(|dirs| dirs.data_local_dir().join("MediaBuddy").join("data"))
            .ok_or_else(|| {
                AppError::other(format!(
                    "cannot use {} ({portable_err}) and no per-user data directory is available",
                    portable.display()
                ))
            })?;
        eprintln!(
            "Media Buddy: {} is not usable ({portable_err}); using {} instead",
            portable.display(),
            fallback.display()
        );
        Self::ensure_dirs(&fallback)
    }

    /// Build the directory layout under an explicit root.
    #[cfg(test)]
    pub fn at(root: &Path) -> Result<Self> {
        Self::ensure_dirs(root)
    }

    fn ensure_dirs(root: &Path) -> Result<Self> {
        let images = root.join("images");
        let originals = images.join("originals");
        let thumbs = images.join("thumbs");
        let videos = root.join("videos");
        let videos_originals = videos.join("originals");
        let videos_thumbs = videos.join("thumbs");
        let config = root.join("config");
        let models = root.join("models");
        let logs = root.join("logs");
        for dir in [
            root,
            &images,
            &originals,
            &thumbs,
            &videos,
            &videos_originals,
            &videos_thumbs,
            &config,
            &models,
            &logs,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Self {
            db: root.join("images.db"),
            config_file: config.join("settings.json"),
            theme_file: config.join("theme.json"),
            root: root.to_owned(),
            images,
            originals,
            thumbs,
            videos,
            videos_originals,
            videos_thumbs,
            config,
            models,
            logs,
        })
    }
}
