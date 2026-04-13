//! `LiveProviderSetupService` — the runtime implementation of
//! `ProviderSetupService` that manages provider credentials, OAuth flows,
//! key validation, and provider registry rebuilds.

#[path = "support.rs"]
mod support;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use secrecy::{ExposeSecret, Secret};

use {
    async_trait::async_trait,
    serde_json::{Map, Value},
    tokio::sync::{OnceCell, RwLock},
    tracing::{info, warn},
};

use {
    moltis_config::schema::ProvidersConfig,
    moltis_oauth::{
        CallbackServer, OAuthFlow, TokenStore, callback_port, device_flow, load_oauth_config,
        normalize_loopback_redirect,
    },
    moltis_providers::ProviderRegistry,
    moltis_service_traits::{ProviderSetupService, ServiceError, ServiceResult},
};

pub use self::support::ErrorParser;
use {
    self::support::{
        PendingOAuthFlow, ProviderSetupTiming, default_error_parser, progress_payload,
    },
    crate::{
        SetupBroadcaster,
        config_helpers::{
            config_with_saved_keys, env_value_with_overrides, home_key_store, home_provider_config,
            home_token_store, normalize_provider_name, set_provider_enabled_in_config,
            ui_offered_provider_order, ui_offered_provider_set,
        },
        custom_providers::{
            base_url_to_display_name, derive_provider_name_from_url,
            existing_custom_provider_for_base_url, is_custom_provider, make_unique_provider_name,
            validation_provider_name_for_endpoint,
        },
        key_store::{KeyStore, normalize_model_list, parse_models_param},
        known_providers::{AuthType, KnownProvider, known_providers},
        oauth::{
            build_provider_headers, build_verification_uri_complete, has_oauth_tokens,
            normalize_loaded_redirect_uri,
        },
        ollama::{
            discover_ollama_models, normalize_ollama_api_base_url, normalize_ollama_model_id,
            normalize_ollama_openai_base_url, ollama_model_matches, ollama_models_payload,
        },
    },
};

// ── LiveProviderSetupService ───────────────────────────────────────────────

pub struct LiveProviderSetupService {
    registry: Arc<RwLock<ProviderRegistry>>,
    config: Arc<Mutex<ProvidersConfig>>,
    broadcaster: Arc<OnceCell<Arc<dyn SetupBroadcaster>>>,
    token_store: TokenStore,
    pub(crate) key_store: KeyStore,
    pending_oauth: Arc<RwLock<HashMap<String, PendingOAuthFlow>>>,
    /// When set, local-only providers (local-llm, ollama) are hidden from
    /// the available list because they cannot run on cloud VMs.
    deploy_platform: Option<String>,
    /// Shared priority models list from `LiveModelService`. Updated by
    /// `save_model` so the dropdown ordering reflects the latest preference.
    priority_models: Option<Arc<RwLock<Vec<String>>>>,
    /// Monotonic sequence used to drop stale async registry refreshes.
    registry_rebuild_seq: Arc<AtomicU64>,
    /// Static env overrides (for example config `[env]`) used when resolving
    /// provider credentials without mutating the process environment.
    env_overrides: HashMap<String, String>,
    /// Injected error parser for interpreting provider API errors.
    error_parser: ErrorParser,
    /// Address the OAuth callback server binds to. Defaults to `127.0.0.1`
    /// for local development; set to `0.0.0.0` in Docker / remote
    /// deployments so the callback port is reachable from the host.
    callback_bind_addr: String,
}

impl LiveProviderSetupService {
    pub fn new(
        registry: Arc<RwLock<ProviderRegistry>>,
        config: ProvidersConfig,
        deploy_platform: Option<String>,
    ) -> Self {
        Self {
            registry,
            config: Arc::new(Mutex::new(config)),
            broadcaster: Arc::new(OnceCell::new()),
            token_store: TokenStore::new(),
            key_store: KeyStore::new(),
            pending_oauth: Arc::new(RwLock::new(HashMap::new())),
            deploy_platform,
            priority_models: None,
            registry_rebuild_seq: Arc::new(AtomicU64::new(0)),
            env_overrides: HashMap::new(),
            error_parser: default_error_parser,
            callback_bind_addr: "127.0.0.1".to_string(),
        }
    }

    pub fn with_env_overrides(mut self, env_overrides: HashMap<String, String>) -> Self {
        self.env_overrides = env_overrides;
        self
    }

    /// Set a custom error parser for interpreting provider API errors.
    pub fn with_error_parser(mut self, parser: ErrorParser) -> Self {
        self.error_parser = parser;
        self
    }

    /// Set the bind address for the OAuth callback server.
    ///
    /// Defaults to `127.0.0.1`. Pass `0.0.0.0` when the gateway is
    /// bound to all interfaces (e.g. Docker) so the OAuth callback port
    /// is reachable from the host.
    pub fn with_callback_bind_addr(mut self, addr: String) -> Self {
        self.callback_bind_addr = addr;
        self
    }

    /// Wire the shared priority models handle from `LiveModelService` so
    /// `save_model` can update dropdown ordering at runtime.
    pub fn set_priority_models(&mut self, handle: Arc<RwLock<Vec<String>>>) {
        self.priority_models = Some(handle);
    }

    /// Set the broadcaster so validation can publish live progress events
    /// to the UI over WebSocket.
    pub fn set_broadcaster(&self, broadcaster: Arc<dyn SetupBroadcaster>) {
        let _ = self.broadcaster.set(broadcaster);
    }

    async fn emit_validation_progress(
        &self,
        provider: &str,
        request_id: Option<&str>,
        phase: &str,
        mut extra: Map<String, Value>,
    ) {
        let Some(broadcaster) = self.broadcaster.get() else {
            return;
        };

        let mut payload = Map::new();
        payload.insert("provider".to_string(), Value::String(provider.to_string()));
        payload.insert("phase".to_string(), Value::String(phase.to_string()));
        if let Some(id) = request_id {
            payload.insert("requestId".to_string(), Value::String(id.to_string()));
        }
        payload.append(&mut extra);

        broadcaster
            .broadcast("providers.validate.progress", Value::Object(payload))
            .await;
    }

    fn queue_registry_rebuild(&self, provider_name: &str, reason: &'static str) {
        let rebuild_seq = self.registry_rebuild_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let latest_seq = Arc::clone(&self.registry_rebuild_seq);
        let registry = Arc::clone(&self.registry);
        let config = Arc::clone(&self.config);
        let key_store = self.key_store.clone();
        let env_overrides = self.env_overrides.clone();
        let provider_name = provider_name.to_string();

        tokio::spawn(async move {
            let started = std::time::Instant::now();
            info!(
                provider = %provider_name,
                reason,
                rebuild_seq,
                "provider registry async rebuild started"
            );

            let effective = {
                let base = config.lock().unwrap_or_else(|e| e.into_inner()).clone();
                config_with_saved_keys(&base, &key_store, &[])
            };

            let new_registry = match tokio::task::spawn_blocking(move || {
                ProviderRegistry::from_env_with_config_and_overrides(&effective, &env_overrides)
            })
            .await
            {
                Ok(registry) => registry,
                Err(error) => {
                    warn!(
                        provider = %provider_name,
                        reason,
                        rebuild_seq,
                        error = %error,
                        "provider registry async rebuild worker failed"
                    );
                    return;
                },
            };

            let current_seq = latest_seq.load(Ordering::Acquire);
            if rebuild_seq != current_seq {
                info!(
                    provider = %provider_name,
                    reason,
                    rebuild_seq,
                    latest_seq = current_seq,
                    elapsed_ms = started.elapsed().as_millis(),
                    "provider registry async rebuild skipped as stale"
                );
                return;
            }

            let provider_summary = new_registry.provider_summary();
            let model_count = new_registry.list_models().len();
            let mut reg = registry.write().await;
            *reg = new_registry;
            info!(
                provider = %provider_name,
                reason,
                rebuild_seq,
                provider_summary = %provider_summary,
                models = model_count,
                elapsed_ms = started.elapsed().as_millis(),
                "provider registry async rebuild finished"
            );
        });
    }

    fn config_snapshot(&self) -> ProvidersConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_provider_enabled_in_memory(&self, provider: &str, enabled: bool) {
        let mut cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.providers
            .entry(provider.to_string())
            .or_default()
            .enabled = enabled;
    }

    fn is_provider_configured(
        &self,
        provider: &KnownProvider,
        active_config: &ProvidersConfig,
    ) -> bool {
        // Disabled providers (by offered allowlist or explicit enabled=false)
        // should not show as configured, except subscription-backed OAuth
        // providers with valid local tokens.
        if !active_config.is_enabled(provider.name) {
            let subscription_with_tokens =
                matches!(provider.name, "openai-codex" | "github-copilot")
                    && active_config
                        .get(provider.name)
                        .is_none_or(|entry| entry.enabled)
                    && has_oauth_tokens(provider.name, &self.token_store);
            if !subscription_with_tokens {
                return false;
            }
        }

        // Check if the provider has an API key set via env
        if let Some(env_key) = provider.env_key
            && env_value_with_overrides(&self.env_overrides, env_key).is_some()
        {
            return true;
        }
        if provider.auth_type == AuthType::ApiKey
            && moltis_config::generic_provider_api_key_from_env(provider.name, &self.env_overrides)
                .is_some()
        {
            return true;
        }
        // Check config file
        if let Some(entry) = active_config.get(provider.name)
            && entry
                .api_key
                .as_ref()
                .is_some_and(|k| !k.expose_secret().is_empty())
        {
            return true;
        }
        // Check home/global config file as fallback when using custom config dir.
        if home_provider_config()
            .as_ref()
            .and_then(|(cfg, _)| cfg.get(provider.name))
            .and_then(|entry| entry.api_key.as_ref())
            .is_some_and(|k| !k.expose_secret().is_empty())
        {
            return true;
        }
        // Check persisted key store
        if self.key_store.load(provider.name).is_some() {
            return true;
        }
        // Check persisted key store in user-global config dir.
        if home_key_store()
            .as_ref()
            .is_some_and(|(store, _)| store.load(provider.name).is_some())
        {
            return true;
        }
        // For OAuth providers, check token store
        if provider.auth_type == AuthType::Oauth || provider.name == "kimi-code" {
            if self.token_store.load(provider.name).is_some() {
                return true;
            }
            if home_token_store()
                .as_ref()
                .is_some_and(|(store, _)| store.load(provider.name).is_some())
            {
                return true;
            }
            // Match provider-registry behavior: openai-codex may be inferred from
            // Codex CLI auth at ~/.codex/auth.json.
            if provider.name == "openai-codex"
                && crate::oauth::codex_cli_auth_path()
                    .as_deref()
                    .is_some_and(crate::oauth::codex_cli_auth_has_access_token)
            {
                return true;
            }
            return false;
        }
        // For local providers, check if model is configured in local_llm config
        #[cfg(feature = "local-llm")]
        if provider.auth_type == AuthType::Local && provider.name == "local-llm" {
            // Check if local-llm model config file exists
            if let Some(config_dir) = moltis_config::config_dir() {
                let config_path = config_dir.join("local-llm.json");
                return config_path.exists();
            }
        }
        false
    }

    /// Start a device-flow OAuth for providers like GitHub Copilot.
    /// Returns `{ "userCode": "...", "verificationUri": "..." }` for the UI to display.
    async fn oauth_start_device_flow(
        &self,
        provider_name: String,
        oauth_config: moltis_oauth::OAuthConfig,
    ) -> ServiceResult {
        let client = reqwest::Client::new();
        let extra_headers = build_provider_headers(&provider_name);
        let device_resp = device_flow::request_device_code_with_headers(
            &client,
            &oauth_config,
            extra_headers.as_ref(),
        )
        .await
        .map_err(ServiceError::message)?;

        let user_code = device_resp.user_code.clone();
        let verification_uri = device_resp.verification_uri.clone();
        let verification_uri_complete = build_verification_uri_complete(
            &provider_name,
            &verification_uri,
            &user_code,
            device_resp.verification_uri_complete.clone(),
        );
        let device_code = device_resp.device_code.clone();
        let interval = device_resp.interval;

        // Spawn background task to poll for the token
        let token_store = self.token_store.clone();
        let registry = Arc::clone(&self.registry);
        let config = self.effective_config();
        let env_overrides = self.env_overrides.clone();
        let poll_headers = extra_headers.clone();
        tokio::spawn(async move {
            let poll_extra = poll_headers.as_ref();
            match device_flow::poll_for_token_with_headers(
                &client,
                &oauth_config,
                &device_code,
                interval,
                poll_extra,
            )
            .await
            {
                Ok(tokens) => {
                    if let Err(e) = token_store.save(&provider_name, &tokens) {
                        tracing::error!(
                            provider = %provider_name,
                            error = %e,
                            "failed to save device-flow OAuth tokens"
                        );
                        return;
                    }
                    let new_registry = ProviderRegistry::from_env_with_config_and_overrides(
                        &config,
                        &env_overrides,
                    );
                    let provider_summary = new_registry.provider_summary();
                    let model_count = new_registry.list_models().len();
                    let mut reg = registry.write().await;
                    *reg = new_registry;
                    info!(
                        provider = %provider_name,
                        provider_summary = %provider_summary,
                        models = model_count,
                        "device-flow OAuth complete, rebuilt provider registry"
                    );
                },
                Err(e) => {
                    tracing::error!(
                        provider = %provider_name,
                        error = %e,
                        "device-flow OAuth polling failed"
                    );
                },
            }
        });

        Ok(serde_json::json!({
            "deviceFlow": true,
            "userCode": user_code,
            "verificationUri": verification_uri,
            "verificationUriComplete": verification_uri_complete,
        }))
    }

    /// Build a ProvidersConfig that includes saved keys for registry rebuild.
    fn effective_config(&self) -> ProvidersConfig {
        let base = self.config_snapshot();
        config_with_saved_keys(&base, &self.key_store, &[])
    }

    fn build_registry(&self, config: &ProvidersConfig) -> ProviderRegistry {
        ProviderRegistry::from_env_with_config_and_overrides(config, &self.env_overrides)
    }
}

#[async_trait]
impl ProviderSetupService for LiveProviderSetupService {
    async fn available(&self) -> ServiceResult {
        let is_cloud = self.deploy_platform.is_some();
        let active_config = self.config_snapshot();
        let offered_order = ui_offered_provider_order(&active_config);
        let offered = ui_offered_provider_set(&offered_order);
        let offered_rank: HashMap<String, usize> = offered_order
            .iter()
            .enumerate()
            .map(|(idx, provider)| (provider.clone(), idx))
            .collect();

        let mut providers: Vec<(Option<usize>, usize, Value)> = known_providers()
            .iter()
            .enumerate()
            .filter_map(|(known_idx, provider)| {
                // Hide local-only providers on cloud deployments.
                if is_cloud && provider.is_local_only() {
                    return None;
                }

                let configured = self.is_provider_configured(provider, &active_config);
                let normalized_name = normalize_provider_name(provider.name);
                if let Some(allowed) = offered.as_ref()
                    && !allowed.contains(&normalized_name)
                    && !configured
                {
                    return None;
                }

                // Get saved config for this provider (baseUrl, preferred models)
                let saved_config = self.key_store.load_config(provider.name);
                let base_url = saved_config.as_ref().and_then(|c| c.base_url.clone());
                let models = saved_config
                    .map(|c| normalize_model_list(c.models))
                    .unwrap_or_default();
                let model = models.first().cloned();

                Some((
                    offered_rank.get(&normalized_name).copied(),
                    known_idx,
                    serde_json::json!({
                        "name": provider.name,
                        "displayName": provider.display_name,
                        "authType": provider.auth_type.as_str(),
                        "configured": configured,
                        "defaultBaseUrl": provider.default_base_url,
                        "baseUrl": base_url,
                        "models": models,
                        "model": model,
                        "requiresModel": provider.requires_model,
                        "keyOptional": provider.key_optional,
                    }),
                ))
            })
            .collect();

        // Append custom providers from the key store.
        let known_count = providers.len();
        for (name, config) in self.key_store.load_all_configs() {
            if !is_custom_provider(&name) {
                continue;
            }
            if active_config.get(&name).is_some_and(|entry| !entry.enabled) {
                continue;
            }
            let display_name = config.display_name.clone().unwrap_or_else(|| name.clone());
            let base_url = config.base_url.clone();
            let models = normalize_model_list(config.models.clone());
            let model = models.first().cloned();

            providers.push((
                None,
                known_count, // sort after all known providers
                serde_json::json!({
                    "name": name,
                    "displayName": display_name,
                    "authType": "api-key",
                    "configured": true,
                    "defaultBaseUrl": base_url,
                    "baseUrl": base_url,
                    "models": models,
                    "model": model,
                    "requiresModel": true,
                    "keyOptional": false,
                    "isCustom": true,
                }),
            ));
        }

        providers.sort_by(
            |(a_offered, a_known, a_value), (b_offered, b_known, b_value)| {
                let offered_cmp = match (a_offered, b_offered) {
                    (Some(a), Some(b)) => a.cmp(b),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                if offered_cmp != std::cmp::Ordering::Equal {
                    return offered_cmp;
                }

                let known_cmp = a_known.cmp(b_known);
                if known_cmp != std::cmp::Ordering::Equal {
                    return known_cmp;
                }

                let a_name = a_value
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let b_name = b_value
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                a_name.cmp(b_name)
            },
        );

        let providers: Vec<Value> = providers
            .into_iter()
            .enumerate()
            .map(|(idx, (_, _, mut value))| {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("uiOrder".into(), serde_json::json!(idx));
                }
                value
            })
            .collect();

        Ok(Value::Array(providers))
    }

    async fn save_key(&self, params: Value) -> ServiceResult {
        let _timing = ProviderSetupTiming::start(
            "providers.save_key",
            params.get("provider").and_then(Value::as_str),
        );
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?;

        // API key is optional for some providers (e.g., Ollama)
        let api_key = params.get("apiKey").and_then(|v| v.as_str());
        let base_url = params.get("baseUrl").and_then(|v| v.as_str());
        let models = parse_models_param(&params);

        // Custom providers bypass known_providers() validation.
        let is_custom = is_custom_provider(provider_name);
        if !is_custom {
            // Validate provider name - allow both api-key and local providers
            let known = known_providers();
            let provider = known
                .iter()
                .find(|p| {
                    p.name == provider_name
                        && (p.auth_type == AuthType::ApiKey || p.auth_type == AuthType::Local)
                })
                .ok_or_else(|| format!("unknown provider: {provider_name}"))?;

            // API key is required for api-key providers unless the provider
            // marks the key as optional (Ollama, LM Studio).
            if provider.auth_type == AuthType::ApiKey && !provider.key_optional && api_key.is_none()
            {
                return Err("missing 'apiKey' parameter".into());
            }
        } else if api_key.is_none() {
            return Err("missing 'apiKey' parameter".into());
        }

        let normalized_base_url = if provider_name == "ollama" {
            base_url.map(|url| normalize_ollama_openai_base_url(Some(url)))
        } else {
            base_url.map(String::from)
        };

        let key_store_path = self.key_store.path();
        info!(
            provider = provider_name,
            has_api_key = api_key.is_some(),
            has_base_url = normalized_base_url
                .as_ref()
                .is_some_and(|url| !url.trim().is_empty()),
            models = models.len(),
            key_store_path = %key_store_path.display(),
            "saving provider config"
        );

        // Persist full config to disk
        if let Err(error) = self.key_store.save_config(
            provider_name,
            api_key.map(String::from),
            normalized_base_url,
            (!models.is_empty()).then_some(models),
        ) {
            warn!(
                provider = provider_name,
                key_store_path = %key_store_path.display(),
                error = %error,
                "failed to persist provider config"
            );
            return Err(ServiceError::message(error));
        }
        set_provider_enabled_in_config(provider_name, true)?;
        self.set_provider_enabled_in_memory(provider_name, true);

        // Rebuild the provider registry with saved keys merged into config.
        let effective = self.effective_config();
        let new_registry = self.build_registry(&effective);
        let provider_summary = new_registry.provider_summary();
        let model_count = new_registry.list_models().len();
        let mut reg = self.registry.write().await;
        *reg = new_registry;

        info!(
            provider = provider_name,
            provider_summary = %provider_summary,
            models = model_count,
            "saved provider config to disk and rebuilt provider registry"
        );

        Ok(serde_json::json!({ "ok": true }))
    }

    async fn oauth_start(&self, params: Value) -> ServiceResult {
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?
            .to_string();

        // RFC 8252 S7.3/S8.3: loopback redirect URIs must use `http`.
        let redirect_uri = params
            .get("redirectUri")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(normalize_loopback_redirect);

        let mut oauth_config = load_oauth_config(&provider_name)
            .ok_or_else(|| format!("no OAuth config for provider: {provider_name}"))?;

        normalize_loaded_redirect_uri(&mut oauth_config);

        // User explicitly initiated OAuth for this provider; ensure it is enabled.
        set_provider_enabled_in_config(&provider_name, true)?;
        self.set_provider_enabled_in_memory(&provider_name, true);

        // If tokens already exist, skip launching a fresh OAuth flow.
        if has_oauth_tokens(&provider_name, &self.token_store) {
            let effective = self.effective_config();
            let new_registry = self.build_registry(&effective);
            let provider_summary = new_registry.provider_summary();
            let model_count = new_registry.list_models().len();
            let mut reg = self.registry.write().await;
            *reg = new_registry;
            info!(
                provider = %provider_name,
                provider_summary = %provider_summary,
                models = model_count,
                "oauth start skipped because provider already has tokens; rebuilt provider registry"
            );
            return Ok(serde_json::json!({
                "alreadyAuthenticated": true,
            }));
        }

        if oauth_config.device_flow {
            return self
                .oauth_start_device_flow(provider_name, oauth_config)
                .await;
        }

        let has_registered_redirect = !oauth_config.redirect_uri.is_empty();
        let use_server_callback = redirect_uri.is_some() && !has_registered_redirect;
        if !has_registered_redirect && let Some(uri) = redirect_uri {
            oauth_config.redirect_uri = uri;
        }

        let port = callback_port(&oauth_config);
        let oauth_config_for_pending = oauth_config.clone();
        let flow = OAuthFlow::new(oauth_config);
        let auth_req = flow.start().map_err(ServiceError::message)?;

        let auth_url = auth_req.url.clone();
        let verifier = auth_req.pkce.verifier.clone();
        let expected_state = auth_req.state.clone();

        let pending = PendingOAuthFlow {
            provider_name: provider_name.clone(),
            oauth_config: oauth_config_for_pending,
            verifier: verifier.clone(),
        };
        self.pending_oauth
            .write()
            .await
            .insert(expected_state.clone(), pending);

        if use_server_callback {
            return Ok(serde_json::json!({
                "authUrl": auth_url,
            }));
        }

        // Spawn background task to wait for the callback and exchange the code
        let token_store = self.token_store.clone();
        let registry = Arc::clone(&self.registry);
        let config = self.effective_config();
        let env_overrides = self.env_overrides.clone();
        let bind_addr = self.callback_bind_addr.clone();
        let pending_oauth = Arc::clone(&self.pending_oauth);
        let callback_state = expected_state.clone();
        tokio::spawn(async move {
            match CallbackServer::wait_for_code(port, callback_state, &bind_addr).await {
                Ok(code) => {
                    let state_is_pending = pending_oauth
                        .write()
                        .await
                        .remove(&expected_state)
                        .is_some();
                    if !state_is_pending {
                        tracing::debug!(
                            provider = %provider_name,
                            "OAuth callback received after flow was already completed manually"
                        );
                        return;
                    }

                    match flow.exchange(&code, &verifier).await {
                        Ok(tokens) => {
                            if let Err(e) = token_store.save(&provider_name, &tokens) {
                                tracing::error!(
                                    provider = %provider_name,
                                    error = %e,
                                    "failed to save OAuth tokens"
                                );
                                return;
                            }
                            // Rebuild registry with new tokens
                            let new_registry = ProviderRegistry::from_env_with_config_and_overrides(
                                &config,
                                &env_overrides,
                            );
                            let provider_summary = new_registry.provider_summary();
                            let model_count = new_registry.list_models().len();
                            let mut reg = registry.write().await;
                            *reg = new_registry;
                            info!(
                                provider = %provider_name,
                                provider_summary = %provider_summary,
                                models = model_count,
                                "OAuth flow complete, rebuilt provider registry"
                            );
                        },
                        Err(e) => {
                            tracing::error!(
                                provider = %provider_name,
                                error = %e,
                                "OAuth token exchange failed"
                            );
                        },
                    }
                },
                Err(e) => {
                    // Ignore callback timeout/noise after successful manual completion.
                    if pending_oauth.read().await.get(&expected_state).is_none() {
                        tracing::debug!(
                            provider = %provider_name,
                            error = %e,
                            "OAuth callback wait ended after flow was completed elsewhere"
                        );
                        return;
                    }
                    tracing::error!(
                        provider = %provider_name,
                        error = %e,
                        "OAuth callback failed"
                    );
                },
            }
        });

        Ok(serde_json::json!({
            "authUrl": auth_url,
        }))
    }

    async fn oauth_complete(&self, params: Value) -> ServiceResult {
        let parsed_callback = params
            .get("callback")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(moltis_oauth::parse_callback_input)
            .transpose()
            .map_err(ServiceError::message)?;

        let code = params
            .get("code")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| parsed_callback.as_ref().map(|parsed| parsed.code.clone()))
            .ok_or_else(|| "missing 'code' parameter".to_string())?;
        let state = params
            .get("state")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| parsed_callback.as_ref().map(|parsed| parsed.state.clone()))
            .ok_or_else(|| "missing 'state' parameter".to_string())?;
        let requested_provider = params
            .get("provider")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        let pending = {
            let mut pending_oauth = self.pending_oauth.write().await;
            let pending = pending_oauth
                .get(&state)
                .cloned()
                .ok_or_else(|| "unknown or expired OAuth state".to_string())?;

            if let Some(provider) = requested_provider.as_deref()
                && provider != pending.provider_name
            {
                return Err(ServiceError::message(format!(
                    "provider mismatch for OAuth state: expected '{}', got '{}'",
                    pending.provider_name, provider
                )));
            }

            pending_oauth
                .remove(&state)
                .ok_or_else(|| "unknown or expired OAuth state".to_string())?
        };

        let flow = OAuthFlow::new(pending.oauth_config);
        let tokens = flow
            .exchange(&code, &pending.verifier)
            .await
            .map_err(ServiceError::message)?;

        self.token_store
            .save(&pending.provider_name, &tokens)
            .map_err(ServiceError::message)?;
        set_provider_enabled_in_config(&pending.provider_name, true)?;
        self.set_provider_enabled_in_memory(&pending.provider_name, true);

        let effective = self.effective_config();
        let new_registry = self.build_registry(&effective);
        let provider_summary = new_registry.provider_summary();
        let model_count = new_registry.list_models().len();
        let mut reg = self.registry.write().await;
        *reg = new_registry;

        info!(
            provider = %pending.provider_name,
            provider_summary = %provider_summary,
            models = model_count,
            "OAuth callback complete, rebuilt provider registry"
        );

        Ok(serde_json::json!({
            "ok": true,
            "provider": pending.provider_name,
        }))
    }

    async fn remove_key(&self, params: Value) -> ServiceResult {
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?;

        if is_custom_provider(provider_name) {
            // Custom provider: remove key store entry + disable.
            self.key_store
                .remove(provider_name)
                .map_err(ServiceError::message)?;
            set_provider_enabled_in_config(provider_name, false)?;
            self.set_provider_enabled_in_memory(provider_name, false);
        } else {
            let providers = known_providers();
            let known = providers
                .iter()
                .find(|p| p.name == provider_name)
                .ok_or_else(|| format!("unknown provider: {provider_name}"))?;

            // Remove persisted API key
            if known.auth_type == AuthType::ApiKey {
                self.key_store
                    .remove(provider_name)
                    .map_err(ServiceError::message)?;
            }

            // Remove OAuth tokens
            if known.auth_type == AuthType::Oauth || provider_name == "kimi-code" {
                let _ = self.token_store.delete(provider_name);
            }

            // Persist explicit disable so auto-detected/global credentials do not
            // immediately re-enable the provider on next rebuild.
            set_provider_enabled_in_config(provider_name, false)?;
            self.set_provider_enabled_in_memory(provider_name, false);

            // Remove local-llm config
            #[cfg(feature = "local-llm")]
            if known.auth_type == AuthType::Local
                && provider_name == "local-llm"
                && let Some(config_dir) = moltis_config::config_dir()
            {
                let config_path = config_dir.join("local-llm.json");
                let _ = std::fs::remove_file(config_path);
            }
        }

        // Rebuild the provider registry without the removed provider.
        let effective = self.effective_config();
        let new_registry = self.build_registry(&effective);
        let mut reg = self.registry.write().await;
        *reg = new_registry;

        info!(
            provider = provider_name,
            "removed provider credentials and rebuilt registry"
        );

        Ok(serde_json::json!({ "ok": true }))
    }

    async fn oauth_status(&self, params: Value) -> ServiceResult {
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?;

        let has_tokens = has_oauth_tokens(provider_name, &self.token_store);
        Ok(serde_json::json!({
            "provider": provider_name,
            "authenticated": has_tokens,
        }))
    }

    async fn validate_key(&self, params: Value) -> ServiceResult {
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?;

        let api_key = params.get("apiKey").and_then(|v| v.as_str());
        let base_url = params.get("baseUrl").and_then(|v| v.as_str());
        let preferred_models = parse_models_param(&params);
        let request_id = params
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(ToString::to_string);
        let saved_config = self.key_store.load_config(provider_name);
        let saved_base_url = saved_config
            .as_ref()
            .and_then(|config| config.base_url.as_deref())
            .filter(|url| !url.trim().is_empty());
        let effective_base_url = base_url
            .filter(|url| !url.trim().is_empty())
            .or(saved_base_url);

        // Custom providers bypass known_providers() validation.
        let is_custom = is_custom_provider(provider_name);
        let provider_info = if is_custom {
            None
        } else {
            let known = known_providers();
            let info = known
                .iter()
                .find(|p| p.name == provider_name)
                .ok_or_else(|| format!("unknown provider: {provider_name}"))?;
            // API key is required for api-key providers unless the provider
            // marks the key as optional (Ollama, LM Studio).
            if info.auth_type == AuthType::ApiKey && !info.key_optional && api_key.is_none() {
                return Err("missing 'apiKey' parameter".into());
            }
            Some(KnownProvider {
                name: info.name,
                display_name: info.display_name,
                auth_type: info.auth_type,
                env_key: info.env_key,
                default_base_url: info.default_base_url,
                requires_model: info.requires_model,
                key_optional: info.key_optional,
                local_only: info.local_only,
            })
        };

        if is_custom && api_key.is_none() {
            return Err("missing 'apiKey' parameter".into());
        }
        if is_custom && effective_base_url.is_none() {
            return Err("missing 'baseUrl' parameter".into());
        }

        let selected_model = preferred_models.first().map(String::as_str);
        let validation_provider_name = validation_provider_name_for_endpoint(
            provider_name,
            provider_info.as_ref().and_then(|p| p.default_base_url),
            effective_base_url,
        );
        let _timing =
            ProviderSetupTiming::start("providers.validate_key", Some(&validation_provider_name));
        self.emit_validation_progress(
            &validation_provider_name,
            request_id.as_deref(),
            "start",
            progress_payload(serde_json::json!({
                "message": "Starting provider validation.",
            })),
        )
        .await;

        // Ollama supports native model discovery through /api/tags.
        if provider_name == "ollama" {
            let ollama_api_base = normalize_ollama_api_base_url(
                effective_base_url.or(provider_info.as_ref().and_then(|p| p.default_base_url)),
            );
            let discovered_models = match discover_ollama_models(&ollama_api_base).await {
                Ok(models) => models,
                Err(error) => {
                    let error = error.to_string();
                    self.emit_validation_progress(
                        &validation_provider_name,
                        request_id.as_deref(),
                        "error",
                        progress_payload(serde_json::json!({
                            "message": error.clone(),
                        })),
                    )
                    .await;
                    return Ok(serde_json::json!({
                        "valid": false,
                        "error": error,
                    }));
                },
            };

            if discovered_models.is_empty() {
                let error = "No Ollama models found. Install one first with `ollama pull <model>`.";
                self.emit_validation_progress(
                    &validation_provider_name,
                    request_id.as_deref(),
                    "error",
                    progress_payload(serde_json::json!({
                        "message": error,
                    })),
                )
                .await;
                return Ok(serde_json::json!({
                    "valid": false,
                    "error": error,
                }));
            }

            if let Some(requested_model) = selected_model {
                let requested_model = normalize_ollama_model_id(requested_model.trim());
                let installed = discovered_models
                    .iter()
                    .any(|installed_model| ollama_model_matches(installed_model, requested_model));
                if !installed {
                    let error = format!(
                        "Model '{requested_model}' is not installed in Ollama. Install it with `ollama pull {requested_model}`."
                    );
                    self.emit_validation_progress(
                        &validation_provider_name,
                        request_id.as_deref(),
                        "error",
                        progress_payload(serde_json::json!({
                            "message": error.clone(),
                        })),
                    )
                    .await;
                    return Ok(serde_json::json!({
                        "valid": false,
                        "error": error,
                    }));
                }
            } else {
                self.emit_validation_progress(
                    &validation_provider_name,
                    request_id.as_deref(),
                    "complete",
                    progress_payload(serde_json::json!({
                        "message": "Discovered installed Ollama models.",
                        "modelCount": discovered_models.len(),
                    })),
                )
                .await;
                return Ok(serde_json::json!({
                    "valid": true,
                    "models": ollama_models_payload(&discovered_models),
                }));
            }
        }

        // Custom OpenAI-compatible providers: discover models via /v1/models
        // when no model is specified.
        if is_custom && selected_model.is_none() {
            let api_key_str = api_key.unwrap_or_default();
            let base = effective_base_url.unwrap_or_default();
            match moltis_providers::openai::fetch_models_from_api(
                Secret::new(api_key_str.to_string()),
                base.to_string(),
            )
            .await
            {
                Ok(discovered) => {
                    let model_list: Vec<Value> = discovered
                        .iter()
                        .map(|m| {
                            serde_json::json!({
                                "id": format!("{provider_name}::{}", m.id),
                                "displayName": &m.display_name,
                                "provider": provider_name,
                            })
                        })
                        .collect();
                    self.emit_validation_progress(
                        &validation_provider_name,
                        request_id.as_deref(),
                        "complete",
                        progress_payload(serde_json::json!({
                            "message": "Discovered models from endpoint.",
                            "modelCount": model_list.len(),
                        })),
                    )
                    .await;
                    return Ok(serde_json::json!({
                        "valid": true,
                        "models": model_list,
                    }));
                },
                Err(err) => {
                    let error = format!("Failed to discover models from endpoint: {err}");
                    self.emit_validation_progress(
                        &validation_provider_name,
                        request_id.as_deref(),
                        "error",
                        progress_payload(serde_json::json!({
                            "message": error.clone(),
                        })),
                    )
                    .await;
                    return Ok(serde_json::json!({
                        "valid": false,
                        "error": error,
                    }));
                },
            }
        }

        let normalized_base_url = if provider_name == "ollama" {
            effective_base_url.map(|url| normalize_ollama_openai_base_url(Some(url)))
        } else {
            effective_base_url.map(String::from)
        };

        // Build a temporary ProvidersConfig with just this provider.
        let mut temp_config = ProvidersConfig::default();
        temp_config.providers.insert(
            validation_provider_name.clone(),
            moltis_config::schema::ProviderEntry {
                enabled: true,
                api_key: api_key.map(|k| Secret::new(k.to_string())),
                base_url: normalized_base_url,
                models: preferred_models,
                ..Default::default()
            },
        );

        // Build a temporary registry from the temp config.
        let temp_registry = self.build_registry(&temp_config);

        // Filter models for this provider.
        let models: Vec<_> = temp_registry
            .list_models()
            .iter()
            .filter(|m| {
                normalize_provider_name(&m.provider)
                    == normalize_provider_name(&validation_provider_name)
            })
            .cloned()
            .collect();

        if models.is_empty() {
            let error =
                "No models available for this provider. Check your credentials and try again.";
            self.emit_validation_progress(
                &validation_provider_name,
                request_id.as_deref(),
                "error",
                progress_payload(serde_json::json!({
                    "message": error,
                })),
            )
            .await;
            return Ok(serde_json::json!({
                "valid": false,
                "error": error,
            }));
        }

        info!(
            provider = %validation_provider_name,
            model_count = models.len(),
            "provider validation discovered candidate models"
        );

        let model_list: Vec<Value> = models
            .iter()
            .filter(|m| moltis_providers::is_chat_capable_model(&m.id))
            .map(|m| {
                let supports_tools = temp_registry.get(&m.id).is_some_and(|p| p.supports_tools());
                serde_json::json!({
                    "id": m.id,
                    "displayName": m.display_name,
                    "provider": m.provider,
                    "supportsTools": supports_tools,
                })
            })
            .collect();

        self.emit_validation_progress(
            &validation_provider_name,
            request_id.as_deref(),
            "complete",
            progress_payload(serde_json::json!({
                "message": "Validation complete.",
                "modelCount": model_list.len(),
            })),
        )
        .await;
        Ok(serde_json::json!({
            "valid": true,
            "models": model_list,
        }))
    }

    async fn save_model(&self, params: Value) -> ServiceResult {
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?;

        let model = params
            .get("model")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'model' parameter".to_string())?;

        // Validate provider exists (known or custom).
        if !is_custom_provider(provider_name) {
            let known = known_providers();
            if !known.iter().any(|p| p.name == provider_name) {
                return Err(format!("unknown provider: {provider_name}").into());
            }
        }

        // Prepend chosen model to existing saved models so it appears first.
        let mut models = vec![model.to_string()];
        if let Some(existing) = self.key_store.load_config(provider_name) {
            models.extend(existing.models);
        }

        self.key_store
            .save_config(provider_name, None, None, Some(models))
            .map_err(ServiceError::message)?;

        // Update the cross-provider priority list.
        if let Some(ref priority) = self.priority_models {
            let mut list = priority.write().await;
            let normalized = model.to_string();
            list.retain(|m| m != &normalized);
            list.insert(0, normalized);
        }

        info!(
            provider = provider_name,
            model, "saved model preference and queued async registry rebuild"
        );
        self.queue_registry_rebuild(provider_name, "save_model");
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn save_models(&self, params: Value) -> ServiceResult {
        let _timing = ProviderSetupTiming::start(
            "providers.save_models",
            params.get("provider").and_then(Value::as_str),
        );
        let provider_name = params
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'provider' parameter".to_string())?;

        let models: Vec<String> = params
            .get("models")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "missing 'models' array parameter".to_string())?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        // Validate provider exists (known or custom).
        if !is_custom_provider(provider_name) {
            let known = known_providers();
            if !known.iter().any(|p| p.name == provider_name) {
                return Err(format!("unknown provider: {provider_name}").into());
            }
        }

        self.key_store
            .save_config(provider_name, None, None, Some(models.clone()))
            .map_err(ServiceError::message)?;

        // Update the cross-provider priority list.
        if let Some(ref priority) = self.priority_models {
            let mut list = priority.write().await;
            for m in models.iter().rev() {
                list.retain(|existing| existing != m);
                list.insert(0, m.clone());
            }
        }

        info!(
            provider = provider_name,
            count = models.len(),
            models = ?models,
            "saved model preferences and queued async registry rebuild"
        );
        self.queue_registry_rebuild(provider_name, "save_models");
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn add_custom(&self, params: Value) -> ServiceResult {
        let _timing = ProviderSetupTiming::start("providers.add_custom", None);

        let base_url = params
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "missing 'baseUrl' parameter".to_string())?;

        let api_key = params
            .get("apiKey")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "missing 'apiKey' parameter".to_string())?;

        let model = params
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());

        let base_name = derive_provider_name_from_url(base_url)
            .ok_or_else(|| "could not parse endpoint URL".to_string())?;

        let existing = self.key_store.load_all_configs();
        let provider_name = existing_custom_provider_for_base_url(base_url, &existing)
            .unwrap_or_else(|| make_unique_provider_name(&base_name, &existing));
        let reused_existing_provider = existing.contains_key(&provider_name);
        let display_name = base_url_to_display_name(base_url);

        let models = model.map(|m| vec![m.to_string()]);

        self.key_store
            .save_config_with_display_name(
                &provider_name,
                Some(api_key.to_string()),
                Some(base_url.to_string()),
                models,
                Some(display_name.clone()),
            )
            .map_err(ServiceError::message)?;

        set_provider_enabled_in_config(&provider_name, true)?;
        self.set_provider_enabled_in_memory(&provider_name, true);

        // Rebuild synchronously so the just-added custom provider is immediately
        // available for model probing in the same UI flow.
        let effective = self.effective_config();
        let new_registry = self.build_registry(&effective);
        let provider_summary = new_registry.provider_summary();
        let model_count = new_registry.list_models().len();
        let mut reg = self.registry.write().await;
        *reg = new_registry;

        info!(
            provider = %provider_name,
            display_name = %display_name,
            reused = reused_existing_provider,
            provider_summary = %provider_summary,
            models = model_count,
            "saved custom OpenAI-compatible provider and rebuilt provider registry"
        );

        Ok(serde_json::json!({
            "ok": true,
            "providerName": provider_name,
            "displayName": display_name,
        }))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
