//! Where downloads go (pure; used by the CEF download handler, which never shows a dialog).
//!
//! * a tab of a mass run → `data/runs/<run>/downloads/`
//! * any other tab → `data/downloads/`
//!
//! The file name is the server's suggestion, cleaned of characters Windows forbids; an
//! existing file is never overwritten (`report (1).pdf`, `report (2).pdf`, …).

use std::path::{Path, PathBuf};

/// Folder for a download of a tab that belongs to `run` (if any).
pub fn download_dir(data_root: &Path, run: Option<&str>) -> PathBuf {
    match run.map(safe_file_name).filter(|r| r != "download") {
        Some(run) => data_root.join("runs").join(run).join("downloads"),
        None => data_root.join("downloads"),
    }
}

/// A file name that is safe on every OS: no path separators, no `<>:"|?*`, no control
/// characters, no trailing dots/spaces, not a reserved DOS name, at most 150 chars.
/// Empty → `download`.
pub fn safe_file_name(suggested: &str) -> String {
    // Only the last path component counts (a server could send "../../x").
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or(suggested);
    let mut name: String = last
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    name = name.trim().trim_end_matches(['.', ' ']).to_string();
    if name.chars().count() > 150 {
        // Keep the extension.
        let (stem, ext) = split_ext(&name);
        let stem: String = stem.chars().take(140).collect();
        name = format!("{stem}{ext}");
    }
    let stem_upper = split_ext(&name).0.to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3",
    ];
    if RESERVED.contains(&stem_upper.as_str()) {
        name = format!("_{name}");
    }
    if name.is_empty() || name == "." || name == ".." {
        "download".into()
    } else {
        name
    }
}

/// `"a.tar.gz"` → (`"a.tar"`, `".gz"`); no extension → (`name`, `""`). A leading dot
/// (`.bashrc`) is not an extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `dir/name`, or `dir/name (1).ext`, `(2)`, … — the first that `exists` says is free.
pub fn unique_path(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !exists(&first) {
        return first;
    }
    let (stem, ext) = split_ext(name);
    for n in 1..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !exists(&candidate) {
            return candidate;
        }
    }
    dir.join(format!("{stem} ({}){ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders() {
        let root = Path::new("D:/data");
        assert_eq!(download_dir(root, None), root.join("downloads"));
        assert_eq!(
            download_dir(root, Some("run-17")),
            root.join("runs").join("run-17").join("downloads")
        );
        // A run id can never escape the runs folder.
        assert_eq!(
            download_dir(root, Some("../../evil")),
            root.join("runs").join("evil").join("downloads")
        );
        assert_eq!(download_dir(root, Some("")), root.join("downloads"));
    }

    #[test]
    fn names() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        assert_eq!(safe_file_name("name. . "), "name");
        assert_eq!(safe_file_name(""), "download");
        assert_eq!(safe_file_name(".."), "download");
        let long = format!("{}.zip", "x".repeat(300));
        let cut = safe_file_name(&long);
        assert!(cut.ends_with(".zip") && cut.chars().count() == 144);
    }

    #[test]
    fn never_overwrites() {
        let dir = Path::new("/d");
        let taken = [dir.join("a.pdf"), dir.join("a (1).pdf")];
        let exists = |p: &Path| taken.iter().any(|t| t == p);
        assert_eq!(unique_path(dir, "a.pdf", exists), dir.join("a (2).pdf"));
        assert_eq!(unique_path(dir, "b", exists), dir.join("b"));
    }
}
