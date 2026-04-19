use crate::shell::Shell;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const FIREFOX_COMPATIBLE_DIRS: &[&str] = &[
    ".mozilla/firefox",
    "snap/firefox/common/.mozilla/firefox",
    ".var/app/org.mozilla.firefox/.mozilla/firefox",
    ".zen",
    ".config/zen",
];

pub(crate) fn update_atcoder_cookie_best_effort(cookies_path: &Path, shell: &mut Shell) {
    let browser = env::var("ACCC_BROWSER").unwrap_or_else(|_| "firefox-compatible".into());

    if !matches!(browser.as_str(), "firefox" | "zen" | "firefox-compatible") {
        let _ = shell.warn(
            "cookie update skipped: only firefox-compatible browsers are supported".to_string(),
        );
        return;
    }

    match update_from_firefox(cookies_path) {
        Ok(()) => {}
        Err(e) => {
            let _ = shell.warn(format!("cookie update skipped: {e}"));
        }
    }
}

fn update_from_firefox(cookies_path: &Path) -> anyhow::Result<()> {
    let db = newest_cookie_db()
        .ok_or_else(|| anyhow::anyhow!("no firefox-compatible cookies.sqlite found"))?;
    let tempdir = tempfile::tempdir()?;
    let tmp_db = tempdir.path().join("cookies.sqlite");
    fs::copy(&db, &tmp_db)?;
    let wal = db.with_file_name(format!("{}-wal", db.file_name().unwrap().to_string_lossy()));
    let shm = db.with_file_name(format!("{}-shm", db.file_name().unwrap().to_string_lossy()));
    if wal.exists() {
        let _ = fs::copy(&wal, tempdir.path().join("cookies.sqlite-wal"));
    }
    if shm.exists() {
        let _ = fs::copy(&shm, tempdir.path().join("cookies.sqlite-shm"));
    }

    let conn = Connection::open(&tmp_db)?;
    let row: Option<(String, String, String, String, Option<i64>)> = conn
        .query_row(
            "SELECT host, name, value, path, expiry FROM moz_cookies \
             WHERE host LIKE '%atcoder.jp%' AND name='REVEL_SESSION' \
             ORDER BY lastAccessed DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;

    let (host, name, value, path, expiry) = row
        .ok_or_else(|| anyhow::anyhow!("REVEL_SESSION not found in firefox-compatible cookies"))?;

    let expires = expiry.and_then(|e| {
        let secs = if e > 1_000_000_000_000 { e / 1000 } else { e };
        DateTime::<Utc>::from_timestamp(secs, 0)
    });

    let mut line = serde_json::json!({
        "raw_cookie": format!("{name}={value}; HttpOnly; Secure"),
        "path": [path, true],
        "domain": {"HostOnly": host},
    });
    if let Some(dt) = expires {
        line["expires"] = serde_json::json!({
            "AtUtc": dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        });
    }

    if let Some(parent) = cookies_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(cookies_path, format!("{}\n", line))?;
    Ok(())
}

fn newest_cookie_db() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for root in FIREFOX_COMPATIBLE_DIRS {
        let root = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(root);
        let _ = collect_cookie_dbs(&root, &mut candidates);
    }
    candidates
        .into_iter()
        .filter_map(|path| {
            let modified = path.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, path))
        })
        .max_by_key(|(mtime, _)| *mtime)
        .map(|(_, path)| path)
}

fn collect_cookie_dbs(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = match entry {
            Ok(v) => v,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_dir() {
            let _ = collect_cookie_dbs(&path, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("cookies.sqlite") {
            out.push(path);
        }
    }
    Ok(())
}
