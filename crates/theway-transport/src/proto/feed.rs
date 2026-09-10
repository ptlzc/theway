fn wire_feed_block(block: &wire::FeedBlock) -> WireFeedBlock {
    use wire::feed_block::Kind;
    let Some(kind) = block.kind.as_ref() else {
        return WireFeedBlock::Plain {
            text: String::new(),
            level: feed::Level::Output,
            timestamp: None,
        };
    };
    match kind {
        Kind::User(block) => WireFeedBlock::User {
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
            attachments: block.attachments.iter().map(attachment_from_proto).collect(),
            source: block.source.as_ref().map(source_from_proto),
        },
        Kind::Context(block) => WireFeedBlock::Context {
            label: block.label.clone(),
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::Assistant(block) => WireFeedBlock::Assistant {
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::Thinking(block) => WireFeedBlock::Thinking {
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::ToolCall(block) => WireFeedBlock::ToolCall {
            name: block.name.clone(),
            args: block.args.clone(),
            metadata: block.metadata.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::Error(block) => WireFeedBlock::Error {
            message: block.message.clone(),
            code: block.code.clone(),
            recoverable: block.recoverable,
            timestamp: block.timestamp.clone(),
        },
        Kind::ToolResult(block) => WireFeedBlock::ToolResult {
            lines: block.lines.clone(),
            is_error: block.is_error,
            timestamp: block.timestamp.clone(),
        },
        Kind::Plain(block) => WireFeedBlock::Plain {
            text: block.text.clone(),
            level: level_from_str(&block.level),
            timestamp: block.timestamp.clone(),
        },
    }
}

/// `FeedAttachment` → `WireFeedAttachment` chip.
fn attachment_from_proto(attachment: &wire::FeedAttachment) -> crate::feed::WireFeedAttachment {
    crate::feed::WireFeedAttachment {
        kind: attachment.kind.clone(),
        name: attachment.name.clone(),
        detail: attachment.detail.clone(),
    }
}

/// `FeedSource` → `WireFeedSource` origin.
fn source_from_proto(source: &wire::FeedSource) -> crate::feed::WireFeedSource {
    crate::feed::WireFeedSource {
        kind: source.kind.clone(),
        label: source.label.clone(),
    }
}

/// `PlainBlock.level` serializes as snake_case variant names on the JSON surface.
fn level_from_str(level: &str) -> feed::Level {
    match level {
        "system" => feed::Level::System,
        "error" => feed::Level::Error,
        "note" => feed::Level::Note,
        "header" => feed::Level::Header,
        "qr" => feed::Level::Qr,
        _ => feed::Level::Output,
    }
}
