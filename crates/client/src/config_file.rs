use std::path::PathBuf;

/// Platform config dir for the desktop player (installers write here).
pub fn config_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Couchlink").join("config"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|p| {
            PathBuf::from(p)
                .join("Library")
                .join("Application Support")
                .join("Couchlink")
                .join("config")
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("couchlink").join("config"))
    }
    #[cfg(not(any(windows, unix, target_os = "macos")))]
    {
        None
    }
}

pub fn read_join_url_from_config() -> Option<String> {
    let path = config_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("join_url=") {
            let url = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !url.is_empty() {
                return Some(url);
            }
        }
    }
    None
}

/// Persist the join URL so the next desktop launch can pre-fill the prompt.
pub fn write_join_url(url: &str) -> std::io::Result<()> {
    let path = config_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no config path on this platform")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!(
        "# Couchlink Player — last join link (edited on each successful prompt)\njoin_url={url}\n"
    );
    std::fs::write(path, body)
}
