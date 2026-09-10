// `Configure` appliers for the provisioning-owned catalogs: controller-scanned
// skills (#95), prompt templates (#96), and the builtin-skill enable set.
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    /// Issue #95: the controller owns skill discovery and provisions the full
    /// catalog here. Replace the non-builtin portion of the harness catalog
    /// with the provisioned skills (builtins keep their own resolve path below),
    /// apply the daemon-owned override state, and record the catalog in the
    /// shared slot so `/reload` and `SetSkillDirs` keep the provisioned skills.
    async fn configure_skills(
        &mut self,
        config: &WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if !config.skills.is_empty() || config.clears("skills") {
            let provisioned: Vec<theway_core::Skill> = config
                .skills
                .iter()
                .map(|skill| theway_core::Skill {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    file_path: skill.file_path.clone(),
                    content: skill.content.clone(),
                    disable_model_invocation: skill.disable_model_invocation,
                    source: if skill.source == "project" {
                        theway_core::SkillSource::Project
                    } else {
                        theway_core::SkillSource::User
                    },
                })
                .collect();
            let builtins: Vec<_> = self
                .session
                .kernel
                .harness()
                .skills()
                .into_iter()
                .filter(|skill| matches!(skill.source, theway_core::SkillSource::Builtin))
                .collect();
            let mut merged =
                crate::builtin_skills::merge_with_user_project(builtins, &provisioned);
            let overrides = crate::skill_overrides::load(&self.runtime.paths.base).await;
            crate::skill_overrides::apply(&overrides, &mut merged);
            self.session.kernel.harness().replace_skills(merged);
            *write_lock(&self.runtime.provisioned_skills) = provisioned.clone();
            if provisioned.is_empty() {
                applied.clear_fields.push("skills".into());
            } else {
                applied.skills = config.skills.clone();
            }
        }
    }

    /// Issue #96: the controller owns template discovery and provisions the
    /// full catalog here. Templates have no builtin or override layers, so the
    /// provisioned list replaces the harness catalog wholesale; the shared slot
    /// keeps `/reload` and `SetSkillDirs` from wiping it.
    fn configure_templates(&mut self, config: &WireDaemonConfig, applied: &mut WireDaemonConfig) {
        if !config.templates.is_empty() || config.clears("templates") {
            let provisioned: Vec<theway_core::PromptTemplate> = config
                .templates
                .iter()
                .map(|template| theway_core::PromptTemplate {
                    name: template.name.clone(),
                    description: if template.description.trim().is_empty() {
                        None
                    } else {
                        Some(template.description.clone())
                    },
                    content: template.content.clone(),
                    file_path: template.file_path.clone(),
                })
                .collect();
            self.session.kernel.harness().replace_templates(provisioned.clone());
            *write_lock(&self.runtime.provisioned_templates) = provisioned.clone();
            if provisioned.is_empty() {
                applied.clear_fields.push("templates".into());
            } else {
                applied.templates = config.templates.clone();
            }
        }
    }

    /// Resolve the enabled builtin-skill set and merge it with the catalog's
    /// non-builtin portion, replacing the harness catalog with the result.
    fn configure_builtin_skills(
        &mut self,
        config: &WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if !config.builtin_skills.is_empty() || config.clears("builtin_skills") {
            let requested = if config.clears("builtin_skills") && config.builtin_skills.is_empty() {
                Vec::new()
            } else {
                config.builtin_skills.clone()
            };
            let resolved = crate::builtin_skills::resolve_builtins(&[], &requested)
                .expect("an empty CLI list cannot produce a hard builtin error");
            for diagnostic in resolved.diagnostics {
                self.error_line(diagnostic);
            }
            let enabled: Vec<String> = resolved
                .skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect();
            let non_builtin: Vec<_> = self
                .session
                .kernel
                .harness()
                .skills()
                .into_iter()
                .filter(|skill| !matches!(skill.source, theway_core::SkillSource::Builtin))
                .collect();
            self.session.kernel
                .harness()
                .replace_skills(crate::builtin_skills::merge_with_user_project(
                    resolved.skills,
                    &non_builtin,
                ));
            if enabled.is_empty() {
                applied.clear_fields.push("builtin_skills".into());
            } else {
                applied.builtin_skills = enabled;
            }
        }
    }

    /// Apply the `skills_dirs` patch through the authoritative `SetSkillDirs`
    /// path and report what the daemon actually holds.
    async fn configure_skills_dirs(
        &mut self,
        config: &WireDaemonConfig,
        turn: &mut TurnState,
        applied: &mut WireDaemonConfig,
    ) {
        if !config.skills_dirs.is_empty() || config.clears("skills_dirs") {
            let dirs = if config.skills_dirs.is_empty() {
                Vec::new()
            } else {
                config.skills_dirs.clone()
            };
            self.handle_set_skill_dirs(dirs, turn).await;
            let actual = read_lock(&self.runtime.path_context).skills_dirs.clone();
            if actual.is_empty() {
                applied.clear_fields.push("skills_dirs".into());
            } else {
                applied.skills_dirs = actual;
            }
        }
    }
}
