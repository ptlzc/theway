//! Client-side tool executor (issue #77): the TUI/controller serves the
//! `ToolService` RPC surface so the daemon can forward local FS/process
//! operations to this process instead of executing them itself.
//!
//! This is the controller-side counterpart of `theway_transport::ToolOps`.
//! It deliberately lives in the TUI crate (not the daemon) and uses only
//! std/tokio + the shared transport wire types.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use theway_transport::config;
use theway_transport::transport::{ToolExecStream, ToolOps};
use theway_transport::wire::{
    ToolError, WireToolDirEntry, WireToolEditRequest, WireToolEditResult, WireToolExecFrame,
    WireToolExecRequest, WireToolFindRequest, WireToolFindResult, WireToolGrepFileCount,
    WireToolGrepMatch, WireToolGrepRequest, WireToolGrepResult, WireToolListDirRequest,
    WireToolListDirResult, WireToolMemoryEntry, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolSkillSource, WireToolWriteRequest, WireToolWriteResult,
};

/// Local filesystem/process `ToolOps` implementation for the TUI controller.
#[derive(Clone, Default)]
pub struct LocalToolOps {
    /// Working directory used when a request omits `cwd`/`path`.
    pub work_dir: PathBuf,
}

impl LocalToolOps {
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.work_dir.join(p)
        }
    }
}

#[async_trait]
impl ToolOps for LocalToolOps {
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        let path = self.resolve(&request.path);
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::NotFound(format!("{}: {e}", path.display())))?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len() as u64;
        let start = request.offset.unwrap_or(1).saturating_sub(1) as usize;
        let end = match request.limit {
            Some(limit) => (start + limit as usize).min(lines.len()),
            None => lines.len(),
        };
        let truncated = end < lines.len();
        let selected = lines.get(start..end).unwrap_or(&[]).join("\n");
        Ok(WireToolReadResult {
            content: selected,
            total_lines,
            truncated,
        })
    }

    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        let path = self.resolve(&request.path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::Other(anyhow::anyhow!("create {}: {e}", parent.display()))
            })?;
        }
        let bytes = request.content.len() as u64;
        tokio::fs::write(&path, &request.content)
            .await
            .map_err(|e| ToolError::Other(anyhow::anyhow!("write {}: {e}", path.display())))?;
        Ok(WireToolWriteResult {
            bytes_written: bytes,
        })
    }

    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        let path = self.resolve(&request.path);
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::NotFound(format!("{}: {e}", path.display())))?;
        let mut text = content.clone();
        let mut replacements = 0u32;
        if request.replace_all {
            replacements = text.matches(&request.old_string).count() as u32;
            text = text.replace(&request.old_string, &request.new_string);
        } else {
            if let Some(pos) = text.find(&request.old_string) {
                text.replace_range(pos..pos + request.old_string.len(), &request.new_string);
                replacements = 1;
            }
        }
        if replacements > 0 {
            tokio::fs::write(&path, &text)
                .await
                .map_err(|e| ToolError::Other(anyhow::anyhow!("write {}: {e}", path.display())))?;
        }
        Ok(WireToolEditResult { replacements })
    }

    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command;

        let cwd = request
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.work_dir.clone());
        let timeout = request.timeout_ms.map(Duration::from_millis);
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&request.command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::Other(anyhow::anyhow!("spawn: {e}")))?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<WireToolExecFrame>();
        tokio::spawn(async move {
            let mut out_lines = BufReader::new(stdout).lines();
            let mut err_lines = BufReader::new(stderr).lines();
            let started = Instant::now();
            loop {
                tokio::select! {
                    line = out_lines.next_line() => {
                        match line {
                            Ok(Some(text)) => {
                                if tx.send(WireToolExecFrame::Output { text: format!("{text}\n") }).is_err() {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    line = err_lines.next_line() => {
                        match line {
                            Ok(Some(text)) => {
                                if tx.send(WireToolExecFrame::Output { text: format!("{text}\n") }).is_err() {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                }
            }
            let status = match timeout {
                Some(duration) => match tokio::time::timeout(duration, child.wait()).await {
                    Ok(status) => status.unwrap_or_default(),
                    Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait().await;
                        let _ = tx.send(WireToolExecFrame::Exit {
                            code: -1,
                            timed_out: true,
                            duration_ms: started.elapsed().as_millis() as u64,
                        });
                        return;
                    }
                },
                None => child.wait().await.unwrap_or_default(),
            };
            let _ = tx.send(WireToolExecFrame::Exit {
                code: status.code().unwrap_or(-1),
                timed_out: false,
                duration_ms: started.elapsed().as_millis() as u64,
            });
        });
        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|frame| (frame, rx))
        });
        Ok(Box::pin(stream))
    }

    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        let path = self.resolve(&request.path);
        let mut entries = Vec::new();
        let mut rd = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| ToolError::NotFound(format!("{}: {e}", path.display())))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ToolError::Other(e.into()))?
        {
            let file_type = entry.file_type().await.ok();
            let kind = match &file_type {
                Some(ft) if ft.is_dir() => "dir".to_string(),
                Some(ft) if ft.is_file() => "file".to_string(),
                Some(ft) if ft.is_symlink() => "symlink".to_string(),
                _ => "other".to_string(),
            };
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            entries.push(WireToolDirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
                size,
            });
            if let Some(limit) = request.limit {
                if entries.len() as u64 >= limit {
                    break;
                }
            }
        }
        Ok(WireToolListDirResult { entries })
    }

    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        let root = request
            .path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.work_dir.clone());
        let pattern = if request.case_insensitive {
            format!("(?i){}", request.pattern)
        } else {
            request.pattern.clone()
        };
        let regex = Regex::new(&pattern)
            .map_err(|e| ToolError::InvalidArgument(format!("bad regex: {e}")))?;
        let mode = request.output_mode.as_deref().unwrap_or("content");
        let mut matches = Vec::new();
        let mut files = Vec::new();
        let mut counts = Vec::new();
        let mut walked = 0usize;
        let max = request.max_results.unwrap_or(u64::MAX);

        fn walk(
            dir: &Path,
            regex: &Regex,
            glob_filter: Option<&str>,
            mode: &str,
            matches: &mut Vec<WireToolGrepMatch>,
            files: &mut Vec<String>,
            counts: &mut Vec<WireToolGrepFileCount>,
            max: u64,
            walked: &mut usize,
        ) -> Result<(), ToolError> {
            let mut rd = std::fs::read_dir(dir)
                .map_err(|e| ToolError::NotFound(format!("{}: {e}", dir.display())))?;
            while let Some(entry) = rd.next().transpose().map_err(ToolError::other)? {
                let path = entry.path();
                if path.is_dir() {
                    walk(
                        &path,
                        regex,
                        glob_filter,
                        mode,
                        matches,
                        files,
                        counts,
                        max,
                        walked,
                    )?;
                } else if path.is_file() {
                    if let Some(filter) = glob_filter {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !glob_match(filter, name) {
                            continue;
                        }
                    }
                    if *walked as u64 >= max {
                        return Ok(());
                    }
                    *walked += 1;
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        let mut count = 0u64;
                        for (idx, line) in text.lines().enumerate() {
                            if regex.is_match(line) {
                                count += 1;
                                if mode == "content" {
                                    matches.push(WireToolGrepMatch {
                                        path: path.display().to_string(),
                                        line_number: idx as u64 + 1,
                                        line: line.to_string(),
                                    });
                                }
                            }
                        }
                        if mode == "files_with_matches" && count > 0 {
                            files.push(path.display().to_string());
                        } else if mode == "count" && count > 0 {
                            counts.push(WireToolGrepFileCount {
                                path: path.display().to_string(),
                                count,
                            });
                        }
                    }
                }
            }
            Ok(())
        }

        walk(
            &root,
            &regex,
            request.glob_filter.as_deref(),
            mode,
            &mut matches,
            &mut files,
            &mut counts,
            max,
            &mut walked,
        )?;
        Ok(WireToolGrepResult {
            matches,
            files,
            counts,
        })
    }

    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        let root = request
            .path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.work_dir.clone());
        let mut paths = Vec::new();
        let limit = request.limit.unwrap_or(u64::MAX) as usize;

        fn walk(
            dir: &Path,
            pattern: &str,
            out: &mut Vec<String>,
            limit: usize,
        ) -> Result<(), ToolError> {
            if out.len() >= limit {
                return Ok(());
            }
            let mut rd = std::fs::read_dir(dir)
                .map_err(|e| ToolError::NotFound(format!("{}: {e}", dir.display())))?;
            while let Some(entry) = rd.next().transpose().map_err(ToolError::other)? {
                if out.len() >= limit {
                    return Ok(());
                }
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, pattern, out, limit)?;
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if glob_match(pattern, name) {
                        out.push(path.display().to_string());
                    }
                }
            }
            Ok(())
        }

        walk(&root, &request.pattern, &mut paths, limit)?;
        Ok(WireToolFindResult { paths })
    }

    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        let dir = config::memory_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| ToolError::Other(e.into()))?;
        let safe = sanitize_name(&request.name);
        let path = dir.join(format!("{safe}.md"));
        let mut content = String::new();
        if let Some(desc) = &request.description {
            content.push_str(&format!("---\ndescription: {desc}\n"));
            if let Some(ty) = &request.memory_type {
                content.push_str(&format!("type: {ty}\n"));
            }
            content.push_str("---\n\n");
        }
        content.push_str(&request.content);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolError::Other(e.into()))?;
        Ok(WireToolMemorySaveResult {
            name: request.name.clone(),
            path: path.display().to_string(),
        })
    }

    async fn memory_list(
        &self,
        _request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        let dir = config::memory_dir();
        let mut entries = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let name = entry.file_name().to_string_lossy().replace(".md", "");
                entries.push(WireToolMemoryEntry {
                    name,
                    description: None,
                    memory_type: None,
                    path: entry.path().display().to_string(),
                });
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(WireToolMemoryListResult { entries })
    }

    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        let dir = config::memory_dir();
        let path = dir.join(format!("{}.md", sanitize_name(&request.name)));
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::NotFound(format!("{}: {e}", path.display())))?;
        Ok(WireToolMemoryReadResult {
            name: request.name.clone(),
            content,
        })
    }

    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        let dir = config::memory_dir();
        let path = dir.join(format!("{}.md", sanitize_name(&request.name)));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(WireToolMemoryForgetResult { removed: true }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(WireToolMemoryForgetResult { removed: false })
            }
            Err(e) => Err(ToolError::Other(e.into())),
        }
    }

    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        let base = config::base_dir().join("skills");
        let (name, content) = match &request.source {
            WireToolSkillSource::Url(url) => {
                return Err(ToolError::InvalidArgument(format!(
                    "remote skill install from URL is not supported by the TUI executor yet: {url}"
                )));
            }
            WireToolSkillSource::Path(path) => {
                let path = self.resolve(path);
                let content = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| ToolError::NotFound(format!("{}: {e}", path.display())))?;
                let name = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("skill")
                    .to_string();
                (name, content)
            }
            WireToolSkillSource::Content(content) => {
                // Inline content: derive a name from the first heading if possible.
                let name = content
                    .lines()
                    .find_map(|line| {
                        line.trim()
                            .strip_prefix("# ")
                            .map(|title| title.trim().to_lowercase().replace(' ', "-"))
                    })
                    .unwrap_or_else(|| "skill".to_string());
                (name, content.clone())
            }
        };
        let target = base.join(&name).join("SKILL.md");
        let existing = target.exists();
        let size = content.len() as u64;
        if !request.confirm {
            return Ok(WireToolSkillInstallResult {
                name: name.clone(),
                target_path: target.display().to_string(),
                installed: false,
                content_hash: None,
                size,
                existing,
                warning: Some("preview only — pass confirm=true to install".to_string()),
            });
        }
        if existing && !request.overwrite {
            return Err(ToolError::InvalidArgument(format!(
                "skill {name} already exists at {} (pass overwrite=true to replace)",
                target.display()
            )));
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::Other(e.into()))?;
        }
        tokio::fs::write(&target, &content)
            .await
            .map_err(|e| ToolError::Other(e.into()))?;
        Ok(WireToolSkillInstallResult {
            name,
            target_path: target.display().to_string(),
            installed: true,
            content_hash: None,
            size,
            existing,
            warning: None,
        })
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Minimal glob-to-boolean matcher: `*` matches any sequence, `?` matches a
/// single char. Used for `grep`/`find` filename filters without adding a glob
/// dependency.
fn glob_match(pattern: &str, name: &str) -> bool {
    let regex = glob_to_regex(pattern);
    Regex::new(&regex)
        .map(|re| re.is_match(name))
        .unwrap_or(false)
}

fn glob_to_regex(pattern: &str) -> String {
    let mut out = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out.push('$');
    out
}
