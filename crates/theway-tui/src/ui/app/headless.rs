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

        // A background printer drains stream snapshots to stdout, printing only
        // rows the headless view has not emitted yet.
        let mut stream = self
            .client
            .stream_events_for_session(Some(&self.session_id))
            .await?;
        let mut printed: usize = 0;
        let printer = tokio::spawn(async move {
            while let Some(frame) = stream.next().await {
                let Ok(frame) = frame else { continue };
                if let Some(stream_frame::Payload::Snapshot(state)) = frame.payload {
                    let base = state.feed_lines_base as usize;
                    let lines = state.feed_lines;
                    if let Some(start) = headless_unprinted_start(base, lines.len(), &mut printed) {
                        for line in &lines[start..] {
                            println!("{line}");
                        }
                        let _ = std::io::stdout().flush();
                    }
                }
            }
        });

        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();
            if input.is_empty() {
                continue;
            }
            // Local-only surfaces (login needs a TTY; quit ends this process;
            // clear/help are UI concerns). Everything else goes to the daemon.
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
            let (expanded, _) = mentions::expand(input, &self.cwd).await;
            let prompt = commands::attach_skill_prompt(expanded, None);
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
        printer.abort();
        Ok(())
    }
}
