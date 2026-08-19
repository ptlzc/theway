impl GrpcClient {
    // ── tool operations (issue #75) ───────────────────────────────────

    /// Read a file in the daemon's execution environment (1-based line
    /// pagination, same window semantics as the `read` agent tool).
    pub async fn tool_read(&mut self, request: &WireToolReadRequest) -> Result<WireToolReadResult> {
        let response = self
            .tools
            .read_file(crate::tools::read_file_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_read: {e}"))?;
        Ok(crate::tools::read_file_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Write (create/overwrite) a file in the daemon's execution environment.
    pub async fn tool_write(
        &mut self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult> {
        let response = self
            .tools
            .write_file(crate::tools::write_file_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_write: {e}"))?;
        Ok(crate::tools::write_file_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Search-and-replace edit in the daemon's execution environment.
    pub async fn tool_edit(&mut self, request: &WireToolEditRequest) -> Result<WireToolEditResult> {
        let response = self
            .tools
            .edit_file(crate::tools::edit_file_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_edit: {e}"))?;
        Ok(crate::tools::edit_file_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Run a shell command line in the daemon's execution environment; the
    /// returned stream yields zero or more output chunks (interleaved
    /// stdout/stderr) followed by the terminal exit frame.
    pub async fn tool_exec(
        &mut self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecClientStream> {
        let response = self
            .tools
            .exec_command(crate::tools::exec_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_exec: {e}"))?;
        let frames = response.into_inner().map(|item| {
            item.map(|frame| crate::tools::exec_frame_from_proto(&frame))
                .map_err(|e| anyhow::anyhow!("tool_exec stream: {e}"))
        });
        Ok(Box::pin(frames))
    }

    /// Unary shape of [`tool_exec`](Self::tool_exec): collect the whole
    /// frame stream into one result (the JSON-RPC surface returns this).
    pub async fn tool_exec_collect(
        &mut self,
        request: &WireToolExecRequest,
    ) -> Result<WireToolExecResult> {
        let mut stream = self.tool_exec(request).await?;
        let mut result = WireToolExecResult {
            output: String::new(),
            code: -1,
            timed_out: false,
            duration_ms: 0,
        };
        while let Some(frame) = stream.next().await {
            match frame? {
                WireToolExecFrame::Output { text } => result.output.push_str(&text),
                WireToolExecFrame::Exit {
                    code,
                    timed_out,
                    duration_ms,
                } => {
                    result.code = code;
                    result.timed_out = timed_out;
                    result.duration_ms = duration_ms;
                }
            }
        }
        Ok(result)
    }

    /// List one directory level in the daemon's execution environment.
    pub async fn tool_list_dir(
        &mut self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult> {
        let response = self
            .tools
            .list_dir(crate::tools::list_dir_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_list_dir: {e}"))?;
        Ok(crate::tools::list_dir_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Regex content search under a root in the daemon's execution
    /// environment (gitignore-aware on the daemon side).
    pub async fn tool_grep(&mut self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult> {
        let response = self
            .tools
            .grep(crate::tools::grep_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_grep: {e}"))?;
        Ok(crate::tools::grep_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Filename-glob search under a root in the daemon's execution
    /// environment.
    pub async fn tool_find(&mut self, request: &WireToolFindRequest) -> Result<WireToolFindResult> {
        let response = self
            .tools
            .find(crate::tools::find_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_find: {e}"))?;
        Ok(crate::tools::find_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Save a cross-session memory entry on the daemon side.
    pub async fn tool_memory_save(
        &mut self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult> {
        let response = self
            .tools
            .memory_save(crate::tools::memory_save_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_save: {e}"))?;
        Ok(crate::tools::memory_save_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// List the daemon-side memory entries.
    pub async fn tool_memory_list(
        &mut self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult> {
        let response = self
            .tools
            .memory_list(crate::tools::memory_list_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_list: {e}"))?;
        Ok(crate::tools::memory_list_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Read one daemon-side memory entry's content.
    pub async fn tool_memory_read(
        &mut self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult> {
        let response = self
            .tools
            .memory_read(crate::tools::memory_read_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_read: {e}"))?;
        Ok(crate::tools::memory_read_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Forget (delete) one daemon-side memory entry.
    pub async fn tool_memory_forget(
        &mut self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult> {
        let response = self
            .tools
            .memory_forget(crate::tools::memory_forget_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_forget: {e}"))?;
        Ok(crate::tools::memory_forget_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Two-phase skill install on the daemon side: without `confirm` the
    /// call is a read-only preview and installs nothing.
    pub async fn tool_skill_install(
        &mut self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult> {
        let response = self
            .tools
            .skill_install(crate::tools::skill_install_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_skill_install: {e}"))?;
        Ok(crate::tools::skill_install_response_from_proto(
            &response.into_inner(),
        ))
    }

    // ── graph control (DAG + goal runs) ────────────────────────────────
}
