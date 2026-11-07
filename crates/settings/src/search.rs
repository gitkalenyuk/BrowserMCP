//! Search engines of the address bar (`data/config/search.json`, `search.config`, `search.url`).
//!
//! Built-in engines (Google, Bing, DuckDuckGo, Brave, Startpage) are always there; people add
//! their own with a URL template where `%s` stands for the words. Every engine has a keyword:
//! typing `w cats` in the address bar searches "cats" with the engine whose keyword is `w`.
//! The UI resolves the address bar itself (ui/src/lib/settings/e1-search.ts, same rules as
//! [`resolve`]); `search.url` gives agents and scripts the same answer.

use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::urls::encode_component;
use crate::{BrowserSettings, SETTINGS_CHANGED};

/// The engine used when nothing else is chosen.
pub const DEFAULT_ENGINE: &str = "google";
/// Most custom engines.
pub const MAX_CUSTOM: usize = 50;

/// One search engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchEngine {
    /// Stable id: `google`, `bing`, … for built-ins, `custom-<name>` for added ones.
    pub id: String,
    pub name: String,
    /// Search URL with `%s` where the words go, e.g. `https://example.com/search?q=%s`.
    pub url: String,
    /// Shortcut typed before the words in the address bar (`@google cats`); unique.
    pub keyword: String,
    /// Built-in engines cannot be removed or edited.
    #[serde(default)]
    pub builtin: bool,
}

/// The built-in engines, in the order Settings lists them.
///
/// Their keywords start with `@` (as Chrome's `@bookmarks`) because people cannot change
/// them: a bare `d` or `b` would turn ordinary queries such as "d day" or "b 52" into a
/// search of the rest ("day", "52") with another engine.
pub fn builtin_engines() -> Vec<SearchEngine> {
    [
        (
            "google",
            "Google",
            "https://www.google.com/search?q=%s",
            "@google",
        ),
        ("bing", "Bing", "https://www.bing.com/search?q=%s", "@bing"),
        (
            "duckduckgo",
            "DuckDuckGo",
            "https://duckduckgo.com/?q=%s",
            "@ddg",
        ),
        (
            "brave",
            "Brave Search",
            "https://search.brave.com/search?q=%s",
            "@brave",
        ),
        (
            "startpage",
            "Startpage",
            "https://www.startpage.com/do/search?q=%s",
            "@startpage",
        ),
    ]
    .into_iter()
    .map(|(id, name, url, keyword)| SearchEngine {
        id: id.into(),
        name: name.into(),
        url: url.into(),
        keyword: keyword.into(),
        builtin: true,
    })
    .collect()
}

/// Contents of `search.json`: the default engine and the added ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchConfig {
    /// Id of the engine the address bar uses.
    #[serde(default = "default_engine")]
    pub default: String,
    /// Engines people added (built-ins are not stored).
    #[serde(default)]
    pub custom: Vec<SearchEngine>,
}

fn default_engine() -> String {
    DEFAULT_ENGINE.into()
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            default: default_engine(),
            custom: Vec::new(),
        }
    }
}

impl SearchConfig {
    /// Built-ins first, then the added engines.
    pub fn engines(&self) -> Vec<SearchEngine> {
        let mut all = builtin_engines();
        all.extend(self.custom.iter().cloned().map(|mut e| {
            e.builtin = false;
            e
        }));
        all
    }

    /// The default engine (Google when the stored id no longer exists).
    pub fn default_engine(&self) -> SearchEngine {
        let all = self.engines();
        all.iter()
            .find(|e| e.id == self.default)
            .or_else(|| all.iter().find(|e| e.id == DEFAULT_ENGINE))
            .cloned()
            .expect("the built-in default engine exists")
    }
}

/// `template` with `%s` replaced by the encoded words.
pub fn search_url(template: &str, words: &str) -> String {
    template.replace("%s", &encode_component(words))
}

/// The search for `input` (pure): `"<keyword> words"` uses that engine, anything else the
/// default one. Returns the engine and the URL; `None` for empty input.
pub fn resolve(config: &SearchConfig, input: &str) -> Option<(SearchEngine, String)> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some((word, rest)) = input.split_once(char::is_whitespace) {
        let rest = rest.trim();
        let engine = config
            .engines()
            .into_iter()
            .find(|e| e.keyword.eq_ignore_ascii_case(word));
        if let (Some(engine), false) = (engine, rest.is_empty()) {
            let url = search_url(&engine.url, rest);
            return Some((engine, url));
        }
    }
    let engine = config.default_engine();
    let url = search_url(&engine.url, input);
    Some((engine, url))
}

// --------------------------------------------------------------------------- validation

/// What people type for a new or changed engine.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineInput {
    pub name: String,
    /// Search URL with `%s` where the words go.
    pub url: String,
    /// Address bar shortcut (letters/digits, no spaces). Default: derived from the name.
    pub keyword: Option<String>,
}

fn check_template(url: &str) -> Result<String, CmdError> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(CmdError::invalid_params(format!(
            "the search URL must start with https:// or http://, got {url:?}"
        )));
    }
    if !url.contains("%s") {
        return Err(CmdError::invalid_params(
            "the search URL needs %s where the search words go, e.g. https://example.com/search?q=%s",
        ));
    }
    if url.chars().any(char::is_whitespace) {
        return Err(CmdError::invalid_params(
            "the search URL must not contain spaces",
        ));
    }
    Ok(url.to_string())
}

fn check_name(name: &str) -> Result<String, CmdError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(CmdError::invalid_params(
            "the engine name must be 1-60 characters",
        ));
    }
    Ok(name.to_string())
}

fn check_keyword(keyword: &str) -> Result<String, CmdError> {
    let keyword = keyword.trim().to_lowercase();
    let ok = !keyword.is_empty()
        && keyword.chars().count() <= 20
        && !keyword.chars().any(|c| c.is_whitespace() || c == '%');
    if ok {
        Ok(keyword)
    } else {
        Err(CmdError::invalid_params(
            "the keyword must be 1-20 characters without spaces",
        ))
    }
}

/// `"My Wiki!"` → `my-wiki` (for ids and default keywords).
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "engine".into() } else { out }
}

/// Refuse a keyword another engine already uses (`except` = the engine being edited).
fn unique_keyword(
    config: &SearchConfig,
    keyword: &str,
    except: Option<&str>,
) -> Result<(), CmdError> {
    match config
        .engines()
        .into_iter()
        .find(|e| Some(e.id.as_str()) != except && e.keyword.eq_ignore_ascii_case(keyword))
    {
        Some(other) => Err(CmdError::invalid_params(format!(
            "the keyword {keyword:?} is already used by {}",
            other.name
        ))),
        None => Ok(()),
    }
}

/// Add an engine (pure); returns its id.
pub fn add_engine(config: &mut SearchConfig, input: EngineInput) -> Result<String, CmdError> {
    if config.custom.len() >= MAX_CUSTOM {
        return Err(CmdError::invalid_params(format!(
            "at most {MAX_CUSTOM} custom engines"
        )));
    }
    let name = check_name(&input.name)?;
    let url = check_template(&input.url)?;
    let keyword = check_keyword(input.keyword.as_deref().unwrap_or(&slug(&name)))?;
    unique_keyword(config, &keyword, None)?;
    let base = format!("custom-{}", slug(&name));
    let taken = |id: &str| config.engines().iter().any(|e| e.id == id);
    let id = (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|id| !taken(id))
        .expect("an unused id exists");
    config.custom.push(SearchEngine {
        id: id.clone(),
        name,
        url,
        keyword,
        builtin: false,
    });
    Ok(id)
}

/// Change an added engine (pure). Built-ins cannot be changed.
pub fn update_engine(
    config: &mut SearchConfig,
    id: &str,
    input: EngineInput,
) -> Result<(), CmdError> {
    let name = check_name(&input.name)?;
    let url = check_template(&input.url)?;
    let keyword = check_keyword(input.keyword.as_deref().unwrap_or(&slug(&name)))?;
    unique_keyword(config, &keyword, Some(id))?;
    let engine = custom_mut(config, id)?;
    engine.name = name;
    engine.url = url;
    engine.keyword = keyword;
    Ok(())
}

/// Remove an added engine (pure); the default goes back to Google if it was this one.
pub fn remove_engine(config: &mut SearchConfig, id: &str) -> Result<(), CmdError> {
    custom_mut(config, id)?;
    config.custom.retain(|e| e.id != id);
    if config.default == id {
        config.default = default_engine();
    }
    Ok(())
}

fn custom_mut<'a>(
    config: &'a mut SearchConfig,
    id: &str,
) -> Result<&'a mut SearchEngine, CmdError> {
    if builtin_engines().iter().any(|e| e.id == id) {
        return Err(CmdError::invalid_params(format!(
            "{id} is a built-in engine and cannot be changed or removed"
        )));
    }
    config
        .custom
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, format!("no search engine {id:?}"))
                .with_hint("search_config lists the engines and their ids")
        })
}

// --------------------------------------------------------------------------- commands

pub struct Config;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigArgs {
    /// Make this engine (id) the address bar's default.
    pub default: Option<String>,
    /// Add an engine: {name, url with %s, keyword?}.
    pub add: Option<EngineInput>,
    /// Change an added engine: its id plus the new {name, url, keyword?}.
    pub update: Option<EngineUpdate>,
    /// Remove an added engine by id.
    pub remove: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineUpdate {
    /// Id of the added engine to change.
    pub id: String,
    pub name: String,
    /// Search URL with `%s` where the words go.
    pub url: String,
    /// Address bar shortcut. Default: derived from the name.
    pub keyword: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigOutput {
    /// Id of the default engine.
    pub default: String,
    /// Every engine: built-ins first, then the added ones.
    pub engines: Vec<SearchEngine>,
    /// Id of the engine `add` created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
}

impl Command for Config {
    const NAME: &'static str = "search.config";
    type Args = ConfigArgs;
    type Output = ConfigOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read or change the address bar's search engines (data/config/search.json): \
             default (engine id), add {name, url with %s, keyword?}, update {id, name, url, \
             keyword?}, remove (id). Built-ins: google, bing, duckduckgo, brave, startpage. No \
             arguments = read. Returns {default, engines:[{id,name,url,keyword,builtin}]}.",
            // A settings command like notify.config_set: not in the default MCP toolsets, so
            // an agent only gets it when its client asks for `ui` (it redirects every search).
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ConfigArgs) -> Result<ConfigOutput, CmdError> {
        let settings = BrowserSettings::from_ctx(&ctx)?;
        let changes = args.default.is_some()
            || args.add.is_some()
            || args.update.is_some()
            || args.remove.is_some();
        let (config, added) = if changes {
            let (config, added) = settings.files().search.update(|c| apply(c, args))?;
            ctx.events.emit(
                SETTINGS_CHANGED,
                serde_json::json!({ "area": "search", "default": config.default_engine().id }),
            );
            (config, added)
        } else {
            (settings.search(), None)
        };
        Ok(ConfigOutput {
            default: config.default_engine().id,
            engines: config.engines(),
            added,
        })
    }
}

/// Apply a `search.config` change (pure): remove, update, add, then the default (so a new
/// engine can be made the default in the same call by its returned id on the next call).
pub fn apply(config: &mut SearchConfig, args: ConfigArgs) -> Result<Option<String>, CmdError> {
    if let Some(id) = args.remove {
        remove_engine(config, &id)?;
    }
    if let Some(update) = args.update {
        let input = EngineInput {
            name: update.name,
            url: update.url,
            keyword: update.keyword,
        };
        update_engine(config, &update.id, input)?;
    }
    let added = args
        .add
        .map(|input| add_engine(config, input))
        .transpose()?;
    if let Some(id) = args.default {
        if !config.engines().iter().any(|e| e.id == id) {
            return Err(
                CmdError::new(ErrorCode::NotFound, format!("no search engine {id:?}"))
                    .with_hint("search_config lists the engines and their ids"),
            );
        }
        config.default = id;
    }
    Ok(added)
}

pub struct Url;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UrlArgs {
    /// What would be typed in the address bar: "cats", or "<keyword> cats".
    pub query: String,
    /// Use this engine (id) instead of the default / keyword.
    pub engine: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UrlOutput {
    /// The search results URL (open it with nav_goto or tabs_open).
    pub url: String,
    /// Id of the engine used.
    pub engine: String,
}

impl Command for Url {
    const NAME: &'static str = "search.url";
    type Args = UrlArgs;
    type Output = UrlOutput;

    fn meta() -> Meta {
        Meta::new(
            "The search results URL for a query with the browser's search engines (the default \
             one, or \"<keyword> words\", or `engine`). Opens nothing: pass the url to nav_goto. \
             Returns {url, engine}.",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, args: UrlArgs) -> Result<UrlOutput, CmdError> {
        let config = BrowserSettings::from_ctx(&ctx)?.search();
        let (engine, url) = match args.engine {
            Some(id) => {
                let engine = config
                    .engines()
                    .into_iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| {
                        CmdError::new(ErrorCode::NotFound, format!("no search engine {id:?}"))
                    })?;
                if args.query.trim().is_empty() {
                    return Err(CmdError::invalid_params("`query` must not be empty"));
                }
                let url = search_url(&engine.url, args.query.trim());
                (engine, url)
            }
            None => resolve(&config, &args.query)
                .ok_or_else(|| CmdError::invalid_params("`query` must not be empty"))?,
        };
        Ok(UrlOutput {
            url,
            engine: engine.id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, url: &str, keyword: Option<&str>) -> EngineInput {
        EngineInput {
            name: name.into(),
            url: url.into(),
            keyword: keyword.map(str::to_string),
        }
    }

    #[test]
    fn default_and_keyword_searches() {
        let mut c = SearchConfig::default();
        let (e, url) = resolve(&c, "  rust lang ").unwrap();
        assert_eq!(e.id, "google");
        assert_eq!(url, "https://www.google.com/search?q=rust%20lang");
        let (e, url) = resolve(&c, "@ddg cats & dogs").unwrap();
        assert_eq!(e.id, "duckduckgo");
        assert_eq!(url, "https://duckduckgo.com/?q=cats%20%26%20dogs");
        // A keyword alone is a plain search for that word.
        assert_eq!(resolve(&c, "@ddg").unwrap().0.id, "google");
        // Ordinary queries that start with a short word stay whole searches.
        for q in ["d day", "b 52", "sp 500", "g force", "br cats"] {
            let (e, url) = resolve(&c, q).unwrap();
            assert_eq!(e.id, "google", "{q}");
            assert_eq!(url, search_url(&e.url, q), "{q}");
        }
        assert!(resolve(&c, "   ").is_none());

        c.default = "brave".into();
        assert_eq!(resolve(&c, "x").unwrap().0.id, "brave");
        // A default that no longer exists falls back to Google.
        c.default = "custom-gone".into();
        assert_eq!(c.default_engine().id, "google");
    }

    #[test]
    fn add_update_remove_custom_engines() {
        let mut c = SearchConfig::default();
        let id = add_engine(
            &mut c,
            input(
                "Wikipedia (UK)",
                "https://uk.wikipedia.org/w/index.php?search=%s",
                Some("W"),
            ),
        )
        .unwrap();
        assert_eq!(id, "custom-wikipedia-uk");
        assert_eq!(c.custom[0].keyword, "w");
        assert_eq!(
            resolve(&c, "w Київ").unwrap().1,
            "https://uk.wikipedia.org/w/index.php?search=%D0%9A%D0%B8%D1%97%D0%B2"
        );

        // Same name again: a new id; keyword clash refused.
        let err = add_engine(
            &mut c,
            input("Wikipedia (UK)", "https://x.test/?q=%s", Some("w")),
        )
        .unwrap_err();
        assert!(err.message.contains("already used"), "{err:?}");
        let id2 = add_engine(
            &mut c,
            input("Wikipedia (UK)", "https://x.test/?q=%s", Some("wx")),
        )
        .unwrap();
        assert_eq!(id2, "custom-wikipedia-uk-2");
        let err =
            add_engine(&mut c, input("G2", "https://x.test/?q=%s", Some("@google"))).unwrap_err();
        assert!(err.message.contains("Google"), "{err:?}");

        // Templates must be web URLs with %s.
        assert!(add_engine(&mut c, input("Bad", "https://x.test/?q=", None)).is_err());
        assert!(add_engine(&mut c, input("Bad", "javascript:%s", None)).is_err());

        update_engine(&mut c, &id2, input("X", "https://x.test/s?q=%s", Some("x"))).unwrap();
        assert_eq!(c.custom[1].name, "X");
        // Keeping its own keyword is fine.
        update_engine(&mut c, &id2, input("X", "https://x.test/s?q=%s", Some("x"))).unwrap();
        assert!(update_engine(&mut c, "google", input("G", "https://g.test/?q=%s", None)).is_err());

        apply(
            &mut c,
            ConfigArgs {
                default: Some(id2.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.default, id2);
        remove_engine(&mut c, &id2).unwrap();
        assert_eq!(c.default, "google");
        assert!(remove_engine(&mut c, "bing").is_err());
        assert!(remove_engine(&mut c, "custom-none").is_err());
        assert!(
            apply(
                &mut c,
                ConfigArgs {
                    default: Some("nope".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
