use super::super::*;

pub(crate) fn resolved_default_provider(config: &Config) -> String {
    config
        .default_provider
        .clone()
        .unwrap_or_else(|| "openrouter".to_string())
}

pub(crate) fn resolved_default_model(config: &Config) -> String {
    config
        .default_model
        .clone()
        .unwrap_or_else(|| "anthropic/claude-sonnet-4.6".to_string())
}

pub(crate) fn runtime_defaults_from_config(config: &Config) -> ChannelRuntimeDefaults {
    ChannelRuntimeDefaults {
        default_provider: resolved_default_provider(config),
        model: resolved_default_model(config),
        temperature: config.default_temperature,
        api_key: config.api_key.clone(),
        api_url: config.api_url.clone(),
        reliability: config.reliability.clone(),
    }
}

fn runtime_config_path(ctx: &ChannelRuntimeContext) -> Option<PathBuf> {
    ctx.provider_runtime_options
        .zeroclaw_dir
        .as_ref()
        .map(|dir| dir.join("config.toml"))
}

pub(crate) fn runtime_defaults_snapshot(ctx: &ChannelRuntimeContext) -> ChannelRuntimeDefaults {
    if let Some(config_path) = runtime_config_path(ctx) {
        let store = runtime_config_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(state) = store.get(&config_path) {
            return state.defaults.clone();
        }
    }

    ChannelRuntimeDefaults {
        default_provider: ctx.default_provider.as_str().to_string(),
        model: ctx.model.as_str().to_string(),
        temperature: ctx.temperature,
        api_key: ctx.api_key.clone(),
        api_url: ctx.api_url.clone(),
        reliability: (*ctx.reliability).clone(),
    }
}

pub(crate) async fn config_file_stamp(path: &Path) -> Option<ConfigFileStamp> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let modified = metadata.modified().ok()?;
    Some(ConfigFileStamp {
        modified,
        len: metadata.len(),
    })
}

fn decrypt_optional_secret_for_runtime_reload(
    store: &crate::security::SecretStore,
    value: &mut Option<String>,
    field_name: &str,
) -> Result<()> {
    if let Some(raw) = value.clone() {
        if crate::security::SecretStore::is_encrypted(&raw) {
            *value = Some(
                store
                    .decrypt(&raw)
                    .with_context(|| format!("Failed to decrypt {field_name}"))?,
            );
        }
    }
    Ok(())
}

async fn load_runtime_defaults_from_config_file(path: &Path) -> Result<ChannelRuntimeDefaults> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut parsed: Config =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    parsed.config_path = path.to_path_buf();

    if let Some(zeroclaw_dir) = path.parent() {
        let store = crate::security::SecretStore::new(zeroclaw_dir, parsed.secrets.encrypt);
        decrypt_optional_secret_for_runtime_reload(&store, &mut parsed.api_key, "config.api_key")?;
        if let Some(ref mut openai) = parsed.tts.openai {
            decrypt_optional_secret_for_runtime_reload(
                &store,
                &mut openai.api_key,
                "config.tts.openai.api_key",
            )?;
        }
        if let Some(ref mut elevenlabs) = parsed.tts.elevenlabs {
            decrypt_optional_secret_for_runtime_reload(
                &store,
                &mut elevenlabs.api_key,
                "config.tts.elevenlabs.api_key",
            )?;
        }
        if let Some(ref mut google) = parsed.tts.google {
            decrypt_optional_secret_for_runtime_reload(
                &store,
                &mut google.api_key,
                "config.tts.google.api_key",
            )?;
        }
    }

    parsed.apply_env_overrides();
    Ok(runtime_defaults_from_config(&parsed))
}

pub(crate) async fn maybe_apply_runtime_config_update(ctx: &ChannelRuntimeContext) -> Result<()> {
    let Some(config_path) = runtime_config_path(ctx) else {
        return Ok(());
    };

    let Some(stamp) = config_file_stamp(&config_path).await else {
        return Ok(());
    };

    {
        let store = runtime_config_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(state) = store.get(&config_path) {
            if state.last_applied_stamp == Some(stamp) {
                return Ok(());
            }
        }
    }

    let next_defaults = load_runtime_defaults_from_config_file(&config_path).await?;
    let next_default_provider = providers::create_resilient_provider_with_options(
        &next_defaults.default_provider,
        next_defaults.api_key.as_deref(),
        next_defaults.api_url.as_deref(),
        &next_defaults.reliability,
        &ctx.provider_runtime_options,
    )?;
    let next_default_provider: Arc<dyn Provider> = Arc::from(next_default_provider);

    if let Err(err) = next_default_provider.warmup().await {
        tracing::warn!(
            provider = %next_defaults.default_provider,
            "Provider warmup failed after config reload: {err}"
        );
    }

    {
        let mut cache = ctx.provider_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.clear();
        cache.insert(
            next_defaults.default_provider.clone(),
            Arc::clone(&next_default_provider),
        );
    }

    {
        let mut store = runtime_config_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store.insert(
            config_path.clone(),
            RuntimeConfigState {
                defaults: next_defaults.clone(),
                last_applied_stamp: Some(stamp),
            },
        );
    }

    tracing::info!(
        path = %config_path.display(),
        provider = %next_defaults.default_provider,
        model = %next_defaults.model,
        temperature = next_defaults.temperature,
        "Applied updated channel runtime config from disk"
    );

    Ok(())
}
