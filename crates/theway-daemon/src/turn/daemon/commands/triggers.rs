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
        if turn.fut.is_some() {
            self.queue_user_prompt(display, rule.action, Vec::new())
                .await;
        } else {
            self.projection.feed.push_user(display);
            self.start_user_prompt_turn(rule.action, Vec::new(), turn);
        }
    }
}
