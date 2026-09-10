// `TriggerRuleNow`: run one dynamic-trigger rule's action as a user turn.
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    async fn trigger_web_rule_now(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim();
        if id.is_empty() {
            self.error_line("trigger: missing rule id");
            return;
        }
        let Some(rule) = self.automation.services.dynamic_triggers
            .list()
            .into_iter()
            .find(|rule| rule.id == id)
        else {
            self.error_line(format!("trigger: no dynamic trigger rule with id `{id}`"));
            return;
        };
        let display = format!(
            "trigger now {}: {}",
            feed::truncate_chars(&rule.id, 18),
            wire_preview(&rule.action)
        );
        // The rule id is in scope, so the synthesised turn is recorded as the trigger's own
        // round of input (provenance marker `[trigger: <id>]`) rather than a host- or
        // user-authored one.
        let record = UserInput {
            text: rule.action.clone(),
            parts: Vec::new(),
            source: InputSource::Trigger,
            source_ref: Some(rule.id.clone()),
        };
        if turn.fut.is_some() {
            self.queue_user_prompt_with_input(display, rule.action, Vec::new(), Some(record))
                .await;
        } else {
            push_user_record_blocks(&mut self.projection.feed, Some(&record), &display);
            self.start_user_prompt_turn_with_input(rule.action, Vec::new(), Some(record), turn);
        }
    }
}
