use super::*;

fn new(name: &str, template: Option<&str>) -> NewProvider {
    NewProvider {
        name: name.into(),
        template: template.map(String::from),
        ..Default::default()
    }
}

fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn message(result: Result<impl std::fmt::Debug, CmdError>) -> String {
    result.unwrap_err().message
}

#[test]
fn presets_are_unique_and_complete() {
    let mut ids: Vec<_> = PRESETS.iter().map(|p| p.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), PRESETS.len());
    for id in [
        "anthropic",
        "openai",
        "gemini",
        "openrouter",
        "mistral",
        "deepseek",
        "xai",
        "groq",
        "ollama",
        "lmstudio",
        "claude-code",
        "codex",
    ] {
        assert!(preset(id).is_some(), "missing preset {id}");
    }
    // The single custom endpoint became a template for user providers.
    assert!(preset(BLANK_TEMPLATE).is_none());
    assert_eq!(
        preset("ollama").unwrap().base_url,
        "http://localhost:11434/v1"
    );
    assert_eq!(
        preset("lmstudio").unwrap().base_url,
        "http://localhost:1234/v1"
    );
}

#[test]
fn save_load_and_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config/ai.json");
    assert_eq!(AiConfig::load(&path), AiConfig::default());

    let mut config = AiConfig::default();
    config.providers.insert(
        "openai".into(),
        ProviderSettings {
            api_key: Some("sk-test".into()),
            models: vec!["m1".into(), "m2".into()],
            ..Default::default()
        },
    );
    config
        .add_provider(new("Home server", Some("ollama")))
        .unwrap();
    config.save(&path).unwrap();
    let loaded = AiConfig::load(&path);
    assert_eq!(loaded, config);

    let openai = loaded.resolve("openai").unwrap();
    assert!(openai.configured());
    assert!(openai.builtin);
    assert_eq!(openai.model.as_deref(), Some("m1"));
    assert_eq!(openai.base_url, "https://api.openai.com/v1");

    // No key → not configured; local providers need none.
    let gemini = loaded.resolve("gemini").unwrap();
    assert_eq!(gemini.problem(), Some(Problem::NoApiKey));
    assert!(gemini.ensure_configured().is_err());
    assert!(loaded.resolve("ollama").unwrap().configured());

    let home = loaded.resolve("home-server").unwrap();
    assert!(!home.builtin);
    assert!(home.configured());
    assert_eq!(home.base_url, "http://localhost:11434/v1");
    assert_eq!(home.kind, Kind::Api(Api::OpenAi));

    let err = loaded.resolve("nope").unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.candidates.contains(&"home-server".into()));
    assert_eq!(
        loaded.ids().last().map(String::as_str),
        Some("home-server"),
        "user providers come after the built-ins"
    );
}

#[test]
fn the_old_custom_endpoint_is_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ai.json");
    let old = r#"{
        "providers": {
            "custom": { "base_url": "http://10.0.0.5:8000/v1", "api_key": "k", "model": "my-model" },
            "anthropic": { "api_key": "sk-ant" }
        },
        "default_provider": "custom"
    }"#;
    std::fs::write(&path, old).unwrap();
    let config = AiConfig::load(&path);
    assert!(!config.providers.contains_key("custom"));
    assert_eq!(
        config.providers["anthropic"].api_key.as_deref(),
        Some("sk-ant")
    );
    let custom = config.user("custom").expect("migrated");
    assert_eq!(custom.name, "Custom (OpenAI-compatible)");
    assert_eq!(custom.api, Api::OpenAi);
    assert_eq!(custom.settings.api_key.as_deref(), Some("k"));
    assert_eq!(custom.settings.model.as_deref(), Some("my-model"));
    assert_eq!(config.default_provider.as_deref(), Some("custom"));
    let resolved = config.resolve("custom").unwrap();
    assert!(resolved.configured());
    assert_eq!(resolved.base_url, "http://10.0.0.5:8000/v1");

    // Saved and loaded again: nothing is migrated twice.
    config.save(&path).unwrap();
    let mut again = AiConfig::load(&path);
    assert_eq!(again, config);
    assert!(!again.migrate());

    // An empty old entry is just dropped.
    let mut empty = AiConfig::default();
    empty
        .providers
        .insert("custom".into(), ProviderSettings::default());
    assert!(empty.migrate());
    assert!(empty.user_providers.is_empty());
}

#[test]
fn user_providers_start_from_a_template() {
    let mut config = AiConfig::default();
    let id = config
        .add_provider(new("OpenAI (work)", Some("openai")))
        .unwrap();
    assert_eq!(id, "openai-work");
    let work = config.resolve(&id).unwrap();
    assert_eq!(work.base_url, "https://api.openai.com/v1");
    assert!(work.needs_key, "an OpenAI account needs its key");
    assert_eq!(work.problem(), Some(Problem::NoApiKey));
    assert_eq!(
        config.user(&id).unwrap().template.as_deref(),
        Some("openai")
    );

    // A second account of the same service, with a key and headers.
    let id = config
        .add_provider(NewProvider {
            api_key: Some(" sk-2 ".into()),
            headers: headers(&[("OpenAI-Organization", "org-1"), (" ", "dropped")]),
            ..new("OpenAI (home)", Some("openai"))
        })
        .unwrap();
    let home = config.resolve(&id).unwrap();
    assert!(home.configured());
    assert_eq!(home.api_key.as_deref(), Some("sk-2"));
    assert_eq!(home.headers, headers(&[("OpenAI-Organization", "org-1")]));
    assert!(home.client().is_some());

    // Anthropic template: its models and default model carry over.
    let id = config
        .add_provider(new("Claude via proxy", Some("anthropic")))
        .unwrap();
    let claude = config.resolve(&id).unwrap();
    assert_eq!(claude.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(claude.models.len(), 4);

    // Blank: OpenAI-compatible, no key needed, the URL is required.
    assert!(message(config.add_provider(new("Box", None))).contains("base_url is required"));
    let id = config
        .add_provider(NewProvider {
            base_url: Some("http://10.0.0.5:8000/v1/".into()),
            ..new("Box", Some("custom"))
        })
        .unwrap();
    let blank = config.resolve(&id).unwrap();
    assert_eq!(blank.base_url, "http://10.0.0.5:8000/v1");
    assert!(!blank.needs_key);
    assert!(blank.configured());
    assert_eq!(config.user(&id).unwrap().template, None);
}

#[test]
fn ids_are_made_from_names_and_stay_unique() {
    let mut config = AiConfig::default();
    let ollama = |name: &str| new(name, Some("ollama"));
    assert_eq!(config.add_provider(ollama("Lab GPU")).unwrap(), "lab-gpu");
    assert_eq!(
        config.add_provider(ollama("Lab-GPU!")).unwrap(),
        "lab-gpu-2"
    );
    // A name in another script still gets an id.
    assert_eq!(
        config.add_provider(ollama("Мій сервер")).unwrap(),
        "provider"
    );
    assert_eq!(
        config.add_provider(ollama("Ще один")).unwrap(),
        "provider-2"
    );
    // A name that slugs to a built-in id gets a suffix instead of shadowing it.
    let mut config = AiConfig::default();
    assert_eq!(config.add_provider(ollama("Ollama!")).unwrap(), "ollama-2");

    assert_eq!(slug("  OpenAI (work)  "), "openai-work");
    assert_eq!(slug("a--b__c"), "a-b-c");
    assert!(slug(&"x".repeat(200)).len() <= MAX_ID);

    // Explicit ids are checked.
    let with_id = |id: &str| NewProvider {
        id: Some(id.into()),
        ..ollama("Some name")
    };
    assert!(message(config.add_provider(with_id("openai"))).contains("already exists"));
    assert!(message(config.add_provider(with_id("Bad Id"))).contains("bad provider id"));
    assert_eq!(config.add_provider(with_id("gpu.1")).unwrap(), "gpu.1");
}

#[test]
fn add_rejects_bad_input_and_changes_nothing() {
    let mut config = AiConfig::default();
    config.add_provider(new("Lab", Some("lmstudio"))).unwrap();
    let before = config.clone();

    // Duplicate names, also against the built-ins and in another case.
    assert!(message(config.add_provider(new("lab", Some("ollama")))).contains("already exists"));
    assert!(message(config.add_provider(new("OLLAMA", Some("ollama")))).contains("already exists"));
    assert!(message(config.add_provider(new("  ", Some("ollama")))).contains("empty"));
    assert!(message(config.add_provider(new(&"n".repeat(65), Some("ollama")))).contains("longer"));

    let with_url = |url: &str| NewProvider {
        base_url: Some(url.into()),
        ..new("Other", None)
    };
    assert!(message(config.add_provider(with_url("localhost:8000/v1"))).contains("bad base URL"));
    assert!(message(config.add_provider(with_url("ftp://host/v1"))).contains("http://"));
    assert!(message(config.add_provider(with_url("not a url"))).contains("bad base URL"));

    let err = config.add_provider(new("Other", Some("nope"))).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.candidates.contains(&"custom".into()));
    assert!(message(config.add_provider(new("Other", Some("claude-code")))).contains("CLI agent"));
    let bad_header = NewProvider {
        headers: headers(&[("Bad Header", "x")]),
        ..new("Other", Some("ollama"))
    };
    assert!(message(config.add_provider(bad_header)).contains("bad header"));

    assert_eq!(config, before);
}

#[test]
fn update_renames_and_edits_user_providers() {
    let mut config = AiConfig::default();
    let id = config.add_provider(new("Lab", Some("ollama"))).unwrap();
    config.add_provider(new("Other", Some("ollama"))).unwrap();

    config
        .update_provider(
            &id,
            ProviderChanges {
                name: Some("LAB ".into()),
                api: Some(Api::Anthropic),
                base_url: Some("https://proxy.test/".into()),
                api_key: Some("k".into()),
                headers: Some(headers(&[("X-Team", "lab")])),
                default: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let user = config.user(&id).unwrap();
    assert_eq!((user.name.as_str(), user.api), ("LAB", Api::Anthropic));
    assert_eq!(user.id, id, "renaming keeps the id");
    let resolved = config.resolve(&id).unwrap();
    assert_eq!(resolved.base_url, "https://proxy.test");
    assert_eq!(resolved.kind, Kind::Api(Api::Anthropic));
    assert_eq!(config.default_provider.as_deref(), Some(id.as_str()));

    // Headers are replaced as a whole; {} removes them. "" clears the key.
    let clear = ProviderChanges {
        headers: Some(BTreeMap::new()),
        api_key: Some(String::new()),
        default: Some(false),
        ..Default::default()
    };
    config.update_provider(&id, clear).unwrap();
    let resolved = config.resolve(&id).unwrap();
    assert!(resolved.headers.is_empty());
    assert!(resolved.api_key.is_none());
    assert_eq!(config.default_provider, None);

    // Errors leave everything as it was.
    let before = config.clone();
    let rename = |name: &str| ProviderChanges {
        name: Some(name.into()),
        model: Some("m".into()),
        ..Default::default()
    };
    assert!(message(config.update_provider(&id, rename("other"))).contains("already exists"));
    let bad_url = ProviderChanges {
        base_url: Some("nope".into()),
        ..Default::default()
    };
    assert!(message(config.update_provider(&id, bad_url)).contains("bad base URL"));
    assert_eq!(config, before);

    // A user provider without a URL is not configured.
    let no_url = ProviderChanges {
        base_url: Some(String::new()),
        ..Default::default()
    };
    config.update_provider(&id, no_url).unwrap();
    assert_eq!(
        config.resolve(&id).unwrap().problem(),
        Some(Problem::NoBaseUrl)
    );
}

#[test]
fn update_guards_the_built_ins() {
    let mut config = AiConfig::default();
    let rename = ProviderChanges {
        name: Some("Mine".into()),
        ..Default::default()
    };
    assert!(message(config.update_provider("openai", rename)).contains("built-in"));
    let cli_headers = ProviderChanges {
        headers: Some(headers(&[("X-A", "b")])),
        ..Default::default()
    };
    assert!(message(config.update_provider("claude-code", cli_headers)).contains("CLI agent"));
    let err = config
        .update_provider("nope", ProviderChanges::default())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    let set = ProviderChanges {
        base_url: Some("http://127.0.0.1:9000/v1".into()),
        headers: Some(headers(&[("HTTP-Referer", "https://me.test")])),
        ..Default::default()
    };
    config.update_provider("openrouter", set).unwrap();
    let r = config.resolve("openrouter").unwrap();
    assert_eq!(r.base_url, "http://127.0.0.1:9000/v1");
    assert_eq!(r.headers.len(), 1);
    // "" = back to the preset's URL.
    let reset = ProviderChanges {
        base_url: Some(String::new()),
        ..Default::default()
    };
    config.update_provider("openrouter", reset).unwrap();
    assert_eq!(
        config.resolve("openrouter").unwrap().base_url,
        "https://openrouter.ai/api/v1"
    );
}

#[test]
fn provider_set_on_custom_still_creates_the_old_endpoint() {
    let mut config = AiConfig::default();
    let set = ProviderChanges {
        base_url: Some("http://127.0.0.1:9/v1".into()),
        model: Some("test-model".into()),
        ..Default::default()
    };
    config.update_provider("custom", set).unwrap();
    let custom = config.resolve("custom").unwrap();
    assert!(!custom.builtin);
    assert!(custom.configured());
    assert_eq!(custom.model.as_deref(), Some("test-model"));
    // A failing first call creates nothing.
    let mut config = AiConfig::default();
    let bad = ProviderChanges {
        base_url: Some("nope".into()),
        ..Default::default()
    };
    assert!(config.update_provider("custom", bad).is_err());
    assert!(config.user("custom").is_none());
}

#[test]
fn remove_only_user_providers() {
    let mut config = AiConfig::default();
    let id = config.add_provider(new("Lab", Some("ollama"))).unwrap();
    config.default_provider = Some(id.clone());
    assert!(message(config.remove_provider("openai")).contains("cannot be removed"));
    assert_eq!(
        config.remove_provider("nope").unwrap_err().code,
        ErrorCode::NotFound
    );
    assert!(config.remove_provider(&id).unwrap(), "it was the default");
    assert_eq!(config.default_provider, None);
    assert!(config.resolve(&id).is_err());
    // The name is free again.
    config.add_provider(new("Lab", Some("ollama"))).unwrap();
}

#[test]
fn extra_settings_fields_survive_for_user_providers() {
    // ProviderSettings is flattened into a user provider: fields stay at the top level.
    let json = r#"{ "user_providers": [
        { "id": "lab", "name": "Lab", "api": "gemini", "base_url": "https://g.test",
          "api_key": "k", "headers": { "X-A": "b" }, "models": ["g1"] }
    ] }"#;
    let config: AiConfig = serde_json::from_str(json).unwrap();
    let lab = config.resolve("lab").unwrap();
    assert_eq!(lab.kind, Kind::Api(Api::Gemini));
    assert_eq!(lab.model.as_deref(), Some("g1"));
    assert!(!lab.needs_key, "no template: the key is optional");
    let back = serde_json::to_value(&config).unwrap();
    assert_eq!(back["user_providers"][0]["headers"]["X-A"], "b");
}
