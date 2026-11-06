//! License notices for the portable package.
//!
//! * `cef_license`: CEF's BSD license (from the distribution if present, else the known text).
//! * `third_party`: every Rust crate compiled into the shipped executables (normal dependencies
//!   reachable from the given root packages in `cargo metadata --filter-platform …`; build and
//!   dev dependencies are excluded), the npm packages bundled into the UI (`pnpm licenses list
//!   --prod --json` in `ui/`, best effort), then each distinct license text once together with
//!   the packages that use it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::Result;

/// CEF's license (BSD 3-clause). Used when the distribution folder has no LICENSE.txt
/// (cef-rs flattens the archive and may drop it).
const CEF_LICENSE_FALLBACK: &str = "\
Copyright (c) 2008-2020 Marshall A. Greenblatt. Portions Copyright (c)
2006-2009 Google Inc. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the name Chromium Embedded
Framework nor the names of its contributors may be used to endorse
or promote products derived from this software without specific prior
written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
\"AS IS\" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
";

/// CEF's license text: `LICENSE.txt` from the distribution, or the known BSD text.
pub fn cef_license(cef_dir: &Path) -> Result<String> {
    for name in ["LICENSE.txt", "LICENSE"] {
        if let Ok(text) = std::fs::read_to_string(cef_dir.join(name)) {
            return Ok(text);
        }
    }
    Ok(CEF_LICENSE_FALLBACK.to_owned())
}

/// One shipped third-party package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Pkg {
    ecosystem: &'static str,
    name: String,
    version: String,
    license: String,
    repository: String,
}

/// Build THIRD_PARTY.txt.
pub fn third_party(
    root: &Path,
    meta: &Value,
    roots: &[&str],
    version: &str,
    cef_version: &str,
) -> Result<String> {
    let crates = shipped_crates(meta, roots)?;

    // License texts: text -> packages that ship it.
    let mut texts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pkgs = Vec::new();
    for (pkg, dir) in &crates {
        for text in license_texts(dir) {
            texts
                .entry(text)
                .or_default()
                .insert(format!("{} {}", pkg.name, pkg.version));
        }
        pkgs.push(pkg.clone());
    }
    let npm = npm_packages(&root.join("ui"));
    let npm_note = match &npm {
        Ok(list) => {
            pkgs.extend(list.iter().cloned());
            String::new()
        }
        Err(err) => format!("\n(npm package list unavailable: {err})\n"),
    };
    pkgs.sort();

    let mut out = String::new();
    out.push_str(&format!(
        "BrowserMCP {version} — third-party software\n\
         ==========================================\n\n\
         Bundled components\n\
         ------------------\n\
         * Chromium Embedded Framework {cef_version} (BSD-3-Clause): LICENSES/CEF-LICENSE.txt\n\
         * Chromium and its dependencies (V8, ICU, Abseil, …; also the V8 inside\n\
           bmcp-script-host): LICENSES/Chromium-CREDITS.html\n\
         * Mozilla Readability (Apache-2.0): LICENSES/Readability-LICENSE.md\n\n"
    ));
    out.push_str("Packages\n--------\n");
    for p in &pkgs {
        let license = if p.license.is_empty() {
            "see text below"
        } else {
            &p.license
        };
        out.push_str(&format!(
            "{:<5} {} {} — {license}",
            p.ecosystem, p.name, p.version
        ));
        if !p.repository.is_empty() {
            out.push_str(&format!(" — {}", p.repository));
        }
        out.push('\n');
    }
    out.push_str(&npm_note);
    out.push_str(&format!(
        "\nLicense texts ({} distinct)\n-------------\n",
        texts.len()
    ));
    for (text, users) in &texts {
        out.push_str("\n================================================================\n");
        out.push_str("Used by: ");
        out.push_str(&users.iter().cloned().collect::<Vec<_>>().join(", "));
        out.push_str("\n----------------------------------------------------------------\n");
        out.push_str(text.trim());
        out.push('\n');
    }
    Ok(out)
}

/// Third-party crates reachable through normal dependencies from `roots`, with their source
/// folder (where LICENSE files live). Workspace members (no `source`) are excluded.
fn shipped_crates(meta: &Value, roots: &[&str]) -> Result<Vec<(Pkg, std::path::PathBuf)>> {
    let packages: HashMap<&str, &Value> = meta["packages"]
        .as_array()
        .ok_or("cargo metadata: no packages")?
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let nodes: HashMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .ok_or("cargo metadata: no resolve graph")?
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();

    let mut stack: Vec<&str> = packages
        .iter()
        .filter(|(_, p)| p["source"].is_null() && roots.iter().any(|r| p["name"] == *r))
        .map(|(id, _)| *id)
        .collect();
    if stack.is_empty() {
        return Err(format!("cargo metadata: none of {roots:?} found"));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            // Only normal dependencies end up in the executable (kind null).
            let normal = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null());
            if normal && let Some(pkg) = dep["pkg"].as_str() {
                stack.push(pkg);
            }
        }
    }

    let mut out = Vec::new();
    for id in seen {
        let Some(p) = packages.get(id) else { continue };
        if p["source"].is_null() {
            continue; // our own workspace crates
        }
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or_default())
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        out.push((
            Pkg {
                ecosystem: "crate",
                name: p["name"].as_str().unwrap_or_default().to_owned(),
                version: p["version"].as_str().unwrap_or_default().to_owned(),
                license: p["license"].as_str().unwrap_or_default().to_owned(),
                repository: p["repository"].as_str().unwrap_or_default().to_owned(),
            },
            dir,
        ));
    }
    Ok(out)
}

/// Texts of LICENSE*/LICENCE*/COPYING*/NOTICE* files in a package folder.
fn license_texts(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_license_file(n))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| std::fs::read_to_string(dir.join(n)).ok())
        .map(|t| t.replace("\r\n", "\n"))
        .collect()
}

fn is_license_file(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"]
        .iter()
        .any(|p| upper.starts_with(p))
}

/// Production npm packages of the UI via `pnpm licenses list --prod --json`.
fn npm_packages(ui_dir: &Path) -> Result<Vec<Pkg>> {
    let pnpm = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let out = Command::new(pnpm)
        .args(["licenses", "list", "--prod", "--json"])
        .current_dir(ui_dir)
        .output()
        .map_err(|e| format!("cannot run pnpm: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    let json: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("pnpm licenses output: {e}"))?;
    Ok(parse_pnpm_licenses(&json))
}

/// `{ "<license>": [ {name, versions: [...], homepage?}, … ], … }` → packages.
fn parse_pnpm_licenses(json: &Value) -> Vec<Pkg> {
    let mut out = Vec::new();
    for (license, list) in json.as_object().into_iter().flatten() {
        for p in list.as_array().into_iter().flatten() {
            let versions: Vec<&str> = p["versions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            out.push(Pkg {
                ecosystem: "npm",
                name: p["name"].as_str().unwrap_or_default().to_owned(),
                version: versions.join(", "),
                license: license.clone(),
                repository: p["homepage"].as_str().unwrap_or_default().to_owned(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn license_file_names() {
        for ok in [
            "LICENSE",
            "LICENSE-MIT",
            "license.txt",
            "COPYING",
            "NOTICE.md",
            "UNLICENSE",
        ] {
            assert!(is_license_file(ok), "{ok}");
        }
        for no in ["README.md", "Cargo.toml", "lib.rs"] {
            assert!(!is_license_file(no), "{no}");
        }
    }

    #[test]
    fn only_normal_dependencies_of_the_roots_are_shipped() {
        let meta = json!({
            "packages": [
                {"id": "app", "name": "browsermcp", "version": "0.1.0", "source": null, "manifest_path": "/w/app/Cargo.toml"},
                {"id": "a", "name": "a", "version": "1.0.0", "source": "registry", "license": "MIT", "manifest_path": "/r/a/Cargo.toml"},
                {"id": "b", "name": "b", "version": "2.0.0", "source": "registry", "license": "Apache-2.0", "manifest_path": "/r/b/Cargo.toml"},
                {"id": "dev", "name": "dev", "version": "1.0.0", "source": "registry", "manifest_path": "/r/dev/Cargo.toml"},
                {"id": "other", "name": "other", "version": "1.0.0", "source": "registry", "manifest_path": "/r/o/Cargo.toml"}
            ],
            "resolve": {"nodes": [
                {"id": "app", "deps": [
                    {"pkg": "a", "dep_kinds": [{"kind": null}]},
                    {"pkg": "dev", "dep_kinds": [{"kind": "dev"}]}
                ]},
                {"id": "a", "deps": [{"pkg": "b", "dep_kinds": [{"kind": null}]}]},
                {"id": "b", "deps": []},
                {"id": "dev", "deps": []},
                {"id": "other", "deps": []}
            ]}
        });
        let names: Vec<String> = shipped_crates(&meta, &["browsermcp"])
            .unwrap()
            .into_iter()
            .map(|(p, _)| p.name)
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(shipped_crates(&meta, &["missing"]).is_err());
    }

    #[test]
    fn pnpm_license_json_is_parsed() {
        let json = json!({"MIT": [{"name": "svelte", "versions": ["5.1.0"], "homepage": "https://svelte.dev"}]});
        let pkgs = parse_pnpm_licenses(&json);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "svelte");
        assert_eq!(pkgs[0].license, "MIT");
        assert_eq!(pkgs[0].version, "5.1.0");
    }

    #[test]
    fn cef_license_falls_back_to_known_text() {
        let dir = std::env::temp_dir().join("bmcp-no-cef-license-here");
        assert!(cef_license(&dir).unwrap().contains("Chromium Embedded"));
    }
}
