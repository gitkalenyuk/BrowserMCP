//! Module loader: reads `.ts`/`.js` files from disk, transpiles TypeScript
//! with `deno_ast`, and keeps source maps so error stacks show TS lines.
//!
//! Only `file://` modules are allowed (no network imports). Types are stripped,
//! NOT checked (like `deno run --no-check`); type checking is the editor's job.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use deno_ast::{EmitOptions, MediaType, ParseParams, SourceMapOption, TranspileModuleOptions};
use deno_core::error::ModuleLoaderError;
use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleResolveResponse,
    ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind, resolve_import,
};
use deno_error::JsErrorBox;

#[derive(Default)]
pub struct TsModuleLoader {
    /// Source map per module URL (only for transpiled modules).
    source_maps: RefCell<HashMap<String, Vec<u8>>>,
    /// Original (TS) source text per module URL, to print the failing line.
    sources: RefCell<HashMap<String, String>>,
}

impl TsModuleLoader {
    fn load_file(&self, specifier: &ModuleSpecifier) -> Result<ModuleSource, ModuleLoaderError> {
        let path = specifier.to_file_path().map_err(|_| {
            JsErrorBox::generic(format!("only local files can be imported, got {specifier}"))
        })?;
        let media_type = MediaType::from_path(&path);
        let (module_type, transpile) = match media_type {
            MediaType::JavaScript | MediaType::Mjs => (ModuleType::JavaScript, false),
            MediaType::TypeScript | MediaType::Mts | MediaType::Tsx | MediaType::Jsx => {
                (ModuleType::JavaScript, true)
            }
            MediaType::Json => (ModuleType::Json, false),
            other => {
                return Err(JsErrorBox::generic(format!(
                    "unsupported module type {other:?}: {}",
                    path.display()
                )));
            }
        };

        let text = std::fs::read_to_string(&path)
            .map_err(|e| JsErrorBox::generic(format!("cannot read {}: {e}", path.display())))?;
        self.sources
            .borrow_mut()
            .insert(specifier.to_string(), text.clone());

        let code = if transpile {
            let parsed = deno_ast::parse_module(ParseParams {
                specifier: specifier.clone(),
                text: text.into(),
                media_type,
                capture_tokens: false,
                scope_analysis: false,
                maybe_syntax: None,
            })
            .map_err(JsErrorBox::from_err)?;
            let emitted = parsed
                .transpile(
                    &deno_ast::TranspileOptions::default(),
                    &TranspileModuleOptions { module_kind: None },
                    &EmitOptions {
                        // Keep the map separate; deno_core asks for it via
                        // `get_source_map` when it formats a stack trace.
                        source_map: SourceMapOption::Separate,
                        inline_sources: true,
                        ..Default::default()
                    },
                )
                .map_err(JsErrorBox::from_err)?
                .into_source();
            if let Some(map) = emitted.source_map {
                self.source_maps
                    .borrow_mut()
                    .insert(specifier.to_string(), map.into_bytes());
            }
            emitted.text
        } else {
            text
        };

        Ok(ModuleSource::new(
            module_type,
            ModuleSourceCode::String(code.into()),
            specifier,
            None,
        ))
    }
}

impl ModuleLoader for TsModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> ModuleResolveResponse {
        resolve_import(specifier, referrer).map_err(JsErrorBox::from_err)
    }

    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        // Reading a local file is fast, so we answer synchronously.
        ModuleLoadResponse::Sync(self.load_file(specifier))
    }

    fn get_source_map(&self, specifier: &str) -> Option<Cow<'_, [u8]>> {
        self.source_maps
            .borrow()
            .get(specifier)
            .map(|map| Cow::Owned(map.clone()))
    }

    fn get_source_mapped_source_line(&self, specifier: &str, line_index: usize) -> Option<String> {
        // `line_index` is 0-based.
        let sources = self.sources.borrow();
        sources
            .get(specifier)?
            .lines()
            .nth(line_index)
            .map(str::to_string)
    }
}
