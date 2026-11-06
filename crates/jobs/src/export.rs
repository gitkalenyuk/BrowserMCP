//! `run.export`: a run's rows to CSV, Excel, JSON, SQLite or a webhook.
//!
//! File exports flatten every row into one table:
//!
//! ```text
//! row | status | <input columns…> | <result keys…> | error_code | error | attempts | duration_ms
//! ```
//!
//! Result keys come from the merged `browser.output({...})` object (a result key that clashes
//! with an input column becomes `result.<key>`; a non-object result goes into one `result`
//! column). Nested values are written as JSON text. JSON and webhook exports keep the full
//! row objects instead (as returned by `run.results`).
//!
//! Files go to `data/runs/<id>/results.<ext>` unless `path` is given (a relative `path` is
//! inside `data/runs/<id>/`). A webhook gets `POST {run, script, batch, batches, rows:[…]}`
//! in batches of 100 rows, each retried 3 times on network errors, 408, 429 and 5xx.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bmcp_core::{CmdError, ErrorCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Inner;
use crate::store::{RowRecord, RowStatus};

/// Export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Xlsx,
    Json,
    Sqlite,
    Webhook,
    /// Not available yet (needs Google OAuth).
    GoogleSheets,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Json => "json",
            ExportFormat::Sqlite => "sqlite",
            ExportFormat::Webhook | ExportFormat::GoogleSheets => "",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    /// Run id from run_start / run_list.
    pub id: String,
    /// csv, xlsx, json, sqlite, webhook (or google_sheets: not implemented yet).
    pub to: ExportFormat,
    /// Output file for csv/xlsx/json/sqlite. Default: data/runs/<id>/results.<ext>; a
    /// relative path is inside data/runs/<id>/.
    pub path: Option<String>,
    /// Webhook URL (http or https) for `to: webhook`.
    pub url: Option<String>,
    /// Only rows with this status (e.g. ok or failed).
    pub status: Option<RowStatus>,
    /// Rows per webhook request (default 100).
    pub batch_size: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportOutput {
    pub to: ExportFormat,
    /// Rows exported.
    pub rows: usize,
    /// The file written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Webhook requests sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batches: Option<usize>,
}

/// The flattened table of a run (see the [module docs](self)).
#[derive(Debug, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

const WEBHOOK_ATTEMPTS: u32 = 3;

impl Inner {
    pub async fn export(&self, args: ExportArgs) -> Result<ExportOutput, CmdError> {
        let run = self.store.run(&args.id)?;
        if args.to == ExportFormat::GoogleSheets {
            return Err(CmdError::new(
                ErrorCode::NotImplemented,
                "Google Sheets export is not implemented yet: it needs a Google OAuth sign-in",
            )
            .with_hint("Export to xlsx or csv and import that file into Google Sheets."));
        }
        let rows = self.all_rows(&run.id, args.status)?;
        if args.to == ExportFormat::Webhook {
            let url = args
                .url
                .as_deref()
                .ok_or_else(|| CmdError::invalid_params("to: webhook needs `url`"))?;
            let batch_size = args.batch_size.unwrap_or(100).max(1);
            let batches = post_batches(
                url,
                &run.id,
                &run.script,
                &rows,
                batch_size,
                self.config.retry_backoff,
            )
            .await?;
            return Ok(ExportOutput {
                to: args.to,
                rows: rows.len(),
                path: None,
                batches: Some(batches),
            });
        }

        let path = self.export_path(&run.id, args.to, args.path.as_deref());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| file_error(dir, e))?;
        }
        match args.to {
            ExportFormat::Csv => write_csv(&path, &table(&rows))?,
            ExportFormat::Xlsx => write_xlsx(&path, &table(&rows))?,
            ExportFormat::Sqlite => write_sqlite(&path, &table(&rows))?,
            ExportFormat::Json => {
                let text = serde_json::to_string_pretty(&rows)
                    .map_err(|e| CmdError::internal(e.to_string()))?;
                std::fs::write(&path, text).map_err(|e| file_error(&path, e))?;
            }
            ExportFormat::Webhook | ExportFormat::GoogleSheets => unreachable!("handled above"),
        }
        Ok(ExportOutput {
            to: args.to,
            rows: rows.len(),
            path: Some(path.display().to_string()),
            batches: None,
        })
    }

    fn all_rows(&self, id: &str, status: Option<RowStatus>) -> Result<Vec<RowRecord>, CmdError> {
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.rows(id, after, 1000, status)?;
            let Some(last) = page.last() else { break };
            after = Some(last.row);
            rows.extend(page);
        }
        Ok(rows)
    }

    fn export_path(&self, id: &str, to: ExportFormat, path: Option<&str>) -> PathBuf {
        let run_dir = self.config.runs_dir.join(id);
        match path {
            Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
            Some(p) => run_dir.join(p),
            None => run_dir.join(format!("results.{}", to.extension())),
        }
    }
}

/// Flatten rows into columns (see the [module docs](self)).
pub fn table(rows: &[RowRecord]) -> Table {
    // Column order = first appearance.
    let mut inputs: Vec<String> = Vec::new();
    let mut results: Vec<String> = Vec::new();
    let mut plain_result = false;
    for row in rows {
        for (key, _) in &row.input_pairs {
            if !inputs.contains(key) {
                inputs.push(key.clone());
            }
        }
        match &row.result {
            Value::Object(map) => {
                for key in map.keys() {
                    if !results.contains(key) {
                        results.push(key.clone());
                    }
                }
            }
            Value::Null => {}
            _ => plain_result = true,
        }
    }
    const FIXED: [&str; 6] = [
        "row",
        "status",
        "error_code",
        "error",
        "attempts",
        "duration_ms",
    ];
    let result_column = |key: &String| {
        if inputs.contains(key) || FIXED.contains(&key.as_str()) || key == "result" {
            format!("result.{key}")
        } else {
            key.clone()
        }
    };

    let mut columns = vec!["row".to_string(), "status".to_string()];
    columns.extend(inputs.iter().cloned());
    columns.extend(results.iter().map(result_column));
    if plain_result {
        columns.push("result".into());
    }
    columns.extend(["error_code", "error", "attempts", "duration_ms"].map(String::from));

    let table_rows = rows
        .iter()
        .map(|row| {
            let mut cells = vec![row.row.to_string(), row.status.as_str().to_string()];
            for key in &inputs {
                let value = row.input_pairs.iter().find(|(k, _)| k == key);
                cells.push(value.map(|(_, v)| v.clone()).unwrap_or_default());
            }
            for key in &results {
                cells.push(row.result.get(key).map(cell).unwrap_or_default());
            }
            if plain_result {
                cells.push(match &row.result {
                    Value::Object(_) => String::new(),
                    other => cell(other),
                });
            }
            let error = row.error.as_ref();
            cells.push(error.map(|e| e.code.clone()).unwrap_or_default());
            cells.push(error.map(|e| e.message.clone()).unwrap_or_default());
            cells.push(row.attempts.to_string());
            cells.push(row.duration_ms.map(|d| d.to_string()).unwrap_or_default());
            cells
        })
        .collect();
    Table {
        columns,
        rows: table_rows,
    }
}

/// JSON value → cell text: strings as-is, null empty, the rest as JSON.
fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn write_csv(path: &Path, table: &Table) -> Result<(), CmdError> {
    let mut writer = csv::Writer::from_path(path).map_err(|e| file_error(path, e))?;
    writer
        .write_record(&table.columns)
        .map_err(|e| file_error(path, e))?;
    for row in &table.rows {
        writer.write_record(row).map_err(|e| file_error(path, e))?;
    }
    writer.flush().map_err(|e| file_error(path, e))
}

fn write_xlsx(path: &Path, table: &Table) -> Result<(), CmdError> {
    let err = |e: rust_xlsxwriter::XlsxError| file_error(path, e);
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    sheet.set_name("results").map_err(err)?;
    let bold = rust_xlsxwriter::Format::new().set_bold();
    for (c, name) in table.columns.iter().enumerate() {
        sheet
            .write_string_with_format(0, c as u16, name, &bold)
            .map_err(err)?;
    }
    // These columns are numbers; write them as numbers so Excel can sort/sum them.
    let numeric = ["row", "attempts", "duration_ms"];
    for (r, row) in table.rows.iter().enumerate() {
        let r = r as u32 + 1;
        for (c, text) in row.iter().enumerate() {
            let number = numeric
                .contains(&table.columns[c].as_str())
                .then(|| text.parse::<f64>().ok())
                .flatten();
            match number {
                Some(n) => sheet.write_number(r, c as u16, n).map(drop),
                None => sheet.write_string(r, c as u16, text).map(drop),
            }
            .map_err(err)?;
        }
    }
    book.save(path).map_err(err)
}

/// A fresh file with one `results` table, every column TEXT.
fn write_sqlite(path: &Path, table: &Table) -> Result<(), CmdError> {
    let err = |e: rusqlite::Error| file_error(path, e);
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| file_error(path, e))?;
    }
    let mut conn = rusqlite::Connection::open(path).map_err(err)?;
    let quoted: Vec<String> = table
        .columns
        .iter()
        .map(|c| format!("\"{}\" TEXT", c.replace('"', "\"\"")))
        .collect();
    conn.execute(&format!("CREATE TABLE results ({})", quoted.join(", ")), [])
        .map_err(err)?;
    let placeholders = vec!["?"; table.columns.len()].join(", ");
    let tx = conn.transaction().map_err(err)?;
    {
        let mut insert = tx
            .prepare(&format!("INSERT INTO results VALUES ({placeholders})"))
            .map_err(err)?;
        for row in &table.rows {
            insert
                .execute(rusqlite::params_from_iter(row.iter()))
                .map_err(err)?;
        }
    }
    tx.commit().map_err(err)
}

/// POST the rows in batches; returns the number of requests.
async fn post_batches(
    url: &str,
    run: &str,
    script: &str,
    rows: &[RowRecord],
    batch_size: usize,
    backoff: Duration,
) -> Result<usize, CmdError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(CmdError::invalid_params(format!(
            "webhook url must start with http:// or https://, got {url:?}"
        )));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CmdError::internal(format!("http client: {e}")))?;
    let chunks: Vec<&[RowRecord]> = if rows.is_empty() {
        vec![&[]] // still tell the webhook that the run had no matching rows
    } else {
        rows.chunks(batch_size).collect()
    };
    let batches = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let body = json!({
            "run": run,
            "script": script,
            "batch": i + 1,
            "batches": batches,
            "rows": chunk,
        });
        let mut attempt = 0;
        loop {
            attempt += 1;
            let failure = match client.post(url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => break,
                Ok(resp) => {
                    let status = resp.status();
                    let retryable = status.is_server_error()
                        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                        || status == reqwest::StatusCode::REQUEST_TIMEOUT;
                    let message = format!("webhook answered {status} for batch {}", i + 1);
                    if !retryable {
                        return Err(CmdError::invalid_params(message)
                            .with_hint("Check the webhook URL and what it accepts."));
                    }
                    message
                }
                Err(e) => format!("webhook request for batch {} failed: {e}", i + 1),
            };
            if attempt >= WEBHOOK_ATTEMPTS {
                return Err(CmdError::new(
                    ErrorCode::Timeout,
                    format!("{failure} (after {attempt} attempts; {i} of {batches} batches sent)"),
                ));
            }
            tokio::time::sleep(backoff.saturating_mul(1 << (attempt - 1))).await;
        }
    }
    Ok(batches)
}

fn file_error(path: &Path, err: impl std::fmt::Display) -> CmdError {
    CmdError::internal(format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ScriptFailure;
    use serde_json::Map;

    fn row(idx: u64, inputs: &[(&str, &str)], result: Value, error: Option<&str>) -> RowRecord {
        let input_pairs: Vec<(String, String)> = inputs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        RowRecord {
            row: idx,
            status: if error.is_some() {
                RowStatus::Failed
            } else {
                RowStatus::Ok
            },
            inputs: Map::new(),
            result,
            error: error.map(|m| ScriptFailure {
                code: "SCRIPT_ERROR".into(),
                command: None,
                message: m.into(),
                line: None,
            }),
            attempts: 1,
            duration_ms: Some(5),
            stdout_tail: String::new(),
            script: None,
            input_pairs,
        }
    }

    #[test]
    fn table_flattens_inputs_and_results() {
        let rows = [
            row(
                0,
                &[("email", "a")],
                json!({"welcome": "hi a", "email": "x", "n": 1}),
                None,
            ),
            row(1, &[("email", "b")], Value::Null, Some("Wrong password")),
            row(
                2,
                &[("email", "c"), ("extra", "e")],
                json!({"tags": [1, 2]}),
                None,
            ),
        ];
        let t = table(&rows);
        assert_eq!(
            t.columns,
            [
                "row",
                "status",
                "email",
                "extra",
                // Result keys: sorted within an object (JSON has no order), then first seen.
                "result.email",
                "n",
                "welcome",
                "tags",
                "error_code",
                "error",
                "attempts",
                "duration_ms"
            ]
        );
        assert_eq!(
            t.rows[0],
            ["0", "ok", "a", "", "x", "1", "hi a", "", "", "", "1", "5"]
        );
        assert_eq!(
            t.rows[1],
            [
                "1",
                "failed",
                "b",
                "",
                "",
                "",
                "",
                "",
                "SCRIPT_ERROR",
                "Wrong password",
                "1",
                "5"
            ]
        );
        assert_eq!(t.rows[2][7], "[1,2]");
    }
}
