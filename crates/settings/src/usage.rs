//! How much space the data folder takes, by area (`browsing_data.usage`).

use std::path::{Path, PathBuf};

use bmcp_core::{CmdError, Command, Ctx, DataDir, Meta, Toolset};
use schemars::JsonSchema;
use serde::Serialize;

/// Bytes and files below `path` (links and junctions are not followed, unreadable entries are
/// skipped: this is an estimate for people, not an audit).
pub fn folder_size(path: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Area {
    /// Folder name inside the data folder (`profiles`, `db`, `downloads`, …); `profiles/<id>`
    /// for one profile; `(files)` for loose files at the top.
    pub id: String,
    pub path: String,
    pub bytes: u64,
    pub files: u64,
}

/// Sizes of the data folder's parts (pure apart from reading the disk). Profiles are listed
/// one by one, the biggest areas first.
pub fn measure(root: &Path) -> (Vec<Area>, u64) {
    let mut areas = Vec::new();
    let mut loose = (0u64, 0u64);
    let entries = std::fs::read_dir(root)
        .map(|e| e.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if kind.is_symlink() {
            continue;
        }
        if !kind.is_dir() {
            loose.0 += entry.metadata().map(|m| m.len()).unwrap_or(0);
            loose.1 += 1;
            continue;
        }
        if name == "profiles" {
            let profiles: Vec<PathBuf> = std::fs::read_dir(entry.path())
                .map(|e| e.flatten().map(|p| p.path()).collect())
                .unwrap_or_default();
            for profile in profiles {
                let id = profile
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let (bytes, files) = if profile.is_dir() {
                    folder_size(&profile)
                } else {
                    (std::fs::metadata(&profile).map(|m| m.len()).unwrap_or(0), 1)
                };
                areas.push(Area {
                    id: format!("profiles/{id}"),
                    path: profile.display().to_string(),
                    bytes,
                    files,
                });
            }
            continue;
        }
        let (bytes, files) = folder_size(&entry.path());
        areas.push(Area {
            id: name,
            path: entry.path().display().to_string(),
            bytes,
            files,
        });
    }
    if loose.1 > 0 {
        areas.push(Area {
            id: "(files)".into(),
            path: root.display().to_string(),
            bytes: loose.0,
            files: loose.1,
        });
    }
    areas.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    let total = areas.iter().map(|a| a.bytes).sum();
    (areas, total)
}

pub struct Usage;

#[derive(Debug, Serialize, JsonSchema)]
pub struct UsageOutput {
    /// The data folder.
    pub root: String,
    pub total_bytes: u64,
    /// Biggest first.
    pub areas: Vec<Area>,
}

impl Command for Usage {
    const NAME: &'static str = "browsing_data.usage";
    type Args = bmcp_core::command::Empty;
    type Output = UsageOutput;

    fn meta() -> Meta {
        Meta::new(
            "How much disk space the BrowserMCP data folder uses, by area (each profile's \
             cache and site data, db, downloads, recordings, logs, …). Returns {root, \
             total_bytes, areas:[{id, path, bytes, files}]}.",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, _args: bmcp_core::command::Empty) -> Result<UsageOutput, CmdError> {
        let data = ctx
            .extensions
            .get::<DataDir>()
            .ok_or_else(|| CmdError::internal("the data folder is not known"))?;
        let root = data.root().to_path_buf();
        // A profile cache can hold tens of thousands of files: walk off the async threads.
        let (areas, total) = tokio::task::spawn_blocking({
            let root = root.clone();
            move || measure(&root)
        })
        .await
        .map_err(|e| CmdError::internal(format!("measuring the data folder failed: {e}")))?;
        Ok(UsageOutput {
            root: root.display().to_string(),
            total_bytes: total,
            areas,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_areas_and_each_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (file, size) in [
            ("profiles/default/Cache/a", 300),
            ("profiles/default/b", 100),
            ("profiles/work/c", 50),
            ("db/browsermcp.sqlite", 200),
            ("logs/cef.log", 10),
            ("README.txt", 5),
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0u8; size]).unwrap();
        }
        std::fs::create_dir_all(root.join("empty")).unwrap();
        let (areas, total) = measure(root);
        let ids: Vec<(&str, u64)> = areas.iter().map(|a| (a.id.as_str(), a.bytes)).collect();
        assert_eq!(
            ids,
            [
                ("profiles/default", 400),
                ("db", 200),
                ("profiles/work", 50),
                ("logs", 10),
                ("(files)", 5),
                ("empty", 0),
            ]
        );
        assert_eq!(total, 665);
        assert_eq!(areas[0].files, 2);
    }
}
