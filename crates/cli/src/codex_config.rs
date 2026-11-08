use std::path::{Path, PathBuf};

use anyhow::Context;
use toml_edit::DocumentMut;

pub fn config_toml_path() -> PathBuf {
    kikimimi_schema::paths::codex_home_dir().join("config.toml")
}

pub(crate) fn load_document(path: &Path) -> anyhow::Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .with_context(|| format!("parsing {} as TOML", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Backs up the original once (`config.toml.kikimimi-backup`), then writes atomically.
pub(crate) fn write_document(path: &Path, doc: &DocumentMut) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let backup = crate::claude_settings::backup_path(path);
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
    }
    crate::state::write_atomic(path, doc.to_string().as_bytes())
}
