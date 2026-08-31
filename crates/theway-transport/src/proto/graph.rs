/// Convert one wire session-graph node into the proto message.
pub fn session_graph_node_wire(node: &WireSessionGraphNode) -> wire::SessionGraphNode {
    wire::SessionGraphNode {
        id: node.id.clone(),
        session_id: node.session_id.clone(),
        r#type: session_graph_node_type_to_proto(node.node_type),
        title: node.title.clone(),
        summary: node.summary.clone(),
        parent_node_id: node.parent_node_id.clone(),
        child_node_ids: node.child_node_ids.clone(),
        collapsed_session_id: node.collapsed_session_id.clone(),
        collapsed_at: node.collapsed_at.clone(),
        created_at: node.created_at.clone(),
        updated_at: node.updated_at.clone(),
        message_count: node.message_count,
    }
}

/// Convert a proto session-graph node into the wire form.
pub fn session_graph_node_from_proto(node: &wire::SessionGraphNode) -> WireSessionGraphNode {
    WireSessionGraphNode {
        id: node.id.clone(),
        session_id: node.session_id.clone(),
        node_type: session_graph_node_type_from_proto(node.r#type),
        title: node.title.clone(),
        summary: node.summary.clone(),
        parent_node_id: node.parent_node_id.clone(),
        child_node_ids: node.child_node_ids.clone(),
        collapsed_session_id: node.collapsed_session_id.clone(),
        collapsed_at: node.collapsed_at.clone(),
        created_at: node.created_at.clone(),
        updated_at: node.updated_at.clone(),
        message_count: node.message_count,
    }
}

pub fn collapsed_session_node_wire(node: &WireCollapsedSessionNode) -> wire::CollapsedSessionNode {
    wire::CollapsedSessionNode {
        node_id: node.node_id.clone(),
        session_id: node.session_id.clone(),
        title: node.title.clone(),
        summary: node.summary.clone(),
        message_count: node.message_count,
        collapsed_at: node.collapsed_at.clone(),
        collapsed_into_session_id: node.collapsed_into_session_id.clone(),
        collapsed_into_node_id: node.collapsed_into_node_id.clone(),
        original_session_ids: node.original_session_ids.clone(),
    }
}

pub fn collapsed_session_node_from_proto(
    node: &wire::CollapsedSessionNode,
) -> WireCollapsedSessionNode {
    WireCollapsedSessionNode {
        node_id: node.node_id.clone(),
        session_id: node.session_id.clone(),
        title: node.title.clone(),
        summary: node.summary.clone(),
        message_count: node.message_count,
        collapsed_at: node.collapsed_at.clone(),
        collapsed_into_session_id: node.collapsed_into_session_id.clone(),
        collapsed_into_node_id: node.collapsed_into_node_id.clone(),
        original_session_ids: node.original_session_ids.clone(),
    }
}

pub fn session_graph_node_stream_frame_wire(
    frame: &WireSessionGraphNodeStreamFrame,
) -> wire::SessionGraphNodeStreamFrame {
    use wire::session_graph_node_stream_frame::Payload;
    let payload = match frame {
        WireSessionGraphNodeStreamFrame::Node(node) => Payload::Node(session_graph_node_wire(node)),
        WireSessionGraphNodeStreamFrame::Block(block) => Payload::Block(feed_block(block)),
    };
    wire::SessionGraphNodeStreamFrame {
        payload: Some(payload),
    }
}

pub fn session_graph_node_stream_frame_from_proto(
    frame: &wire::SessionGraphNodeStreamFrame,
) -> Option<WireSessionGraphNodeStreamFrame> {
    use wire::session_graph_node_stream_frame::Payload;
    match frame.payload.as_ref()? {
        Payload::Node(node) => Some(WireSessionGraphNodeStreamFrame::Node(
            session_graph_node_from_proto(node),
        )),
        Payload::Block(block) => Some(WireSessionGraphNodeStreamFrame::Block(wire_feed_block(
            block,
        ))),
    }
}

pub fn collapse_session_request_from_proto(
    request: &wire::CollapseSessionRequest,
) -> WireCollapseSessionRequest {
    WireCollapseSessionRequest {
        session_id: request.session_id.clone(),
        into_session_id: request.into_session_id.clone(),
        title: request.title.clone(),
        summary: request.summary.clone(),
    }
}

pub fn collapse_session_response_to_proto(
    response: &WireCollapseSessionResponse,
) -> wire::CollapseSessionResponse {
    wire::CollapseSessionResponse {
        session_id: response.session_id.clone(),
        node: response.node.as_ref().map(session_graph_node_wire),
        collapsed: response.collapsed.as_ref().map(collapsed_session_node_wire),
    }
}

pub fn list_session_graph_node_messages_response_to_proto(
    blocks: &[WireFeedBlock],
) -> wire::ListSessionGraphNodeMessagesResponse {
    wire::ListSessionGraphNodeMessagesResponse {
        blocks: blocks.iter().map(feed_block).collect(),
    }
}

pub fn list_session_graph_node_messages_response_from_proto(
    response: &wire::ListSessionGraphNodeMessagesResponse,
) -> Vec<WireFeedBlock> {
    response.blocks.iter().map(wire_feed_block).collect()
}

fn thinking_level_to_proto(level: &str) -> i32 {
    match level {
        "off" => wire::ThinkingLevel::Off as i32,
        "minimal" => wire::ThinkingLevel::Minimal as i32,
        "low" => wire::ThinkingLevel::Low as i32,
        "medium" => wire::ThinkingLevel::Medium as i32,
        "high" => wire::ThinkingLevel::High as i32,
        "xhigh" => wire::ThinkingLevel::Xhigh as i32,
        "max" => wire::ThinkingLevel::Max as i32,
        _ => wire::ThinkingLevel::Unspecified as i32,
    }
}

fn thinking_level_from_proto(level: i32) -> String {
    match level {
        x if x == wire::ThinkingLevel::Off as i32 => "off".to_string(),
        x if x == wire::ThinkingLevel::Minimal as i32 => "minimal".to_string(),
        x if x == wire::ThinkingLevel::Low as i32 => "low".to_string(),
        x if x == wire::ThinkingLevel::Medium as i32 => "medium".to_string(),
        x if x == wire::ThinkingLevel::High as i32 => "high".to_string(),
        x if x == wire::ThinkingLevel::Xhigh as i32 => "xhigh".to_string(),
        x if x == wire::ThinkingLevel::Max as i32 => "max".to_string(),
        _ => String::new(),
    }
}

fn session_graph_node_type_to_proto(node_type: WireSessionGraphNodeType) -> i32 {
    match node_type {
        WireSessionGraphNodeType::Unspecified => wire::SessionGraphNodeType::Unspecified as i32,
        WireSessionGraphNodeType::Session => wire::SessionGraphNodeType::Session as i32,
        WireSessionGraphNodeType::Collapsed => wire::SessionGraphNodeType::Collapsed as i32,
    }
}

fn session_graph_node_type_from_proto(node_type: i32) -> WireSessionGraphNodeType {
    match node_type {
        x if x == wire::SessionGraphNodeType::Session as i32 => WireSessionGraphNodeType::Session,
        x if x == wire::SessionGraphNodeType::Collapsed as i32 => {
            WireSessionGraphNodeType::Collapsed
        }
        _ => WireSessionGraphNodeType::Unspecified,
    }
}
