use std::ops::{Deref, Range};
use std::sync::Arc;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation as _};
use unicode_width::UnicodeWidthStr as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordStyle {
    Small,
    WhitespaceDelimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Insert(char),
    MoveGraphemeLeft,
    MoveGraphemeRight,
    MoveWordLeft(WordStyle),
    MoveWordRight(WordStyle),
    MoveLogicalLineStart,
    MoveLogicalLineEnd,
    DeleteGraphemeBackward,
    DeleteGraphemeForward,
    DeleteWordBackward(WordStyle),
    DeleteWordForward(WordStyle),
    DeleteToLineStart,
    DeleteToLineEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditCommandCategory {
    Insert,
    Navigation,
    Delete,
    Kill,
}

impl EditCommand {
    pub(crate) fn category(self) -> EditCommandCategory {
        match self {
            Self::Insert(_) => EditCommandCategory::Insert,
            Self::MoveGraphemeLeft
            | Self::MoveGraphemeRight
            | Self::MoveWordLeft(_)
            | Self::MoveWordRight(_)
            | Self::MoveLogicalLineStart
            | Self::MoveLogicalLineEnd => EditCommandCategory::Navigation,
            Self::DeleteGraphemeBackward | Self::DeleteGraphemeForward => {
                EditCommandCategory::Delete
            }
            Self::DeleteWordBackward(_)
            | Self::DeleteWordForward(_)
            | Self::DeleteToLineStart
            | Self::DeleteToLineEnd => EditCommandCategory::Kill,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditDelta {
    pub replaced_byte_range: Range<usize>,
    pub inserted_byte_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOutcome {
    Unchanged,
    CursorOnly,
    TextOnly(EditDelta),
    TextAndCursor(EditDelta),
}

impl EditOutcome {
    fn from_changes(delta: Option<EditDelta>, cursor_changed: bool) -> Self {
        match (delta, cursor_changed) {
            (None, false) => Self::Unchanged,
            (None, true) => Self::CursorOnly,
            (Some(delta), false) => Self::TextOnly(delta),
            (Some(delta), true) => Self::TextAndCursor(delta),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostEditCursorAffinity {
    Exact,
    Right,
}

#[derive(Debug, Clone)]
pub struct EditPlan {
    replaced_byte_range: Range<usize>,
    replacement: String,
    removed_text: String,
    cursor_byte: usize,
    cursor_affinity: PostEditCursorAffinity,
    source_identity: Arc<BufferIdentity>,
    source_generation: u64,
}

impl EditPlan {
    pub fn replaced_byte_range(&self) -> Range<usize> {
        self.replaced_byte_range.clone()
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    pub fn removed_text(&self) -> &str {
        &self.removed_text
    }

    pub fn cursor_byte(&self) -> usize {
        self.cursor_byte
    }

    pub fn cursor_affinity(&self) -> PostEditCursorAffinity {
        self.cursor_affinity
    }

    pub fn into_removed_text(self) -> String {
        self.removed_text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyEditPlanError {
    StalePlan,
    InvalidRange,
    RemovedTextMismatch,
    InvalidCursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleLineViewport {
    pub visible_byte_range: Range<usize>,
    pub cursor_display_column: usize,
}

#[derive(Debug)]
struct BufferIdentity;

#[derive(Debug)]
pub struct EditBuffer {
    text: String,
    cursor_byte: usize,
    identity: Arc<BufferIdentity>,
    generation: u64,
}

impl Default for EditBuffer {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor_byte: 0,
            identity: Arc::new(BufferIdentity),
            generation: 0,
        }
    }
}

impl Clone for EditBuffer {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            cursor_byte: self.cursor_byte,
            identity: Arc::new(BufferIdentity),
            generation: 0,
        }
    }
}

impl PartialEq for EditBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.cursor_byte == other.cursor_byte
    }
}

impl Eq for EditBuffer {}

impl Deref for EditBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.text()
    }
}
