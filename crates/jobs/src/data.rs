//! Input rows for mass runs: CSV, JSON (array of objects), Excel (first sheet) or inline rows.
//!
//! Every row becomes a list of `(column, value)` string pairs, passed to the script host as
//! `--input column=value` (scripts see them as `inputs.column`, always strings). The first
//! row of CSV/Excel is the header. Empty header cells become `col1`, `col2`, …; fully empty
//! rows are skipped.

use std::path::Path;

use bmcp_core::{CmdError, ErrorCode};
use calamine::{Data, Reader};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

/// One input row: `(column, value)` in column order.
pub type Row = Vec<(String, String)>;

/// Where `run.start` takes its rows from. Give exactly one field.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DataArgs {
    /// CSV file with a header row.
    pub csv_path: Option<String>,
    /// JSON file: an array of objects (keys = columns).
    pub json_path: Option<String>,
    /// Excel file (.xlsx/.xls/.ods): first sheet, header row first.
    pub xlsx_path: Option<String>,
    /// Inline rows: an array of objects.
    pub rows: Option<Vec<Map<String, Value>>>,
}

/// Load the rows described by `args`.
pub fn load_rows(args: &DataArgs) -> Result<Vec<Row>, CmdError> {
    let given = [
        args.csv_path.is_some(),
        args.json_path.is_some(),
        args.xlsx_path.is_some(),
        args.rows.is_some(),
    ];
    if given.iter().filter(|g| **g).count() != 1 {
        return Err(CmdError::invalid_params(
            "data: give exactly one of csv_path, json_path, xlsx_path or rows",
        ));
    }
    let rows = if let Some(path) = &args.csv_path {
        read_csv(Path::new(path))?
    } else if let Some(path) = &args.json_path {
        read_json(Path::new(path))?
    } else if let Some(path) = &args.xlsx_path {
        read_xlsx(Path::new(path))?
    } else {
        objects_to_rows(args.rows.clone().unwrap_or_default())?
    };
    if rows.is_empty() {
        return Err(CmdError::invalid_params("data has no rows"));
    }
    Ok(rows)
}

pub fn read_csv(path: &Path) -> Result<Vec<Row>, CmdError> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true) // short rows: missing cells become ""
        .from_path(path)
        .map_err(|e| file_error(path, e))?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| file_error(path, e))?
        .iter()
        .map(str::to_string)
        .collect();
    let headers = clean_headers(headers);
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| file_error(path, e))?;
        let cells: Vec<String> = record.iter().map(str::to_string).collect();
        if let Some(row) = make_row(&headers, cells) {
            rows.push(row);
        }
    }
    Ok(rows)
}

pub fn read_json(path: &Path) -> Result<Vec<Row>, CmdError> {
    let text = std::fs::read_to_string(path).map_err(|e| file_error(path, e))?;
    let objects: Vec<Map<String, Value>> = serde_json::from_str(&text).map_err(|e| {
        CmdError::invalid_params(format!(
            "{}: expected a JSON array of objects: {e}",
            path.display()
        ))
    })?;
    objects_to_rows(objects)
}

pub fn read_xlsx(path: &Path) -> Result<Vec<Row>, CmdError> {
    let mut workbook = calamine::open_workbook_auto(path).map_err(|e| file_error(path, e))?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| CmdError::invalid_params(format!("{}: no sheets", path.display())))?
        .map_err(|e| file_error(path, e))?;
    let mut sheet_rows = range.rows();
    let Some(header) = sheet_rows.next() else {
        return Ok(Vec::new());
    };
    let headers = clean_headers(header.iter().map(cell_text).collect());
    Ok(sheet_rows
        .filter_map(|cells| make_row(&headers, cells.iter().map(cell_text).collect()))
        .collect())
}

fn objects_to_rows(objects: Vec<Map<String, Value>>) -> Result<Vec<Row>, CmdError> {
    objects.into_iter().map(object_to_row).collect()
}

/// One JSON object → one row (also used for `script.run` inputs).
pub fn object_to_row(object: Map<String, Value>) -> Result<Row, CmdError> {
    object
        .into_iter()
        .map(|(key, value)| {
            check_column(&key)?;
            Ok((key, value_text(value)))
        })
        .collect()
}

/// JSON value → the string the script sees.
fn value_text(value: Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s,
        // Numbers, booleans, nested objects/arrays: their JSON text.
        other => other.to_string(),
    }
}

/// Excel cell → text. Whole numbers print without `.0` (Excel stores every number as float).
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        other => other.to_string(),
    }
}

/// Trim, strip a UTF-8 BOM, name empty headers `colN` and make duplicates unique (`x`, `x_2`).
fn clean_headers(raw: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.into_iter()
        .enumerate()
        .map(|(i, h)| {
            let h = h.trim_start_matches('\u{feff}').trim().replace('=', "_");
            let base = if h.is_empty() {
                format!("col{}", i + 1)
            } else {
                h
            };
            let mut name = base.clone();
            let mut n = 2;
            while !seen.insert(name.clone()) {
                name = format!("{base}_{n}");
                n += 1;
            }
            name
        })
        .collect()
}

/// Pair cells with headers; `None` for a row whose cells are all empty.
fn make_row(headers: &[String], mut cells: Vec<String>) -> Option<Row> {
    if cells.iter().all(|c| c.trim().is_empty()) {
        return None;
    }
    cells.resize(headers.len().max(cells.len()), String::new());
    Some(
        cells
            .into_iter()
            .enumerate()
            .map(|(i, cell)| {
                let column = headers
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("col{}", i + 1));
                (column, cell)
            })
            .collect(),
    )
}

/// `--input key=value` splits at the first `=`, so a key cannot contain one.
fn check_column(key: &str) -> Result<(), CmdError> {
    if key.is_empty() || key.contains('=') {
        Err(CmdError::invalid_params(format!(
            "invalid column name {key:?}: must be non-empty and without '='"
        )))
    } else {
        Ok(())
    }
}

fn file_error(path: &Path, err: impl std::fmt::Display) -> CmdError {
    CmdError::new(ErrorCode::NotFound, format!("{}: {err}", path.display()))
        .with_hint("Check that the data file exists and has a header row.")
        .with_retry(bmcp_core::Retry::No)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pairs(row: &Row) -> Vec<(&str, &str)> {
        row.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }

    #[test]
    fn csv_with_bom_quotes_short_rows_and_blank_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("in.csv");
        std::fs::write(
            &path,
            "\u{feff}email,password,note\na@x.test,hunter2,\"hi, there\"\n\nb@x.test,bad\n",
        )
        .unwrap();
        let rows = read_csv(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            pairs(&rows[0]),
            [
                ("email", "a@x.test"),
                ("password", "hunter2"),
                ("note", "hi, there")
            ]
        );
        assert_eq!(
            pairs(&rows[1]),
            [("email", "b@x.test"), ("password", "bad"), ("note", "")]
        );
    }

    #[test]
    fn json_array_of_objects() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("in.json");
        std::fs::write(&path, r#"[{"q":"rust","n":3,"ok":true,"x":null}]"#).unwrap();
        let rows = read_json(&path).unwrap();
        assert_eq!(
            pairs(&rows[0]),
            // JSON objects have no column order: keys come sorted.
            [("n", "3"), ("ok", "true"), ("q", "rust"), ("x", "")]
        );
        std::fs::write(&path, r#"{"not":"an array"}"#).unwrap();
        assert_eq!(read_json(&path).unwrap_err().code, ErrorCode::InvalidParams);
    }

    #[test]
    fn xlsx_first_sheet_with_header() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("in.xlsx");
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_string(0, 0, "email").unwrap();
        sheet.write_string(0, 1, "age").unwrap();
        sheet.write_string(0, 2, "").unwrap();
        sheet.write_string(1, 0, "a@x.test").unwrap();
        sheet.write_number(1, 1, 42.0).unwrap();
        sheet.write_number(1, 2, 1.5).unwrap();
        sheet.write_string(3, 0, "c@x.test").unwrap(); // row 2 left empty
        book.save(&path).unwrap();

        let rows = read_xlsx(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            pairs(&rows[0]),
            [("email", "a@x.test"), ("age", "42"), ("col3", "1.5")]
        );
        assert_eq!(pairs(&rows[1])[0], ("email", "c@x.test"));
    }

    #[test]
    fn exactly_one_source_and_inline_rows() {
        let none = DataArgs::default();
        assert!(load_rows(&none).is_err());
        let inline = DataArgs {
            rows: Some(vec![json!({"a": "1"}).as_object().unwrap().clone()]),
            ..Default::default()
        };
        assert_eq!(pairs(&load_rows(&inline).unwrap()[0]), [("a", "1")]);
        let bad = DataArgs {
            rows: Some(vec![json!({"a=b": "1"}).as_object().unwrap().clone()]),
            ..Default::default()
        };
        assert!(load_rows(&bad).is_err());
        let empty = DataArgs {
            rows: Some(vec![]),
            ..Default::default()
        };
        assert!(load_rows(&empty).is_err());
    }

    #[test]
    fn headers_are_cleaned() {
        let h = clean_headers(vec![" a ".into(), "".into(), "a".into(), "x=y".into()]);
        assert_eq!(h, ["a", "col2", "a_2", "x_y"]);
    }
}
