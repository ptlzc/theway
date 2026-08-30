fn sidebar_wire(sidebar: Option<&wire::SidebarSnapshot>) -> crate::wire::WireSidebarSnapshot {
    let sidebar = sidebar.cloned().unwrap_or_default();
    crate::wire::WireSidebarSnapshot {
        inbox_new: sidebar.inbox_new as usize,
        skills: crate::wire::WireSkillsSnapshot {
            total: sidebar
                .skills
                .as_ref()
                .map(|s| s.total as usize)
                .unwrap_or(0),
            enabled: sidebar
                .skills
                .as_ref()
                .map(|s| s.enabled as usize)
                .unwrap_or(0),
            disabled: sidebar
                .skills
                .as_ref()
                .map(|s| s.disabled as usize)
                .unwrap_or(0),
            builtin: sidebar
                .skills
                .as_ref()
                .map(|s| s.builtin as usize)
                .unwrap_or(0),
            user: sidebar
                .skills
                .as_ref()
                .map(|s| s.user as usize)
                .unwrap_or(0),
            project: sidebar
                .skills
                .as_ref()
                .map(|s| s.project as usize)
                .unwrap_or(0),
            items: sidebar
                .skills
                .as_ref()
                .map(|s| {
                    s.items
                        .iter()
                        .map(|skill| crate::wire::WireSkillSnapshot {
                            name: skill.name.clone(),
                            source: skill.source.clone(),
                            file_path: skill.file_path.clone(),
                            enabled: skill.enabled,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        triggers: crate::wire::WireTriggersSnapshot {
            total: sidebar
                .triggers
                .as_ref()
                .map(|t| t.total as usize)
                .unwrap_or(0),
            enabled: sidebar
                .triggers
                .as_ref()
                .map(|t| t.enabled as usize)
                .unwrap_or(0),
            disabled: sidebar
                .triggers
                .as_ref()
                .map(|t| t.disabled as usize)
                .unwrap_or(0),
            rules: sidebar
                .triggers
                .as_ref()
                .map(|t| {
                    t.rules
                        .iter()
                        .map(|rule| crate::wire::WireTriggerRuleSnapshot {
                            id: rule.id.clone(),
                            full_id: rule.full_id.clone(),
                            enabled: rule.enabled,
                            mode: rule.mode.clone(),
                            condition: rule.condition.clone(),
                            action: rule.action.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        cron: crate::wire::WireCronSnapshot {
            total: sidebar.cron.as_ref().map(|c| c.total as usize).unwrap_or(0),
            enabled: sidebar
                .cron
                .as_ref()
                .map(|c| c.enabled as usize)
                .unwrap_or(0),
            disabled: sidebar
                .cron
                .as_ref()
                .map(|c| c.disabled as usize)
                .unwrap_or(0),
            jobs: sidebar
                .cron
                .as_ref()
                .map(|c| {
                    c.jobs
                        .iter()
                        .map(|job| crate::wire::WireCronJobSnapshot {
                            id: job.id.clone(),
                            enabled: job.enabled,
                            schedule: job.schedule.clone(),
                            action: job.action.clone(),
                            skipped_overlap_count: job.skipped_overlap_count,
                            last_error: job.last_error.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        mcp: crate::wire::WireMcpSnapshot {
            servers: sidebar
                .mcp
                .as_ref()
                .map(|m| m.servers as usize)
                .unwrap_or(0),
            tools: sidebar.mcp.as_ref().map(|m| m.tools as usize).unwrap_or(0),
            notification_hooks: sidebar
                .mcp
                .as_ref()
                .map(|m| m.notification_hooks as usize)
                .unwrap_or(0),
            server_names: sidebar
                .mcp
                .as_ref()
                .map(|m| m.server_names.clone())
                .unwrap_or_default(),
            tool_names: sidebar
                .mcp
                .as_ref()
                .map(|m| m.tool_names.clone())
                .unwrap_or_default(),
        },
        tools: crate::wire::WireToolsSnapshot {
            total: sidebar
                .tools
                .as_ref()
                .map(|t| t.total as usize)
                .unwrap_or(0),
            names: sidebar
                .tools
                .as_ref()
                .map(|t| t.names.clone())
                .unwrap_or_default(),
        },
        hooks: sidebar.hooks.clone(),
        runtime: sidebar.runtime.clone(),
        commands: sidebar.commands.clone(),
        runtime_revision: sidebar.runtime_revision,
    }
}

pub(crate) fn sidebar_proto(
    snapshot: &crate::wire::WireSidebarSnapshot,
) -> Option<wire::SidebarSnapshot> {
    Some(wire::SidebarSnapshot {
        inbox_new: snapshot.inbox_new as u32,
        skills: Some(wire::SkillsSnapshot {
            total: snapshot.skills.total as u32,
            enabled: snapshot.skills.enabled as u32,
            disabled: snapshot.skills.disabled as u32,
            builtin: snapshot.skills.builtin as u32,
            user: snapshot.skills.user as u32,
            project: snapshot.skills.project as u32,
            items: snapshot
                .skills
                .items
                .iter()
                .map(|skill| wire::SkillSnapshot {
                    name: skill.name.clone(),
                    source: skill.source.clone(),
                    file_path: skill.file_path.clone(),
                    enabled: skill.enabled,
                })
                .collect(),
        }),
        triggers: Some(wire::TriggersSnapshot {
            total: snapshot.triggers.total as u32,
            enabled: snapshot.triggers.enabled as u32,
            disabled: snapshot.triggers.disabled as u32,
            rules: snapshot
                .triggers
                .rules
                .iter()
                .map(|rule| wire::TriggerRuleSnapshot {
                    id: rule.id.clone(),
                    full_id: rule.full_id.clone(),
                    enabled: rule.enabled,
                    mode: rule.mode.clone(),
                    condition: rule.condition.clone(),
                    action: rule.action.clone(),
                })
                .collect(),
        }),
        cron: Some(wire::CronSnapshot {
            total: snapshot.cron.total as u32,
            enabled: snapshot.cron.enabled as u32,
            disabled: snapshot.cron.disabled as u32,
            jobs: snapshot
                .cron
                .jobs
                .iter()
                .map(|job| wire::CronJobSnapshot {
                    id: job.id.clone(),
                    enabled: job.enabled,
                    schedule: job.schedule.clone(),
                    action: job.action.clone(),
                    skipped_overlap_count: job.skipped_overlap_count,
                    last_error: job.last_error.clone(),
                })
                .collect(),
        }),
        mcp: Some(wire::McpSnapshot {
            servers: snapshot.mcp.servers as u32,
            tools: snapshot.mcp.tools as u32,
            notification_hooks: snapshot.mcp.notification_hooks as u32,
            server_names: snapshot.mcp.server_names.clone(),
            tool_names: snapshot.mcp.tool_names.clone(),
        }),
        tools: Some(wire::ToolsSnapshot {
            total: snapshot.tools.total as u32,
            names: snapshot.tools.names.clone(),
        }),
        hooks: snapshot.hooks.clone(),
        runtime: snapshot.runtime.clone(),
        commands: snapshot.commands.clone(),
        runtime_revision: snapshot.runtime_revision,
    })
}
