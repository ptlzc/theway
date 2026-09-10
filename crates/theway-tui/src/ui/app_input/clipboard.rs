//! Clipboard paste + image attachments (issue #4): Ctrl+V reads the
//! clipboard (image or text), long text pastes become atomic paste objects,
//! and clipboard images queue for the next prompt.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use theway_transport::images;

use crate::ui::App;
use crate::ui::prompt_chrome;
use crate::ui::render_utils::human_bytes;

/// Element-kind tag for paste objects (issue #4): pastes longer than
/// [`PASTE_OBJECT_MIN_LINES`] lines are inserted as atomic elements whose
/// display chip reads `[ paste N chars ]`.
const PASTE_ELEMENT_KIND: theway_ratatui_textarea::ElementKind =
    theway_ratatui_textarea::ElementKind(1);
/// Pastes longer than this many lines become paste objects.
const PASTE_OBJECT_MIN_LINES: usize = 3;

impl App {
    pub(in crate::ui) async fn paste_clipboard(&mut self) {
        match crate::clipboard_image::read_clipboard().await {
            Ok(crate::clipboard_image::ClipboardPaste::Image(image)) => {
                self.attach_clipboard_image(image);
            }
            Ok(crate::clipboard_image::ClipboardPaste::Text(text)) => {
                self.insert_paste_text(text);
            }
            Ok(crate::clipboard_image::ClipboardPaste::Empty) => {
                self.system_line("clipboard is empty");
            }
            Err(e) => {
                self.error_line(format!("clipboard paste failed: {e}"));
            }
        }
    }

    /// Insert pasted text (issue #4): pastes longer than
    /// [`PASTE_OBJECT_MIN_LINES`] lines become an atomic paste *object* whose
    /// chip renders `[ paste N chars ]` — backspace / navigation treat the
    /// whole object as one unit, and submit expands it to the full text.
    /// Shorter pastes (single lines or up to a few lines) are inserted
    /// directly as plain text.
    pub(in crate::ui) fn insert_paste_text(&mut self, text: String) {
        let chars = text.chars().count();
        let lines = text.lines().count();
        if lines > PASTE_OBJECT_MIN_LINES {
            let display = Line::from(Span::styled(
                format!("[ paste {chars} chars ]"),
                Style::default().fg(prompt_chrome::ACCENT_USER),
            ));
            self.input
                .insert_element(&text, PASTE_ELEMENT_KIND, Some(display));
        } else {
            self.input.insert_str(&text);
        }
        self.refresh_completions();
    }

    pub(in crate::ui) fn attach_clipboard_image(
        &mut self,
        image: crate::clipboard_image::ClipboardImage,
    ) {
        if self.pending_pasted_images.len() + self.pending_images.len()
            >= images::MAX_IMAGES_PER_MESSAGE
        {
            self.error_line(format!(
                "image attachment limit reached (max {} per message)",
                images::MAX_IMAGES_PER_MESSAGE
            ));
            return;
        }

        let size = human_bytes(image.encoded_bytes);
        let index = self.pending_pasted_images.len() + 1;
        let label = format!(
            "attached clipboard image #{index} ({}x{}, {size}); it will be sent with your next prompt",
            image.width, image.height
        );
        self.pending_pasted_images.push(image.image);
        self.system_line(label);
    }

    /// Image support is validated daemon-side (it knows the active model's
    /// modalities); the client always allows and surfaces the daemon's error.
    pub(in crate::ui) fn validate_pending_image_support(&mut self) -> bool {
        true
    }
}
