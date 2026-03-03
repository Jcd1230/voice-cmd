use anyhow::{Context, Result};
use std::path::Path;
use tempfile::NamedTempFile;

pub fn download_to_path(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create destination directory {}",
                parent.display()
            )
        })?;
    }

    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow::anyhow!("failed to download {url}: {err}"))?;

    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid destination path: {}", dest.display()))?;
    let mut tmp = NamedTempFile::new_in(parent).context("failed to create temp file")?;
    let mut reader = response.into_reader();
    std::io::copy(&mut reader, &mut tmp).context("failed to write downloaded file")?;
    tmp.persist(dest)
        .map_err(|e| anyhow::anyhow!("failed to persist file: {}", e.error))?;
    Ok(())
}
