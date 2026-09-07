/// Open the frame stream + authoritative snapshot on a recovered candidate
/// connection. Both steps are bounded by [`crate::ui::daemon_call`] and run
/// inside the background reconnect task — a hung daemon stalls THIS future,
/// never the event loop.
async fn open_recovered_stream(
    mut client: GrpcClient,
    reused: bool,
    notes: Vec<String>,
    session_id: &str,
    feed_limit: Option<u32>,
) -> Option<RecoveredConnection> {
    let stream = match crate::ui::daemon_call(
        "stream_events",
        client.stream_events_for_session_with_limit(Some(session_id), feed_limit),
    )
    .await
    {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!("daemon recovery stream failed: {error}");
            return None;
        }
    };
    let state = match crate::ui::daemon_call(
        "get_snapshot_for_session",
        client.get_snapshot_for_session_with_limit(session_id, feed_limit),
    )
    .await
    {
        Ok(state) => state,
        Err(error) => {
            tracing::debug!("daemon recovery snapshot failed: {error}");
            return None;
        }
    };
    Some(RecoveredConnection {
        client,
        reused,
        notes,
        stream,
        state,
    })
}

impl App {
    pub async fn run(mut self) -> Result<()> {        // Issue #79: a reused-daemon fresh attach must not load the old
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
        let session_id = self.session_id.clone();
        let result = self.event_loop(&mut terminal).await;
        leave_tui().ok();
        terminal.show_cursor().ok();
        // Issue #120: on exit print only the resume command — no banners,
        // diagnostics, or double-press hints after the TUI leaves raw mode.
        println!("theway --resume {session_id}");
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
            let feed_limit = Some(self.feed_limit());
            match crate::ui::daemon_call(
                "stream_events",
                self.client
                    .stream_events_for_session_with_limit(Some(&self.session_id), feed_limit),
            )
            .await
            {
                Ok(stream) => Some(stream),
                Err(e) => {
                    self.mark_disconnected(format!("daemon stream: {e}"));
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
                                let feed_limit = Some(self.feed_limit());
                                match crate::ui::daemon_call(
                                    "stream_events",
                                    self.client
                                        .stream_events_for_session_with_limit(Some(&new_id), feed_limit),
                                )
                                .await
                                {
                                    Ok(new_stream) => stream = Some(new_stream),
                                    Err(e) => {
                                        self.mark_disconnected(format!("daemon stream: {e}"));
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
                                // Issue #99: bounded — a hung daemon must not
                                // freeze the client in this await. The flag
                                // stays armed so the next frame retries.
                                match crate::ui::daemon_call(
                                    "get_snapshot_for_session",
                                    self.client.get_snapshot_for_session_with_limit(
                                        &self.session_id,
                                        Some(self.feed_limit()),
                                    ),
                                )
                                .await
                                {
                                    Ok(state) => {
                                        self.resync_pending = false;
                                        self.apply_snapshot(
                                            wire_status_from_session_snapshot(&state),
                                        )
                                    }
                                    Err(e) => {
                                        self.mark_disconnected(format!("resync GetSnapshot: {e}"))
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            self.mark_disconnected(format!("daemon stream: {e}"));
                            stream = None;
                        }
                        None => {
                            // Stream closed (daemon died or event loop exited).
                            self.connected = false;
                            self.busy = false;
                            self.abort_requested = false;
                            stream = None;
                            if !self.quit {
                                self.connection_line("daemon connection lost — reconnecting…");
                            }
                        }
                    }
                }
                // Background reconnect result. Polling a JoinHandle never
                // blocks: it stays Pending until the task finishes, so the
                // event loop keeps serving keys while the (potentially
                // minute-long) recover chain runs in the background.
                outcome = async {
                    match self.reconnect_handle.as_mut() {
                        Some(handle) => Some(handle.await),
                        None => None,
                    }
                }, if self.reconnect_handle.is_some() => {
                    match outcome {
                        Some(Ok((connector, recovered))) => {
                            self.connector = connector;
                            self.reconnect_handle = None;
                            if let Some(rec) = recovered {
                                let addr = rec.client.addr().to_string();
                                self.client = rec.client;
                                self.apply_snapshot(wire_status_from_session_snapshot(
                                    &rec.state,
                                ));
                                self.connected = true;
                                if rec.reused {
                                    self.connection_line(format!(
                                        "reconnected to daemon at {addr}; state synchronized"
                                    ));
                                } else {
                                    self.connection_line(format!(
                                        "daemon restarted at {addr}; restored session {}",
                                        self.session_id
                                    ));
                                }
                                for note in rec.notes {
                                    self.connection_line(note);
                                }
                                stream = Some(rec.stream);
                            }
                        }
                        Some(Err(error)) => {
                            self.reconnect_handle = None;
                            tracing::debug!("reconnect task failed: {error}");
                        }
                        None => {}
                    }
                }
                _ = reconnect.tick(), if stream.is_none() && !self.pending_fresh_attach && self.reconnect_handle.is_none() => {
                    if !self.quit {
                        // Issue #99: the recover chain runs in a background
                        // task. A hung daemon makes discover/config/snapshot
                        // take their full bounds — awaiting that here froze
                        // the UI for tens of seconds at a time.
                        let connector = self.connector.take();
                        let base_client = self.client.clone();
                        let session_id = self.session_id.clone();
                        let feed_limit = Some(self.feed_limit());
                        self.reconnect_handle = Some(tokio::spawn(async move {
                            let mut connector = connector;
                            let recovered = if let Some(c) = connector.as_mut() {
                                match c.recover(&session_id).await {
                                    Ok(connection) => {
                                        open_recovered_stream(
                                            connection.client,
                                            connection.reused,
                                            connection.notes,
                                            &session_id,
                                            feed_limit,
                                        )
                                        .await
                                    }
                                    Err(error) => {
                                        tracing::debug!(
                                            "daemon recovery attempt failed: {error}"
                                        );
                                        None
                                    }
                                }
                            } else {
                                open_recovered_stream(
                                    base_client,
                                    true,
                                    Vec::new(),
                                    &session_id,
                                    feed_limit,
                                )
                                .await
                            };
                            (connector, recovered)
                        }));
                    }
                }
                _ = async {
                    match self.mcp_error_banner.as_ref() {
                        Some(banner) => tokio::time::sleep_until(banner.until).await,
                        None => std::future::pending().await,
                    }
                }, if self.mcp_error_banner.is_some() => {
                    self.mcp_error_banner = None;
                }
                _ = tick.tick(), if self.busy || crate::ui::dag_band::has_live_runs(&self.latest.dags) => {
                    // Issue #99: the cancel RPC timed out — the daemon is not
                    // answering. Drop the frame stream so the reconnect branch
                    // takes over; the UI stays alive and recovers when the
                    // daemon responds again.
                    if self.abort_failed.swap(false, Ordering::SeqCst) {
                        self.mark_disconnected(
                            "daemon did not answer cancel within 15s — reconnecting…",
                        );
                        stream = None;
                    }
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
