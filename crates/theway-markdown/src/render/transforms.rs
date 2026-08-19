impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    fn apply_transforms<'t>(&self, text: &'t str, start: usize, pretty: bool) -> Cow<'t, str> {
        if self.buffers.transforms.is_empty() {
            return Cow::Borrowed(text);
        }
        // Raw mode applies only `force` transforms (e.g. soft-break collapse).
        if !pretty && !self.buffers.transforms.iter().any(|t| t.force) {
            return Cow::Borrowed(text);
        }

        let end = start + text.len();
        let mut result = String::new();
        let mut pos = start;
        let mut applied = false;

        for transform in &self.buffers.transforms {
            if transform.range.end <= start || transform.range.start >= end {
                continue;
            }
            if !pretty && !transform.force {
                continue;
            }
            applied = true;
            // Clamp transform range to our text range
            let t_start = transform.range.start.max(start);
            let t_end = transform.range.end.min(end);

            // Copy text before transform
            if t_start > pos {
                let before = &text[(pos - start)..(t_start - start)];
                result.push_str(before);
            }

            // Apply transform
            result.push_str(&transform.to);

            pos = t_end;
        }

        if !applied {
            Cow::Borrowed(text)
        } else {
            // Copy remaining text
            if pos < end {
                result.push_str(&text[(pos - start)..]);
            }
            Cow::Owned(result)
        }
    }
}
