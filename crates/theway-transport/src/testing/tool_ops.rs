// ── FakeToolOps (issue #75) ──────────────────────────────────────────

/// In-memory `ToolOps` for the transport tests (issue #75). Files, directory
/// listings and memory entries live in maps; `exec_command` replays a
/// configurable frame script; grep/find operate over the stored files (regex
/// crate + a plain `*` / `?` wildcard matcher). The behavior mirrors the
/// daemon's agent tools at surface level only — transport tests verify the
/// RPC plumbing, not FS/process policy.
#[derive(Default)]
pub struct FakeToolOps {
    inner: Mutex<FakeToolInner>,
}

#[derive(Default)]
struct FakeToolInner {
    files: HashMap<String, String>,
    dirs: HashMap<String, Vec<WireToolDirEntry>>,
    /// name → (content, description, memory_type)
    memory: HashMap<String, (String, Option<String>, Option<String>)>,
    /// Frame script replayed by `exec_command` (always terminated with an
    /// exit frame before streaming).
    exec_frames: Vec<WireToolExecFrame>,
    last_exec: Option<WireToolExecRequest>,
    /// Installed skill names (`skill_install` with `confirm`).
    installed_skills: Vec<String>,
    /// Every `skill_install` request received (preview and confirm).
    skill_installs: Vec<WireToolSkillInstallRequest>,
}

impl FakeToolOps {
    /// New fake with a default exec script: one `ok\n` output chunk and a
    /// clean exit (code 0).
    pub fn new() -> Self {
        let fake = Self::default();
        fake.set_exec_frames(vec![
            WireToolExecFrame::Output {
                text: "ok\n".into(),
            },
            WireToolExecFrame::Exit {
                code: 0,
                timed_out: false,
                duration_ms: 1,
            },
        ]);
        fake
    }

    /// Seed (or overwrite) a file in the fake FS.
    pub fn put_file(&self, path: &str, content: &str) {
        self.inner
            .lock()
            .unwrap()
            .files
            .insert(path.to_string(), content.to_string());
    }

    /// Stored file content, if any.
    pub fn file_content(&self, path: &str) -> Option<String> {
        self.inner.lock().unwrap().files.get(path).cloned()
    }

    /// Seed a directory listing in the fake FS.
    pub fn seed_dir(&self, path: &str, entries: Vec<WireToolDirEntry>) {
        self.inner
            .lock()
            .unwrap()
            .dirs
            .insert(path.to_string(), entries);
    }

    /// Replace the exec frame script replayed by `exec_command`.
    pub fn set_exec_frames(&self, frames: Vec<WireToolExecFrame>) {
        self.inner.lock().unwrap().exec_frames = frames;
    }

    /// The last exec request received.
    pub fn last_exec(&self) -> Option<WireToolExecRequest> {
        self.inner.lock().unwrap().last_exec.clone()
    }

    /// Every `skill_install` request received (preview and confirm).
    pub fn skill_installs(&self) -> Vec<WireToolSkillInstallRequest> {
        self.inner.lock().unwrap().skill_installs.clone()
    }
}

/// FNV-1a 64-bit over the content bytes: the fake's `content_hash` stand-in
/// (deterministic, dependency-free — not cryptographic).
fn fake_content_hash(content: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Plain wildcard match: `*` = any run, `?` = any single char.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
        for j in 1..=t.len() {
            dp[i][j] = match p[i - 1] {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == t[j - 1],
            };
        }
    }
    dp[p.len()][t.len()]
}

/// Filename part of a slash-separated path.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Path-prefix filter: keep `candidate` when it equals `root` or sits under it.
fn under_root(candidate: &str, root: &str) -> bool {
    candidate == root || candidate.starts_with(&format!("{root}/"))
}

#[async_trait]
impl ToolOps for FakeToolOps {
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        let inner = self.inner.lock().unwrap();
        let content = inner
            .files
            .get(&request.path)
            .ok_or_else(|| ToolError::NotFound(format!("file not found: {}", request.path)))?;
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len() as u64;
        let start = request.offset.unwrap_or(1).max(1);
        let end = match request.limit {
            Some(limit) => total.min(start.saturating_add(limit).saturating_sub(1)),
            None => total,
        };
        let window: Vec<&str> = if start > total || start > end {
            Vec::new()
        } else {
            lines[(start - 1) as usize..end as usize].to_vec()
        };
        let mut joined = window.join("\n");
        // Preserve the trailing newline when the window reaches EOF (same
        // line-terminator-preserving shape as the daemon's read tool).
        if end == total && content.ends_with('\n') && !window.is_empty() {
            joined.push('\n');
        }
        Ok(WireToolReadResult {
            content: joined,
            total_lines: total,
            truncated: end < total,
        })
    }

    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .files
            .insert(request.path.clone(), request.content.clone());
        Ok(WireToolWriteResult {
            bytes_written: request.content.len() as u64,
        })
    }

    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        let mut inner = self.inner.lock().unwrap();
        if request.old_string.is_empty() {
            return Err(ToolError::InvalidArgument(
                "old_string must not be empty".into(),
            ));
        }
        let content = inner
            .files
            .get(&request.path)
            .ok_or_else(|| ToolError::NotFound(format!("file not found: {}", request.path)))?
            .clone();
        let trailing_newline = content.ends_with('\n');
        let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
        // Scope: the whole file or the requested 1-based inclusive range.
        let (scope_start, scope_end) = match (request.range_start, request.range_end) {
            (None, None) => (0usize, lines.len()),
            (Some(start), Some(end)) => {
                if start < 1 || end < start {
                    return Err(ToolError::InvalidArgument(format!(
                        "invalid range {start}..={end}"
                    )));
                }
                let start_idx = (start - 1) as usize;
                if start_idx >= lines.len() {
                    return Err(ToolError::NotFound(format!(
                        "old_string not found in {}: range starts past the end",
                        request.path
                    )));
                }
                (start_idx, lines.len().min(end as usize))
            }
            _ => {
                return Err(ToolError::InvalidArgument(
                    "range_start and range_end must be set together".into(),
                ));
            }
        };
        let scope_text = lines[scope_start..scope_end].join("\n");
        let count = scope_text.matches(&request.old_string).count();
        if count == 0 {
            return Err(ToolError::NotFound(format!(
                "old_string not found in {}",
                request.path
            )));
        }
        if count > 1 && !request.replace_all {
            return Err(ToolError::InvalidArgument(format!(
                "old_string is not unique in {} ({count} occurrences); pass replace_all or add context",
                request.path
            )));
        }
        let new_scope = if request.replace_all {
            scope_text.replace(&request.old_string, &request.new_string)
        } else {
            scope_text.replacen(&request.old_string, &request.new_string, 1)
        };
        let replacements = if request.replace_all { count } else { 1 };
        let new_lines: Vec<String> = new_scope.lines().map(str::to_string).collect();
        lines.splice(scope_start..scope_end, new_lines);
        let mut new_content = lines.join("\n");
        if trailing_newline && !new_content.is_empty() {
            new_content.push('\n');
        }
        inner.files.insert(request.path.clone(), new_content);
        Ok(WireToolEditResult {
            replacements: replacements as u32,
        })
    }

    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        let mut inner = self.inner.lock().unwrap();
        inner.last_exec = Some(request.clone());
        let mut frames = inner.exec_frames.clone();
        // The stream contract ends with an exit frame; guarantee one.
        if !matches!(frames.last(), Some(WireToolExecFrame::Exit { .. })) {
            frames.push(WireToolExecFrame::Exit {
                code: 0,
                timed_out: false,
                duration_ms: 0,
            });
        }
        Ok(Box::pin(futures::stream::iter(frames)))
    }

    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        let inner = self.inner.lock().unwrap();
        let entries = inner
            .dirs
            .get(&request.path)
            .ok_or_else(|| ToolError::NotFound(format!("directory not found: {}", request.path)))?
            .clone();
        let entries = match request.limit {
            Some(limit) => entries.into_iter().take(limit as usize).collect(),
            None => entries,
        };
        Ok(WireToolListDirResult { entries })
    }

    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        let re = regex::RegexBuilder::new(&request.pattern)
            .case_insensitive(request.case_insensitive)
            .build()
            .map_err(|e| ToolError::InvalidArgument(format!("invalid regex: {e}")))?;
        let mode = request.output_mode.as_deref().unwrap_or("content");
        if !matches!(mode, "content" | "files_with_matches" | "count") {
            return Err(ToolError::InvalidArgument(format!(
                "unknown output_mode: {mode}"
            )));
        }
        let inner = self.inner.lock().unwrap();
        let mut paths: Vec<&String> = inner.files.keys().collect();
        paths.sort();
        let max = request.max_results.map(|m| m as usize);
        let mut result = WireToolGrepResult::default();
        for path in paths {
            if let Some(root) = request.path.as_deref()
                && !under_root(path, root)
            {
                continue;
            }
            if let Some(glob) = request.glob_filter.as_deref()
                && !wildcard_match(glob, file_name(path))
            {
                continue;
            }
            let content = &inner.files[path];
            let mut matched = 0u64;
            for (index, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    matched += 1;
                    if mode == "content" {
                        result.matches.push(WireToolGrepMatch {
                            path: path.clone(),
                            line_number: (index + 1) as u64,
                            line: line.to_string(),
                        });
                    }
                }
            }
            if matched == 0 {
                continue;
            }
            if mode == "files_with_matches" {
                result.files.push(path.clone());
            }
            if mode == "count" {
                result.counts.push(WireToolGrepFileCount {
                    path: path.clone(),
                    count: matched,
                });
            }
        }
        if let Some(cap) = max {
            result.matches.truncate(cap);
            result.files.truncate(cap);
            result.counts.truncate(cap);
        }
        Ok(result)
    }

    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        let inner = self.inner.lock().unwrap();
        let mut paths: Vec<&String> = inner.files.keys().collect();
        paths.sort();
        let mut matched = Vec::new();
        for path in paths {
            if let Some(root) = request.path.as_deref()
                && !under_root(path, root)
            {
                continue;
            }
            if wildcard_match(&request.pattern, file_name(path)) {
                matched.push(path.clone());
                if let Some(limit) = request.limit
                    && matched.len() as u64 >= limit
                {
                    break;
                }
            }
        }
        Ok(WireToolFindResult { paths: matched })
    }

    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        if request.name.trim().is_empty() {
            return Err(ToolError::InvalidArgument(
                "memory name must not be empty".into(),
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.memory.insert(
            request.name.clone(),
            (
                request.content.clone(),
                request.description.clone(),
                request.memory_type.clone(),
            ),
        );
        Ok(WireToolMemorySaveResult {
            name: request.name.clone(),
            path: format!("/fake-memory/{}.md", request.name),
        })
    }

    async fn memory_list(
        &self,
        _request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        let inner = self.inner.lock().unwrap();
        let mut names: Vec<&String> = inner.memory.keys().collect();
        names.sort();
        let entries = names
            .into_iter()
            .map(|name| {
                let (_, description, memory_type) = &inner.memory[name];
                crate::wire::WireToolMemoryEntry {
                    name: name.clone(),
                    description: description.clone(),
                    memory_type: memory_type.clone(),
                    path: format!("/fake-memory/{name}.md"),
                }
            })
            .collect();
        Ok(WireToolMemoryListResult { entries })
    }

    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        let inner = self.inner.lock().unwrap();
        let (content, _, _) = inner
            .memory
            .get(&request.name)
            .ok_or_else(|| ToolError::NotFound(format!("memory not found: {}", request.name)))?;
        Ok(WireToolMemoryReadResult {
            name: request.name.clone(),
            content: content.clone(),
        })
    }

    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        let mut inner = self.inner.lock().unwrap();
        Ok(WireToolMemoryForgetResult {
            removed: inner.memory.remove(&request.name).is_some(),
        })
    }

    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        let mut inner = self.inner.lock().unwrap();
        inner.skill_installs.push(request.clone());
        let (name, size, content_hash) = match &request.source {
            WireToolSkillSource::Url(url) => {
                let base = file_name(url);
                let name = base.trim_end_matches(".md").to_string();
                (name, url.len() as u64, None)
            }
            WireToolSkillSource::Path(path) => {
                let name = file_name(path).trim_end_matches(".md").to_string();
                (name, 0, None)
            }
            WireToolSkillSource::Content(content) => (
                "inline-skill".to_string(),
                content.len() as u64,
                Some(fake_content_hash(content)),
            ),
        };
        let existing = inner.installed_skills.contains(&name);
        if request.confirm
            && (!existing || request.overwrite)
            && !inner.installed_skills.contains(&name)
        {
            inner.installed_skills.push(name.clone());
        }
        let warning = (existing && !request.overwrite)
            .then(|| format!("skill {name} already exists; pass overwrite to replace"));
        let target_path = format!("/fake-skills/{name}");
        Ok(WireToolSkillInstallResult {
            name,
            target_path,
            installed: request.confirm,
            content_hash,
            size,
            existing,
            warning,
        })
    }
}
