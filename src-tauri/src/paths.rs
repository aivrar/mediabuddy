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

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let root = if let Ok(custom) = std::env::var("MEDIABUDDY_DATA_DIR") {
            PathBuf::from(custom)
        } else if let Ok(custom) = std::env::var("IMAGEBUDDY_DATA_DIR") {
            // Backwards compat for the old name.
            PathBuf::from(custom)
        } else {
            let exe = std::env::current_exe()?;
            let exe_dir = exe
                .parent()
                .ok_or_else(|| AppError::other("executable has no parent directory"))?;
            exe_dir.join("data")
        };
        Self::ensure_dirs(&root)
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
