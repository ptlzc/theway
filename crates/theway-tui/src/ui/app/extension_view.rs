impl App {
    fn render_extension_view(&self, frame: &mut ratatui::Frame) {
        if !self.extension_view {
            return;
        }
        let area = self.theme.screen.inset(frame.area());
        let width = area.width.clamp(42, 92);
        let height = area.height.clamp(10, 28);
        let rect = centered_rect(area, width, height);
        let extensions = &self.latest.extensions;
        let mut lines = vec![Line::styled(
            format!(
                "revision {}{} · {} catalog entries",
                extensions.revision,
                if extensions.reload_pending {
                    " · reload pending"
                } else {
                    ""
                },
                extensions.catalog.len()
            ),
            Style::default().fg(if extensions.reload_pending {
                Color::Yellow
            } else {
                Color::Cyan
            }),
        )];

        if extensions.catalog.is_empty() {
            lines.push(Line::styled(
                "no runtime extensions",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            lines.push(Line::raw(""));
            for entry in &extensions.catalog {
                let color = match entry.status.as_str() {
                    "effective" => Color::Green,
                    "faulted" | "rejected" => Color::Red,
                    "blocked" | "disabled" => Color::Yellow,
                    _ => Color::DarkGray,
                };
                let reason = entry
                    .reason_code
                    .as_deref()
                    .map(|reason| format!(" · {reason}"))
                    .unwrap_or_default();
                lines.push(Line::styled(
                    format!(
                        "{} {} [{} · {}{}]",
                        entry.extension_id,
                        entry.version,
                        entry.status,
                        entry.source,
                        reason
                    ),
                    Style::default().fg(color),
                ));
            }
        }

        if !extensions.diagnostics.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Diagnostics",
                Style::default().fg(Color::Cyan),
            ));
            for diagnostic in extensions.diagnostics.iter().rev().take(8).rev() {
                let redacted = if diagnostic.redacted_fields.is_empty() {
                    String::new()
                } else {
                    format!(" · redacted: {}", diagnostic.redacted_fields.join(", "))
                };
                lines.push(Line::styled(
                    format!(
                        "{} [{}] {}{}",
                        diagnostic.extension_id, diagnostic.code, diagnostic.message, redacted
                    ),
                    Style::default().fg(match diagnostic.severity.as_str() {
                        "error" => Color::Red,
                        "warning" => Color::Yellow,
                        _ => Color::DarkGray,
                    }),
                ));
            }
        }

        let rendered_contributions = extension_contribution_lines(extensions);
        if !rendered_contributions.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Client contributions",
                Style::default().fg(Color::Cyan),
            ));
            lines.extend(rendered_contributions);
        }
        if !extensions.commands.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Commands",
                Style::default().fg(Color::Cyan),
            ));
            lines.extend(extensions.commands.iter().map(|command| {
                Line::styled(
                    format!("/ext:{} — {}", command.name, command.description),
                    Style::default().fg(Color::DarkGray),
                )
            }));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Enter / Esc / q close",
            Style::default().fg(Color::DarkGray),
        ));

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Runtime extensions ")
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
            rect,
        );
    }
}

/// Render only known declarative contribution kinds. An unknown kind returns
/// no line, preserving forward compatibility without changing runtime state.
fn extension_contribution_lines(
    extensions: &theway_transport::wire::WireExtensionSnapshot,
) -> Vec<Line<'static>> {
    extensions
        .contributions
        .iter()
        .filter_map(|contribution| match contribution.kind.as_str() {
            "status_item" => Some(format!(
                "{}: {}",
                contribution
                    .payload
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("status"),
                contribution
                    .payload
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            )),
            "notification" => Some(format!(
                "{} — {}",
                contribution
                    .payload
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("notice"),
                contribution
                    .payload
                    .get("body")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            )),
            "command" => contribution
                .payload
                .get("command")
                .and_then(|command| command.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(|name| format!("command /ext:{name}")),
            "detail_panel" => contribution
                .payload
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(|title| format!("detail: {title}")),
            "form_action" => contribution
                .payload
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(|title| format!("action: {title}")),
            _ => None,
        })
        .map(|line| Line::styled(line, Style::default().fg(Color::DarkGray)))
        .collect()
}
