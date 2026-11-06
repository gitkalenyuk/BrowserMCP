//! Bookmarks: a tree of folders and URL bookmarks in the `bookmarks` table.
//!
//! Two root folders always exist: [`BOOKMARKS_BAR`] (id 1, what the new-tab page shows) and
//! [`OTHER_BOOKMARKS`] (id 2). Siblings are ordered by `position` (0, 1, 2, … without gaps).
//!
//! Commands: `data.bookmarks` (UI listing), `bookmarks.add`, `bookmarks.add_folder`,
//! `bookmarks.remove`, `bookmarks.move`, `bookmarks.search`.

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use rusqlite::{Connection, OptionalExtension, Row, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::{
    BOOKMARKS_BAR, DataError, DataResult, DataStore, OTHER_BOOKMARKS, like_pattern, now_ms,
};

/// A URL bookmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Bookmark {
    pub id: i64,
    pub title: String,
    pub url: String,
    /// Id of the folder it is in.
    pub folder: i64,
    /// Index among its siblings (folders and bookmarks share one order).
    pub position: i64,
    /// Unix ms.
    pub created: i64,
}

/// A bookmark folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Folder {
    pub id: i64,
    pub title: String,
    /// Parent folder; `null` for the two root folders.
    pub parent: Option<i64>,
    pub position: i64,
    pub created: i64,
    /// Number of direct children (folders + bookmarks).
    pub children: i64,
}

const BOOKMARK_COLUMNS: &str = "id, title, url, parent, position, created";

fn bookmark_from_row(row: &Row<'_>) -> rusqlite::Result<Bookmark> {
    Ok(Bookmark {
        id: row.get(0)?,
        title: row.get(1)?,
        url: row.get(2)?,
        folder: row.get(3)?,
        position: row.get(4)?,
        created: row.get(5)?,
    })
}

// ---------------------------------------------------------------------------------------------
// Store functions (synchronous; called inside `DataStore::run`).
// ---------------------------------------------------------------------------------------------

/// Fails unless `id` is an existing folder.
fn check_folder(conn: &Connection, id: i64) -> DataResult<()> {
    let kind: Option<String> = conn
        .query_row("SELECT kind FROM bookmarks WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    match kind.as_deref() {
        Some("folder") => Ok(()),
        Some(_) => Err(DataError::Invalid(format!("bookmark {id} is not a folder"))),
        None => Err(DataError::NotFound(format!(
            "bookmark folder {id} not found"
        ))),
    }
}

/// Position after the last child of `folder`.
fn next_position(conn: &Connection, folder: i64) -> DataResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM bookmarks WHERE parent = ?1",
        [folder],
        |r| r.get(0),
    )?)
}

/// Number the children of `folder` 0, 1, 2, … in their current order.
fn renumber(conn: &Connection, folder: i64) -> DataResult<()> {
    let ids = child_ids(conn, folder)?;
    set_order(conn, &ids)
}

fn child_ids(conn: &Connection, folder: i64) -> DataResult<Vec<i64>> {
    let mut stmt =
        conn.prepare("SELECT id FROM bookmarks WHERE parent = ?1 ORDER BY position, id")?;
    let ids = stmt
        .query_map([folder], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(ids)
}

fn set_order(conn: &Connection, ids: &[i64]) -> DataResult<()> {
    let mut stmt = conn.prepare("UPDATE bookmarks SET position = ?1 WHERE id = ?2")?;
    for (position, id) in ids.iter().enumerate() {
        stmt.execute(params![position as i64, id])?;
    }
    Ok(())
}

pub fn get_bookmark(conn: &Connection, id: i64) -> DataResult<Option<Bookmark>> {
    Ok(conn
        .query_row(
            &format!("SELECT {BOOKMARK_COLUMNS} FROM bookmarks WHERE id = ?1 AND kind = 'url'"),
            [id],
            bookmark_from_row,
        )
        .optional()?)
}

pub fn get_folder(conn: &Connection, id: i64) -> DataResult<Option<Folder>> {
    Ok(conn
        .query_row(
            "SELECT f.id, f.title, f.parent, f.position, f.created,
                    (SELECT COUNT(*) FROM bookmarks c WHERE c.parent = f.id)
             FROM bookmarks f WHERE f.id = ?1 AND f.kind = 'folder'",
            [id],
            |r| {
                Ok(Folder {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    parent: r.get(2)?,
                    position: r.get(3)?,
                    created: r.get(4)?,
                    children: r.get(5)?,
                })
            },
        )
        .optional()?)
}

/// The bookmarks and the sub-folders directly inside `folder`, in order.
pub fn list_folder(
    conn: &Connection,
    folder: i64,
    limit: usize,
) -> DataResult<(Vec<Bookmark>, Vec<Folder>)> {
    check_folder(conn, folder)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {BOOKMARK_COLUMNS} FROM bookmarks WHERE parent = ?1 AND kind = 'url'
         ORDER BY position, id LIMIT ?2"
    ))?;
    let bookmarks = stmt
        .query_map(params![folder, limit as i64], bookmark_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let folder_ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM bookmarks WHERE parent = ?1 AND kind = 'folder' ORDER BY position, id",
        )?;
        stmt.query_map([folder], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut folders = Vec::with_capacity(folder_ids.len());
    for id in folder_ids {
        folders.extend(get_folder(conn, id)?);
    }
    Ok((bookmarks, folders))
}

/// Append a URL bookmark to `folder`. An empty title falls back to the page's title from
/// history, then to the URL itself.
pub fn add_bookmark(
    conn: &Connection,
    folder: i64,
    url: &str,
    title: Option<&str>,
    created: i64,
) -> DataResult<Bookmark> {
    let url = url.trim();
    if url.is_empty() {
        return Err(DataError::Invalid("url must not be empty".into()));
    }
    url::Url::parse(url).map_err(|e| DataError::Invalid(format!("invalid url {url:?}: {e}")))?;
    check_folder(conn, folder)?;
    let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => title.to_owned(),
        None => conn
            .query_row(
                "SELECT title FROM urls WHERE url = ?1 AND title <> ''",
                [url],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| url.to_owned()),
    };
    let position = next_position(conn, folder)?;
    conn.execute(
        "INSERT INTO bookmarks (parent, kind, title, url, position, created)
         VALUES (?1, 'url', ?2, ?3, ?4, ?5)",
        params![folder, title, url, position, created],
    )?;
    let id = conn.last_insert_rowid();
    get_bookmark(conn, id)?.ok_or_else(|| DataError::NotFound(format!("bookmark {id}")))
}

/// Append a folder to `parent`.
pub fn add_folder(conn: &Connection, parent: i64, title: &str, created: i64) -> DataResult<Folder> {
    let title = title.trim();
    if title.is_empty() {
        return Err(DataError::Invalid("folder title must not be empty".into()));
    }
    check_folder(conn, parent)?;
    let position = next_position(conn, parent)?;
    conn.execute(
        "INSERT INTO bookmarks (parent, kind, title, position, created)
         VALUES (?1, 'folder', ?2, ?3, ?4)",
        params![parent, title, position, created],
    )?;
    let id = conn.last_insert_rowid();
    get_folder(conn, id)?.ok_or_else(|| DataError::NotFound(format!("folder {id}")))
}

/// The folder titled `title` inside `parent`, created if missing (used by the import).
pub fn find_or_create_folder(conn: &Connection, parent: i64, title: &str) -> DataResult<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM bookmarks WHERE parent = ?1 AND kind = 'folder' AND title = ?2
             ORDER BY position LIMIT 1",
            params![parent, title],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => Ok(id),
        None => Ok(add_folder(conn, parent, title, now_ms())?.id),
    }
}

/// True when `folder` already directly contains a bookmark for `url` (used by the import).
pub fn folder_has_url(conn: &Connection, folder: i64, url: &str) -> DataResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM bookmarks WHERE parent = ?1 AND kind = 'url' AND url = ?2",
            params![folder, url],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Remove a bookmark or a folder (with everything inside). Returns how many rows went away.
pub fn remove(conn: &mut Connection, id: i64) -> DataResult<usize> {
    if id == BOOKMARKS_BAR || id == OTHER_BOOKMARKS {
        return Err(DataError::Invalid(format!(
            "folder {id} is a root folder and cannot be removed"
        )));
    }
    let tx = conn.transaction()?;
    let parent: Option<i64> = tx
        .query_row("SELECT parent FROM bookmarks WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    let Some(parent) = parent else {
        return Err(DataError::NotFound(format!("bookmark {id} not found")));
    };
    let total_before: i64 = tx.query_row("SELECT COUNT(*) FROM bookmarks", [], |r| r.get(0))?;
    // `ON DELETE CASCADE` removes the folder's contents too.
    tx.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
    let total_after: i64 = tx.query_row("SELECT COUNT(*) FROM bookmarks", [], |r| r.get(0))?;
    renumber(&tx, parent)?;
    tx.commit()?;
    Ok((total_before - total_after) as usize)
}

/// Move `id` into `folder` (default: where it is) at `position` (default: the end).
/// Returns `(folder, position)` after the move.
pub fn move_node(
    conn: &mut Connection,
    id: i64,
    folder: Option<i64>,
    position: Option<usize>,
) -> DataResult<(i64, i64)> {
    if id == BOOKMARKS_BAR || id == OTHER_BOOKMARKS {
        return Err(DataError::Invalid(format!(
            "folder {id} is a root folder and cannot be moved"
        )));
    }
    let tx = conn.transaction()?;
    let old_parent: i64 = tx
        .query_row("SELECT parent FROM bookmarks WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| DataError::NotFound(format!("bookmark {id} not found")))?;
    let new_parent = folder.unwrap_or(old_parent);
    check_folder(&tx, new_parent)?;

    // A folder cannot go into itself or one of its own sub-folders.
    let mut cursor = Some(new_parent);
    while let Some(current) = cursor {
        if current == id {
            return Err(DataError::Invalid(format!(
                "cannot move folder {id} into itself or one of its sub-folders"
            )));
        }
        cursor = tx
            .query_row(
                "SELECT parent FROM bookmarks WHERE id = ?1",
                [current],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
    }

    let mut siblings: Vec<i64> = child_ids(&tx, new_parent)?
        .into_iter()
        .filter(|&other| other != id)
        .collect();
    let index = position.unwrap_or(siblings.len()).min(siblings.len());
    siblings.insert(index, id);
    tx.execute(
        "UPDATE bookmarks SET parent = ?1 WHERE id = ?2",
        params![new_parent, id],
    )?;
    set_order(&tx, &siblings)?;
    if old_parent != new_parent {
        renumber(&tx, old_parent)?;
    }
    tx.commit()?;
    Ok((new_parent, index as i64))
}

/// URL bookmarks whose title or URL contains `q` (case-insensitive for ASCII).
pub fn search(conn: &Connection, q: &str, limit: usize) -> DataResult<Vec<Bookmark>> {
    let pattern = like_pattern(q.trim());
    let mut stmt = conn.prepare(&format!(
        "SELECT {BOOKMARK_COLUMNS} FROM bookmarks
         WHERE kind = 'url' AND (title LIKE ?1 ESCAPE '\\' OR url LIKE ?1 ESCAPE '\\')
         ORDER BY created DESC, id DESC LIMIT ?2"
    ))?;
    let found = stmt
        .query_map(params![pattern, limit as i64], bookmark_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(found)
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

/// Default and maximum number of rows a listing returns.
const DEFAULT_LIMIT: usize = 500;
const MAX_LIMIT: usize = 5000;

fn clamp_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).clamp(1, MAX_LIMIT)
}

/// `data.bookmarks`: the bookmarks in one folder (default: the bookmarks bar).
pub struct List;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Folder id (default 1 = bookmarks bar; 2 = other bookmarks).
    pub folder: Option<i64>,
    /// Max bookmarks to return (default 500).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// The listed folder's id.
    pub folder: i64,
    /// URL bookmarks directly in the folder, in order (`{title, url}` is what the new-tab page
    /// renders).
    pub bookmarks: Vec<Bookmark>,
    /// Sub-folders directly in the folder, in order.
    pub folders: Vec<Folder>,
}

impl Command for List {
    const NAME: &'static str = "data.bookmarks";
    type Args = ListArgs;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List the bookmarks and sub-folders of a bookmark folder (default: the bookmarks \
             bar). Returns {folder, bookmarks:[{id,title,url,…}], folders:[{id,title,…}]}.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: ListArgs) -> Result<ListOutput, CmdError> {
        let folder = args.folder.unwrap_or(BOOKMARKS_BAR);
        let limit = clamp_limit(args.limit, DEFAULT_LIMIT);
        let (bookmarks, folders) = DataStore::from_ctx(&ctx)?
            .run(move |c| list_folder(c, folder, limit))
            .await?;
        Ok(ListOutput {
            folder,
            bookmarks,
            folders,
        })
    }
}

/// `bookmarks.add`.
pub struct Add;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// Absolute URL to bookmark.
    pub url: String,
    /// Title (default: the page title from history, else the URL).
    pub title: Option<String>,
    /// Folder id (default 1 = bookmarks bar).
    pub folder: Option<i64>,
}

impl Command for Add {
    const NAME: &'static str = "bookmarks.add";
    type Args = AddArgs;
    type Output = Bookmark;

    fn meta() -> Meta {
        Meta::new(
            "Bookmark a URL at the end of a folder (default: the bookmarks bar). Returns the \
             new bookmark.",
            Toolset::Data,
        )
        .act()
        .palette("Add bookmark…")
    }

    async fn run(ctx: Ctx, args: AddArgs) -> Result<Bookmark, CmdError> {
        let folder = args.folder.unwrap_or(BOOKMARKS_BAR);
        Ok(DataStore::from_ctx(&ctx)?
            .run(move |c| add_bookmark(c, folder, &args.url, args.title.as_deref(), now_ms()))
            .await?)
    }
}

/// `bookmarks.add_folder`.
pub struct AddFolder;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddFolderArgs {
    pub title: String,
    /// Parent folder id (default 1 = bookmarks bar).
    pub parent: Option<i64>,
}

impl Command for AddFolder {
    const NAME: &'static str = "bookmarks.add_folder";
    type Args = AddFolderArgs;
    type Output = Folder;

    fn meta() -> Meta {
        Meta::new(
            "Create a bookmark folder at the end of a parent folder (default: the bookmarks \
             bar). Returns the new folder.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: AddFolderArgs) -> Result<Folder, CmdError> {
        let parent = args.parent.unwrap_or(BOOKMARKS_BAR);
        Ok(DataStore::from_ctx(&ctx)?
            .run(move |c| add_folder(c, parent, &args.title, now_ms()))
            .await?)
    }
}

/// `bookmarks.remove`.
pub struct Remove;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    /// Bookmark or folder id.
    pub id: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RemoveOutput {
    /// Rows removed: 1 for a bookmark, 1 + contents for a folder.
    pub removed: usize,
}

impl Command for Remove {
    const NAME: &'static str = "bookmarks.remove";
    type Args = IdArgs;
    type Output = RemoveOutput;

    fn meta() -> Meta {
        Meta::new(
            "Remove a bookmark, or a folder with everything inside it. Returns {removed}.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: IdArgs) -> Result<RemoveOutput, CmdError> {
        let removed = DataStore::from_ctx(&ctx)?
            .run(move |c| remove(c, args.id))
            .await?;
        Ok(RemoveOutput { removed })
    }
}

/// `bookmarks.move`.
pub struct Move;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    /// Bookmark or folder id.
    pub id: i64,
    /// Target folder id (default: keep the current folder).
    pub folder: Option<i64>,
    /// Index among the target folder's children (default: the end).
    pub position: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MoveOutput {
    pub id: i64,
    pub folder: i64,
    pub position: i64,
}

impl Command for Move {
    const NAME: &'static str = "bookmarks.move";
    type Args = MoveArgs;
    type Output = MoveOutput;

    fn meta() -> Meta {
        Meta::new(
            "Move a bookmark or folder to another folder and/or position. Returns its new \
             {folder, position}.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: MoveArgs) -> Result<MoveOutput, CmdError> {
        let (folder, position) = DataStore::from_ctx(&ctx)?
            .run(move |c| move_node(c, args.id, args.folder, args.position))
            .await?;
        Ok(MoveOutput {
            id: args.id,
            folder,
            position,
        })
    }
}

/// `bookmarks.search`.
pub struct Search;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Text to find in titles and URLs.
    pub q: String,
    /// Max results (default 100).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    /// Matches in every folder, newest first.
    pub bookmarks: Vec<Bookmark>,
}

impl Command for Search {
    const NAME: &'static str = "bookmarks.search";
    type Args = SearchArgs;
    type Output = SearchOutput;

    fn meta() -> Meta {
        Meta::new(
            "Find bookmarks (in all folders) whose title or URL contains the text. Returns \
             {bookmarks}, newest first.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: SearchArgs) -> Result<SearchOutput, CmdError> {
        let limit = clamp_limit(args.limit, 100);
        let bookmarks = DataStore::from_ctx(&ctx)?
            .run(move |c| search(c, &args.q, limit))
            .await?;
        Ok(SearchOutput { bookmarks })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tests::{call, call_err, setup};
    use bmcp_core::ErrorCode;

    #[tokio::test]
    async fn add_list_search_remove() {
        let (registry, ctx) = setup();
        let a = call(
            &registry,
            &ctx,
            "bookmarks.add",
            json!({"url": "https://a.test/", "title": "Alpha"}),
        )
        .await;
        assert_eq!(a["folder"], 1);
        assert_eq!(a["position"], 0);
        // No title and no history → the URL is the title.
        let b = call(
            &registry,
            &ctx,
            "bookmarks_add",
            json!({"url": "https://b.test/x"}),
        )
        .await;
        assert_eq!(b["title"], "https://b.test/x");
        assert_eq!(b["position"], 1);

        let folder = call(
            &registry,
            &ctx,
            "bookmarks.add_folder",
            json!({"title": "Work"}),
        )
        .await;
        let fid = folder["id"].as_i64().unwrap();
        call(
            &registry,
            &ctx,
            "bookmarks.add",
            json!({"url": "https://c.test/", "title": "Gamma", "folder": fid}),
        )
        .await;

        let bar = call(&registry, &ctx, "data.bookmarks", json!({})).await;
        assert_eq!(bar["folder"], 1);
        let titles: Vec<_> = bar["bookmarks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["title"].clone())
            .collect();
        assert_eq!(titles, [json!("Alpha"), json!("https://b.test/x")]);
        assert_eq!(bar["folders"][0]["title"], "Work");
        assert_eq!(bar["folders"][0]["children"], 1);

        let inside = call(&registry, &ctx, "data.bookmarks", json!({"folder": fid})).await;
        assert_eq!(inside["bookmarks"][0]["url"], "https://c.test/");

        let found = call(&registry, &ctx, "bookmarks.search", json!({"q": "GAMMA"})).await;
        assert_eq!(found["bookmarks"].as_array().unwrap().len(), 1);
        // `%` is literal, not a wildcard.
        let found = call(&registry, &ctx, "bookmarks.search", json!({"q": "%"})).await;
        assert!(found["bookmarks"].as_array().unwrap().is_empty());

        // Removing the folder removes its bookmark too.
        let out = call(&registry, &ctx, "bookmarks.remove", json!({"id": fid})).await;
        assert_eq!(out["removed"], 2);
        let found = call(&registry, &ctx, "bookmarks.search", json!({"q": "c.test"})).await;
        assert!(found["bookmarks"].as_array().unwrap().is_empty());

        let err = call_err(&registry, &ctx, "bookmarks.remove", json!({"id": 1})).await;
        assert_eq!(err.code, ErrorCode::InvalidParams);
        let err = call_err(&registry, &ctx, "bookmarks.remove", json!({"id": 999})).await;
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = call_err(
            &registry,
            &ctx,
            "bookmarks.add",
            json!({"url": "not a url"}),
        )
        .await;
        assert_eq!(err.code, ErrorCode::InvalidParams);
        let err = call_err(&registry, &ctx, "data.bookmarks", json!({"folder": 999})).await;
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn move_reorders_and_reparents() {
        let (registry, ctx) = setup();
        let mut ids = Vec::new();
        for n in 0..3 {
            let b = call(
                &registry,
                &ctx,
                "bookmarks.add",
                json!({"url": format!("https://{n}.test/")}),
            )
            .await;
            ids.push(b["id"].as_i64().unwrap());
        }
        // Move the last one to the front.
        let out = call(
            &registry,
            &ctx,
            "bookmarks.move",
            json!({"id": ids[2], "position": 0}),
        )
        .await;
        assert_eq!(out, json!({"id": ids[2], "folder": 1, "position": 0}));
        let bar = call(&registry, &ctx, "data.bookmarks", json!({})).await;
        let order: Vec<i64> = bar["bookmarks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_i64().unwrap())
            .collect();
        assert_eq!(order, [ids[2], ids[0], ids[1]]);
        let positions: Vec<i64> = bar["bookmarks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, [0, 1, 2]);

        // To "Other bookmarks": the bar is renumbered without gaps.
        call(
            &registry,
            &ctx,
            "bookmarks.move",
            json!({"id": ids[0], "folder": 2}),
        )
        .await;
        let bar = call(&registry, &ctx, "data.bookmarks", json!({})).await;
        let positions: Vec<i64> = bar["bookmarks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, [0, 1]);
        let other = call(&registry, &ctx, "data.bookmarks", json!({"folder": 2})).await;
        assert_eq!(other["bookmarks"][0]["id"], ids[0]);

        // A folder cannot move into its own sub-folder.
        let outer = call(
            &registry,
            &ctx,
            "bookmarks.add_folder",
            json!({"title": "Outer"}),
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let inner = call(
            &registry,
            &ctx,
            "bookmarks.add_folder",
            json!({"title": "Inner", "parent": outer}),
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let err = call_err(
            &registry,
            &ctx,
            "bookmarks.move",
            json!({"id": outer, "folder": inner}),
        )
        .await;
        assert_eq!(err.code, ErrorCode::InvalidParams);
        let err = call_err(
            &registry,
            &ctx,
            "bookmarks.move",
            json!({"id": ids[1], "folder": ids[2]}),
        )
        .await;
        assert_eq!(
            err.code,
            ErrorCode::InvalidParams,
            "a bookmark is not a folder"
        );
        let err = call_err(
            &registry,
            &ctx,
            "bookmarks.move",
            json!({"id": 2, "folder": 1}),
        )
        .await;
        assert_eq!(err.code, ErrorCode::InvalidParams, "root folders stay put");
    }

    #[tokio::test]
    async fn add_takes_title_from_history() {
        let (registry, ctx) = setup();
        let store = crate::DataStore::from_ctx(&ctx).unwrap();
        store
            .with(|c| {
                crate::history::record_visit(c, "https://h.test/", "From history", "link", 1_000)
            })
            .unwrap();
        let b = call(
            &registry,
            &ctx,
            "bookmarks.add",
            json!({"url": "https://h.test/"}),
        )
        .await;
        assert_eq!(b["title"], "From history");
    }
}
