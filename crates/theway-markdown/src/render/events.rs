impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    /// Build sorted render events into the provided Vec.
    fn build_render_events_into(&self, events: &mut Vec<RenderEvent>) {
        events.clear();
        let capacity = self.buffers.highlights.len() * 2
            + self.buffers.replaces.len() * 2
            + self.buffers.table_replaces.len() * 2
            + self.buffers.mermaid_replaces.len() * 2;
        events.reserve(capacity);

        for (i, hl) in self.buffers.highlights.iter().enumerate() {
            events.push(RenderEvent {
                pos: hl.range.start,
                kind: RenderEventKind::Highlight,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: hl.range.end,
                kind: RenderEventKind::Highlight,
                index: i,
                is_end: true,
            });
        }
        for (i, r) in self.buffers.replaces.iter().enumerate() {
            events.push(RenderEvent {
                pos: r.range.start,
                kind: RenderEventKind::Replace,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: r.range.end,
                kind: RenderEventKind::Replace,
                index: i,
                is_end: true,
            });
        }
        for (i, t) in self.buffers.table_replaces.iter().enumerate() {
            events.push(RenderEvent {
                pos: t.range.start,
                kind: RenderEventKind::Table,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: t.range.end,
                kind: RenderEventKind::Table,
                index: i,
                is_end: true,
            });
        }
        for (i, m) in self.buffers.mermaid_replaces.iter().enumerate() {
            events.push(RenderEvent {
                pos: m.range.start,
                kind: RenderEventKind::Mermaid,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: m.range.end,
                kind: RenderEventKind::Mermaid,
                index: i,
                is_end: true,
            });
        }
        events.sort_unstable();
    }

    /// Build sorted render events into a new Vec.
    fn build_render_events(&self) -> Vec<RenderEvent> {
        let mut events = Vec::new();
        self.build_render_events_into(&mut events);
        events
    }
}
