use super::*;
use crate::local_tool_ops::LocalToolOps;
use futures::StreamExt as _;
use std::path::Path;
use theway_transport::transport::ToolOps;
use theway_transport::wire::WireCommand;
use theway_transport::wire::{
    WireToolEditRequest, WireToolExecFrame, WireToolExecRequest, WireToolFindRequest,
    WireToolGrepRequest, WireToolListDirRequest, WireToolMemoryForgetRequest,
    WireToolMemoryListRequest, WireToolMemoryReadRequest, WireToolMemorySaveRequest,
    WireToolReadRequest, WireToolSkillInstallRequest, WireToolSkillSource, WireToolWriteRequest,
};
use tokio::sync::mpsc;

mod event_loop;
mod history;
mod interaction;
mod local_tools;
mod modals;
mod submit_slash;
