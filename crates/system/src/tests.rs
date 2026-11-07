//! Path checks and command behaviour with a fake [`Opener`] (no Explorer window is opened).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{Caller, CmdError, Ctx, DataDir, ErrorCode, Registry, Toolset};
use serde_json::{Value, json};

use super::*;

/// Records what would have been shown.
#[derive(Default)]
struct FakeOpener {
    calls: Mutex<Vec<(&'static str, PathBuf)>>,
}

impl Opener for FakeOpener {
    fn reveal(&self, path: &Path) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(("reveal", path.to_path_buf()));
        Ok(())
    }
    fn open_folder(&self, path: &Path) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(("open", path.to_path_buf()));
        Ok(())
    }
}

struct Setup {
    registry: Registry,
    ctx: Ctx,
    opener: Arc<FakeOpener>,
    /// Temp folder holding `data/` (the allowed data folder) and `outside/`.
    tmp: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        for dir in ["data/videos", "data2", "outside"] {
            std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        for file in ["data/videos/a b.mp4", "data2/x.txt", "outside/secret.txt"] {
            std::fs::write(tmp.path().join(file), b"x").unwrap();
        }
        let mut registry = Registry::new();
        register(&mut registry);
        registry
            .extensions()
            .insert(DataDir::from_root(tmp.path().join("data")));
        let opener = Arc::new(FakeOpener::default());
        registry.extensions().insert(OpenerExt(opener.clone()));
        let mut ctx = ctx(Caller::Rest, FakeBackend::new());
        ctx.extensions = registry.extensions().clone();
        Setup {
            registry,
            ctx,
            opener,
            tmp,
        }
    }

    fn path(&self, rel: &str) -> String {
        self.tmp.path().join(rel).display().to_string()
    }

    async fn call(&self, name: &str, path: &str) -> Result<Value, CmdError> {
        self.registry
            .call(self.ctx.clone(), name, json!({ "path": path }))
            .await
    }

    fn calls(&self) -> Vec<(&'static str, PathBuf)> {
        self.opener.calls.lock().unwrap().clone()
    }
}

/// The canonical, plain (`C:\…`) form of a path, as the commands report it.
fn resolved(path: &str) -> PathBuf {
    plain_path(Path::new(path).canonicalize().unwrap())
}

#[tokio::test]
async fn reveals_a_file_in_the_data_folder() {
    let s = Setup::new();
    let file = s.path("data/videos/a b.mp4");
    let out = s.call("system.reveal_path", &file).await.unwrap();
    assert_eq!(out["path"], resolved(&file).display().to_string());
    assert!(!out["path"].as_str().unwrap().starts_with(r"\\?\"));
    assert_eq!(s.calls(), [("reveal", resolved(&file))]);

    // The verbatim form of a drive path (`\\?\C:\…`) is accepted too.
    if cfg!(windows) {
        let verbatim = Path::new(&file).canonicalize().unwrap();
        assert!(verbatim.display().to_string().starts_with(r"\\?\"));
        s.call("system.reveal_path", &verbatim.display().to_string())
            .await
            .unwrap();
        assert_eq!(s.calls().len(), 2);
    }
}

#[tokio::test]
async fn extra_roots_of_the_app_are_allowed_too() {
    let s = Setup::new();
    let outside = s.path("outside");
    assert!(s.call("system.open_path", &outside).await.is_err());
    // E.g. the download folder chosen in Settings.
    let root = PathBuf::from(&outside);
    s.registry
        .extensions()
        .insert(ExtraRoots(Arc::new(move || vec![root.clone()])));
    let mut ctx = s.ctx.clone();
    ctx.extensions = s.registry.extensions().clone();
    let out = s
        .registry
        .call(ctx, "system.open_path", json!({ "path": outside }))
        .await
        .unwrap();
    assert_eq!(out["path"], resolved(&outside).display().to_string());
}

#[tokio::test]
async fn opens_folders_only() {
    let s = Setup::new();
    let dir = s.path("data/videos");
    s.call("system.open_path", &dir).await.unwrap();
    assert_eq!(s.calls(), [("open", resolved(&dir))]);

    // A file would be *run* by Explorer: refused.
    let err = s
        .call("system.open_path", &s.path("data/videos/a b.mp4"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert_eq!(s.calls().len(), 1);
}

#[tokio::test]
async fn refuses_paths_outside_the_allowed_folders() {
    let s = Setup::new();
    for (path, code) in [
        // Another folder.
        (s.path("outside/secret.txt"), ErrorCode::InvalidParams),
        (s.path("outside"), ErrorCode::InvalidParams),
        // `..` out of the data folder (the file exists).
        (
            s.path(r"data\..\outside\secret.txt"),
            ErrorCode::InvalidParams,
        ),
        (
            s.path("data/videos/../../outside"),
            ErrorCode::InvalidParams,
        ),
        // A sibling whose name starts like the data folder.
        (s.path("data2/x.txt"), ErrorCode::InvalidParams),
        // The parent of the data folder.
        (s.path(""), ErrorCode::InvalidParams),
        // Missing, inside and outside.
        (s.path("data/videos/missing.mp4"), ErrorCode::NotFound),
        (s.path("data/../nope/x"), ErrorCode::NotFound),
        // Network and device paths are refused before anything is opened (TEST-NET address).
        (
            r"\\203.0.113.9\share\x.mp4".to_string(),
            ErrorCode::InvalidParams,
        ),
        (
            r"\\?\UNC\203.0.113.9\share\x".to_string(),
            ErrorCode::InvalidParams,
        ),
        (r"\\.\pipe\x".to_string(), ErrorCode::InvalidParams),
        // Relative and empty.
        (r"videos\a b.mp4".to_string(), ErrorCode::InvalidParams),
        (r"..\..\Windows".to_string(), ErrorCode::InvalidParams),
        ("  ".to_string(), ErrorCode::InvalidParams),
    ] {
        for command in ["system.reveal_path", "system.open_path"] {
            let err = s.call(command, &path).await.unwrap_err();
            assert_eq!(err.code, code, "{command} {path}: {}", err.message);
        }
    }
    assert!(s.calls().is_empty(), "nothing may be shown");
}

#[test]
fn check_path_without_roots_allows_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().display().to_string();
    let err = check_path(&path, &[]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    // A missing root is skipped, not an error.
    let err = check_path(&path, &[tmp.path().join("missing")]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert!(check_path(&path, &[tmp.path().to_path_buf()]).is_ok());
}

#[test]
fn plain_path_strips_the_verbatim_prefix() {
    assert_eq!(
        plain_path(PathBuf::from(r"\\?\C:\Users\me\a.mp4")),
        PathBuf::from(r"C:\Users\me\a.mp4")
    );
    assert_eq!(
        plain_path(PathBuf::from(r"\\?\UNC\server\share\a")),
        PathBuf::from(r"\\server\share\a")
    );
    assert_eq!(plain_path(PathBuf::from(r"C:\a")), PathBuf::from(r"C:\a"));
}

#[test]
fn commands_are_in_the_system_toolset() {
    let mut registry = Registry::new();
    register(&mut registry);
    let names: Vec<_> = registry.list(None).into_iter().map(|i| i.name).collect();
    assert_eq!(names.len(), 2);
    for info in registry.list(None) {
        assert_eq!(info.toolset, Toolset::System, "{}", info.name);
    }
}
