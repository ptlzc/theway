// `Configure` appliers for the model surface: custom-model registration, the
// provider credential overlay, catalog auto-fetch, the provider/model/base_url
// pair, and the thinking level (issues #123 / #136).
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    /// Issue #136: register controller-provisioned custom models before the
    /// model pair is resolved.
    fn configure_models(&mut self, config: &WireDaemonConfig, applied: &mut WireDaemonConfig) {
        if !config.models.is_empty() {
            crate::model_defaults::register_models(&config.models);
            applied.models = config.models.clone();
            self.runtime.model_catalog = model_catalog();
        }
    }

    /// Issue #136: seed the credential overlay from the patch. The provider
    /// falls back to the provider of the session's active model.
    fn configure_api_key(&mut self, config: &WireDaemonConfig, applied: &mut WireDaemonConfig) {
        if let Some(raw_key) = config.api_key.as_deref() {
            let provider = config
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    self.session
                        .kernel
                        .harness()
                        .agent()
                        .state()
                        .model
                        .as_ref()
                        .map(|model| model.provider.0.clone())
                });
            match provider {
                Some(provider) => {
                    let key = raw_key.trim();
                    let mut keys = self
                        .automation
                        .services
                        .configured_api_keys
                        .write()
                        .expect("configured api keys poisoned");
                    if key.is_empty() {
                        keys.remove(&provider);
                        drop(keys);
                        applied.clear_fields.push("api_key".into());
                    } else {
                        keys.insert(provider, key.to_string());
                        drop(keys);
                        applied.api_key = Some(raw_key.to_string());
                    }
                }
                None => self.error_line("configure: api_key requires a provider"),
            }
        }
    }

    /// Issue #136: `auto_fetch_models = true` fetches the provider catalog
    /// and fills an unset model from the first entry. Bounded by the fetch
    /// timeout; a failure is reported without failing the rest of the patch.
    async fn configure_auto_fetch_models(
        &mut self,
        config: &mut WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if config.auto_fetch_models == Some(true) {
            let provider = config
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    self.session
                        .kernel
                        .harness()
                        .agent()
                        .state()
                        .model
                        .as_ref()
                        .map(|model| model.provider.0.clone())
                });
            let base_url = config.base_url.clone().or_else(|| {
                self.runtime
                    .config
                    .read()
                    .expect("daemon config poisoned")
                    .base_url
                    .clone()
            });
            match (provider, base_url) {
                (Some(provider), Some(base_url)) => {
                    let configured_key = self
                        .automation
                        .services
                        .configured_api_keys
                        .read()
                        .expect("configured api keys poisoned")
                        .get(&provider)
                        .cloned();
                    // Same precedence as the request path: env > configured
                    // (`[model] api_key`) > auth.json.
                    let api_key = theway_transport::auth::AuthStore::load()
                        .unwrap_or_default()
                        .resolve_for_provider_with(&provider, configured_key.as_deref());
                    match crate::model_fetch::fetch_models(&base_url, api_key.as_deref(), &provider)
                        .await
                    {
                        Ok(models) if !models.is_empty() => {
                            let first = models[0].id.clone();
                            crate::model_defaults::register_models(&models);
                            self.runtime.model_catalog = model_catalog();
                            // Fill an unset model id from the catalog; an
                            // explicit model in the same patch wins.
                            if config.model.is_none() {
                                config.provider.get_or_insert(provider);
                                config.model = Some(first);
                            }
                            applied.auto_fetch_models = Some(true);
                            // Report the imported catalog in GetConfig so
                            // clients observe what was registered.
                            applied.models = models;
                        }
                        Ok(_) => self
                            .error_line("configure: auto-fetch models returned an empty catalog"),
                        Err(err) => {
                            self.error_line(format!("configure: auto-fetch models: {err}"));
                        }
                    }
                }
                (None, _) => {
                    self.error_line("configure: auto-fetch models requires a provider")
                }
                (_, None) => {
                    self.error_line("configure: auto-fetch models requires a base_url")
                }
            }
        }
    }

    /// Apply the provider/model/base_url selection of the patch.
    async fn configure_model_pair(
        &mut self,
        config: &WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if (config.clears("provider") && config.provider.is_none())
            || (config.clears("model") && config.model.is_none())
        {
            self.error_line("configure: the active provider/model cannot be cleared");
        } else if config.provider.is_some() != config.model.is_some() {
            self.error_line("configure: provider and model must be supplied together");
        } else if config.provider.is_some()
            || config.base_url.is_some()
            || config.clears("base_url")
        {
            let mut model = match (config.provider.as_deref(), config.model.as_deref()) {
                (Some(provider), Some(id)) => theway_llm_provider::get_model(
                    &theway_llm_provider::Provider::from(provider),
                    id,
                ),
                _ => self.session.kernel.harness().agent().state().model.clone(),
            };
            if config.clears("base_url")
                && let Some(current) = model.as_ref()
            {
                model = theway_llm_provider::get_model(&current.provider, &current.id)
                    .or_else(|| Some(current.clone()));
            }
            if let Some(model) = model.as_mut()
                && let Some(base_url) = config.base_url.as_ref()
            {
                model.base_url = base_url.clone();
            }
            match model {
                Some(model) if self.apply_model(model.clone()).await => {
                    applied.provider = Some(model.provider.0.clone());
                    applied.model = Some(model.id.clone());
                    if model.base_url.is_empty() {
                        applied.clear_fields.push("base_url".into());
                    } else {
                        applied.base_url = Some(model.base_url);
                    }
                }
                Some(_) => {}
                None => self.error_line("configure: no active or matching model to update"),
            }
        }
    }

    /// Apply the legacy `thinking` toggle and the full `thinking_level` value.
    ///
    /// The level is the persisted last-choice default: it applies the exact
    /// level, finer-grained than the legacy toggle.
    async fn configure_thinking(
        &mut self,
        config: &WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if config.thinking.is_some() || config.clears("thinking") {
            let enabled = config.thinking.unwrap_or(false);
            let level = if enabled {
                theway_core::ThinkingLevel::High
            } else {
                theway_core::ThinkingLevel::Off
            };
            match self.session.kernel.harness().set_thinking_level(level).await {
                Ok(_) if config.thinking.is_none() => applied.clear_fields.push("thinking".into()),
                Ok(_) => applied.thinking = Some(enabled),
                Err(err) => self.error_line(format!("configure thinking: {err}")),
            }
        }

        if config.thinking_level.is_some() || config.clears("thinking_level") {
            let requested = config.thinking_level.as_deref();
            let level = match requested {
                Some(raw) => match raw.parse::<theway_core::ThinkingLevel>() {
                    Ok(level) => Some(level),
                    Err(err) => {
                        self.error_line(format!(
                            "configure thinking_level: invalid level {raw:?}: {err}"
                        ));
                        None
                    }
                },
                // Clearing the level falls back to the runtime default (off).
                None => Some(theway_core::ThinkingLevel::Off),
            };
            if let Some(level) = level {
                match self.session.kernel.harness().set_thinking_level(level).await {
                    Ok(_) if requested.is_none() => {
                        applied.clear_fields.push("thinking_level".into())
                    }
                    Ok(_) => applied.thinking_level = config.thinking_level.clone(),
                    Err(err) => self.error_line(format!("configure thinking_level: {err}")),
                }
            }
        }
    }
}
