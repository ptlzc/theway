impl App {
    pub async fn run(mut self) -> Result<()> {
        // Issue #79: a reused-daemon fresh attach must not load the old
        // session's nested snapshot before the fresh session is created.
        if !self.pending_fresh_attach {
            self.refresh_session_snapshot().await;
        }
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return self.run_headless().await;
        }
        enter_tui()?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let result = self.event_loop(&mut terminal).await;
        leave_tui().ok();
        terminal.show_cursor().ok();
        // Issue #47: an idle TUI must not leave the daemon's startup session
        // behind as an empty conversation.
        self.reap_empty_auto_session().await;
        result
    }

    /// Client event loop: select over terminal events + the daemon's frame
    /// stream + a reconnect timer. The stream drop flips the offline banner
    /// and arms the reconnect path; a live snapshot resyncs the whole UI.
    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(SPINNER_TICK_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut reconnect = tokio::time::interval(Duration::from_secs(1));
        // Issue #79: on a reused-daemon fresh attach, do not subscribe to the
        // daemon's current (old) session at all. The first submitted message
        // creates + selects the fresh session, and the resubscribe branch
        // below opens the stream for that new session.
        let mut stream = if self.pending_fresh_attach {
            None
        } else {
            match crate::ui::daemon_call(
                "stream_events",
                self.client
                    .stream_events_for_session(Some(&self.session_id)),
            )
            .await
            {
                Ok(stream) => Some(stream),
                Err(e) => {
                    self.connected = false;
                    self.error_line(format!("daemon stream: {e}"));
                    None
                }
            }
        };

        loop {
            terminal.draw(|f| self.render(f))?;
            if self.quit {
                break;
            }
            tokio::select! {
                biased;
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            self.handle_event(event, terminal).await?;
                            // A local session selection (`/resume`, `/session
                            // switch`) changes the session id; recreate the
                            // frame stream so live snapshots follow the new
                            // session instead of staying on the old filter.
                            if let Some(new_id) = self.resubscribe_session.take() {
                                stream = None;
                                match crate::ui::daemon_call(
                                    "stream_events",
                                    self.client.stream_events_for_session(Some(&new_id)),
                                )
                                .await
                                {
                                    Ok(new_stream) => stream = Some(new_stream),
                                    Err(e) => {
                                        self.connected = false;
                                        self.error_line(format!("daemon stream: {e}"));
                                    }
                                }
                            }
                        }
                        Some(Err(_)) => {}
                        None => self.quit = true,
                    }
                }
                frame = async { stream.as_mut()?.next().await }, if stream.is_some() => {
                    match frame {
                        Some(Ok(frame)) => {
                            self.apply_frame(frame);
                            if self.resync_pending {
                                self.resync_pending = false;
                                match self.client.get_snapshot_for_session(&self.session_id).await {
                                    Ok(state) => {
                                        self.apply_snapshot(wire_status_from_session_snapshot(
                                            &state,
                                        ))
                                    }
                                    Err(e) => self.error_line(format!("GetSnapshot: {e}")),
                                }
                            }
                        }
                        Some(Err(e)) => {
                            self.connected = false;
                            self.error_line(format!("daemon stream: {e}"));
                            stream = None;
                        }
                        None => {
                            // Stream closed (daemon died or event loop exited).
                            self.connected = false;
                            stream = None;
                            if !self.quit {
                                self.connection_line("daemon connection lost — reconnecting…");
                            }
                        }
                    }
                }
                _ = reconnect.tick(), if stream.is_none() && !self.pending_fresh_attach => {
                    if !self.quit {
                        let session_id = self.session_id.clone();
                        let attempt: Result<(GrpcClient, bool, Vec<String>)> =
                            if let Some(connector) = self.connector.as_mut() {
                                connector.recover(&session_id).await.map(|connection| {
                                    (connection.client, connection.reused, connection.notes)
                                })
                            } else {
                                Ok((self.client.clone(), true, Vec::new()))
                            };

                        match attempt {
                            Ok((mut candidate, reused, notes)) => {
                                // A recovery is announced only after both the
                                // event stream and an authoritative snapshot
                                // succeed on the candidate connection.
                                match crate::ui::daemon_call(
                                    "stream_events",
                                    candidate.stream_events_for_session(Some(&session_id)),
                                )
                                .await
                                {
                                    Ok(candidate_stream) => {
                                        match candidate.get_snapshot_for_session(&session_id).await {
                                            Ok(state) => {
                                                let addr = candidate.addr().to_string();
                                                self.client = candidate;
                                                self.apply_snapshot(
                                                    wire_status_from_session_snapshot(&state),
                                                );
                                                self.connected = true;
                                                if reused {
                                                    self.connection_line(format!(
                                                        "reconnected to daemon at {addr}; state synchronized"
                                                    ));
                                                } else {
                                                    self.connection_line(format!(
                                                        "daemon restarted at {addr}; restored session {}",
                                                        self.session_id
                                                    ));
                                                }
                                                for note in notes {
                                                    self.connection_line(note);
                                                }
                                                stream = Some(candidate_stream);
                                            }
                                            Err(error) => tracing::debug!(
                                                "daemon recovery snapshot failed: {error}"
                                            ),
                                        }
                                    }
                                    Err(error) => tracing::debug!(
                                        "daemon recovery stream failed: {error}"
                                    ),
                                }
                            }
                            Err(error) => {
                                tracing::debug!("daemon recovery attempt failed: {error}");
                            }
                        }
                    }
                }
                _ = tick.tick(), if self.busy || crate::ui::dag_band::has_live_runs(&self.latest.dags) => {
                    if self.busy {
                        self.spinner_frame = self.spinner_frame.wrapping_add(1);
                        self.cps_meter
                            .record(feed_text_bytes(&self.latest.feed_blocks));
                        self.token_meter
                            .record(feed_text_tokens(&self.latest.feed_blocks));
                        let cps = self.cps_meter.cps();
                        self.spinner.advance(cps);
                        self.spinner.tick(SPINNER_TICK_MS);
                    }
                    self.dag_tick = self.dag_tick.wrapping_add(1);
                    dag_band::record_meters(&mut self.dag_meters, &self.latest.dags);
                }
            }
        }
        Ok(())
    }

    // ── event handling ──────────────────────────────────────────────────────────────────
}
