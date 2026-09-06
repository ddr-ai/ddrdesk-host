use anyhow::{Context, Result};
use rand::Rng;
use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DDRDESK_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/var/lib/ddrdesk".into());
    PathBuf::from(home).join(".local/share/ddrdesk")
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("mkdir {}", p.display()))?;
    Ok(())
}

/// Always 9 digits, never a leading zero.
pub fn generate_id() -> String {
    let n: u32 = rand::thread_rng().gen_range(100_000_000..1_000_000_000);
    format!("{n:09}")
}

pub fn load_or_create_id() -> Result<String> {
    let dir = data_dir();
    ensure_dir(&dir)?;
    let path = dir.join("connection-id");
    if path.exists() {
        let s = std::fs::read_to_string(&path)?;
        let s = s.trim().to_string();
        if s.len() == 9 && s.chars().all(|c| c.is_ascii_digit()) {
            return Ok(s);
        }
    }
    let id = generate_id();
    std::fs::write(&path, format!("{id}\n"))?;
    let pretty = dir.join("CONNECTION-ID.txt");
    std::fs::write(
        &pretty,
        format!(
            "DDRDesk connection ID\n\n  {id}\n\nEnter this 9-digit ID in the iOS app to connect.\n"
        ),
    )?;
    Ok(id)
}

pub fn regen_id() -> Result<String> {
    let dir = data_dir();
    ensure_dir(&dir)?;
    let id = generate_id();
    std::fs::write(dir.join("connection-id"), format!("{id}\n"))?;
    std::fs::write(
        dir.join("CONNECTION-ID.txt"),
        format!(
            "DDRDesk connection ID\n\n  {id}\n\nEnter this 9-digit ID in the iOS app to connect.\n"
        ),
    )?;
    Ok(id)
}

pub fn format_id(id: &str) -> String {
    if id.len() == 9 {
        format!("{} {} {}", &id[0..3], &id[3..6], &id[6..9])
    } else {
        id.to_string()
    }
}
