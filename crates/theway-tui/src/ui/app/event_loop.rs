impl App {
    pub async fn run(mut self) -> Result<()> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return self.run_headless().await;
        }
        enter_tui()?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let result = self.event_loop(&mut terminal).await;
        leave_tui().ok();
        terminal.show_cursor().ok();
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
        let mut reconnect = tokio::time::interval(Duration::from_secs(1));
        let mut stream = match self.client.stream_events().await {
            Ok(stream) => Some(stream),
            Err(e) => {
                self.connected = false;
                self.error_line(format!("daemon stream: {e}"));
                None
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
                        Some(Ok(event)) => self.handle_event(event, terminal).await?,
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
                                match self.client.get_state().await {
                                    Ok(state) => self.apply_snapshot(wire_status(&state)),
                                    Err(e) => self.error_line(format!("get_state: {e}")),
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
                                self.system_line(
                                    "daemon connection lost — reconnecting…",
                                );
                            }
                        }
                    }
                }
                _ = reconnect.tick(), if stream.is_none() => {
                    if !self.quit
                        && let Ok(s) = self.client.stream_events().await
                    {
                        self.connected = true;
                        self.system_line("reconnected to daemon");
                        // Re-fetch the full state in case we missed
                        // snapshots while down.
                        match self.client.get_state().await {
                            Ok(state) => self.apply_snapshot(wire_status(&state)),
                            Err(e) => self.error_line(format!("get_state: {e}")),
                        }
                        stream = Some(s);
                    }
                }
                _ = tick.tick() => {
                    if self.busy {
                        self.spinner_frame = self.spinner_frame.wrapping_add(1);
                        self.cps_meter
                            .record(feed_text_bytes(&self.latest.feed_blocks));
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
