impl App {
    // ── non-interactive fallback ──────────────────────────────────────────────────────────

    /// Line-based fallback for non-TTY stdin/stdout (e.g. `echo prompt | theway`).
    /// No fixed input box — read prompts from stdin, forward them to the daemon
    /// via `send_message`, and print the feed as snapshots arrive.
    async fn run_headless(mut self) -> Result<()> {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        // Flush startup feed (banner from the initial snapshot) first.
        for line in self.feed.plain_lines(100) {
            println!("{line}");
        }
        let _ = std::io::stdout().flush();

        // Issue #79: on a reused-daemon fresh attach, do not subscribe to the
        // daemon's current (old) session. The printer is started when the
        // first message creates + selects the fresh session (issue #46).
        let mut printer = if self.pending_fresh_attach {
            None
        } else {
            let stream = self
                .client
                .stream_events_for_session(Some(&self.session_id))
                .await?;
            Some(spawn_headless_printer(stream))
        };

        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();
            if input.is_empty() {
                continue;
            }
            // Local-only surfaces (login needs a TTY; quit ends this process;
            // clear/help/new are UI concerns). Everything else goes to the daemon.
            if input.starts_with('/') {
                match input {
                    "/quit" | "/exit" => break,
                    "/clear" => {
                        self.feed.clear();
                        continue;
                    }
                    "/help" => {
                        println!(
                            "theway client — send messages to the thewayd daemon; local commands: /login /quit /clear /new /resume /status-panel /session"
                        );
                        continue;
                    }
                    "/new" => {
                        match self
                            .client
                            .create_session_with_metadata(None, None, Default::default())
                            .await
                        {
                            Ok(summary) => {
                                let id = summary.session_id;
                                if let Err(e) = self.select_session(id.clone()).await {
                                    println!("error: select new session failed: {e}");
                                } else {
                                    println!("new session {id}");
                                }
                            }
                            Err(e) => println!("error: create session failed: {e}"),
                        }
                        continue;
                    }
                    _ if input.starts_with("/login") => {
                        let provider = input
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or("anthropic")
                            .to_string();
                        let result = crate::local_commands::prompt_for_api_key(&provider).await;
                        match result {
                            Ok(token) if token.trim().is_empty() => {
                                println!("login cancelled (empty key)");
                            }
                            Ok(token) => {
                                match theway_transport::auth::save_api_key(&provider, &token) {
                                    Ok(path) => {
                                        println!(
                                            "saved api key for `{provider}` to {}",
                                            path.display()
                                        )
                                    }
                                    Err(e) => println!("error: {e}"),
                                }
                            }
                            Err(e) => println!("error: {e}"),
                        }
                        continue;
                    }
                    _ => {}
                }
            }

            // Issue #46/#79: a reused-daemon fresh attach creates its session
            // lazily, right before the first daemon-bound input. This also
            // clears the pending flag so the message below targets the new
            // session, not the reused daemon's old conversation.
            if self.pending_fresh_attach {
                if let Err(e) = self.ensure_fresh_session().await {
                    println!("error: new session failed: {e}");
                    continue;
                }
            }

            // A fresh attach (or `/new` in this run) may not have a stream
            // yet; start one for the now-selected session.
            if printer.is_none() {
                match self
                    .client
                    .stream_events_for_session(Some(&self.session_id))
                    .await
                {
                    Ok(stream) => printer = Some(spawn_headless_printer(stream)),
                    Err(e) => {
                        println!("error: daemon stream: {e}");
                        continue;
                    }
                }
            }

            let (expanded, _) = mentions::expand(input, &self.cwd).await;
            let prompt = commands::attach_skill_prompt(expanded, None);
            self.messaged_sessions.insert(self.session_id.clone());
            match self
                .client
                .send_message_to_session(Some(&self.session_id), prompt, vec![], false)
                .await
            {
                Ok(true) => {}
                Ok(false) => println!("error: daemon rejected the message"),
                Err(e) => println!("error: {e}"),
            }
        }
        if let Some(printer) = printer {
            printer.abort();
        }
        // Issue #47: an idle run (no input lines) must not leave the daemon's
        // startup session behind as an empty conversation.
        self.reap_empty_auto_session().await;
        Ok(())
    }
}

/// Background printer for headless snapshots. Kept as a free function so both
/// the immediate stream (normal attach) and the deferred fresh-attach stream
/// use the same printing/cursor logic.
fn spawn_headless_printer<S, E>(mut stream: S) -> tokio::task::JoinHandle<()>
where
    S: futures::Stream<Item = Result<theway_grpc::StreamFrame, E>> + Send + Unpin + 'static,
    E: Send + 'static,
{
    tokio::spawn(async move {
        let mut printed: usize = 0;
        while let Some(frame) = stream.next().await {
            let Ok(frame) = frame else { continue };
            if let Some(stream_frame::Payload::Snapshot(state)) = frame.payload {
                let Some(feed) = state.feed else { continue };
                let base = feed.lines_base as usize;
                let lines = feed.lines;
                if let Some(start) = headless_unprinted_start(base, lines.len(), &mut printed) {
                    for line in &lines[start..] {
                        println!("{line}");
                    }
                    let _ = std::io::stdout().flush();
                }
            }
        }
    })
}
