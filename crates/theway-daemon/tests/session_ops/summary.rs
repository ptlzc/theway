use super::*;

#[test]
fn rolling_summary_plain_material_lands_in_critical_context_with_all_headers() {
    let summary = render_rolling_summary("explored auth module, decided token refresh");

    for component in [
        "goal",
        "completed work",
        "key decisions",
        "next steps",
        "critical context",
    ] {
        assert!(
            summary.contains(&format!("{component}: ")),
            "summary must always contain the fixed component {component:?}: {summary:?}"
        );
    }
    assert!(summary.contains("critical context: explored auth module"));
}

#[test]
fn rolling_summary_carries_previous_components_forward_bounded() {
    let first = render_rolling_summary(
        "goal: ship the auth refactor\ncompleted work: auth module + tests\nkey decisions: token refresh strategy\nnext steps: login form\ncritical context: keep legacy sessions readable",
    );
    let second = render_rolling_summary(&first);

    for component in [
        "goal: ship the auth refactor",
        "completed work: auth module + tests",
        "key decisions: token refresh strategy",
        "next steps: login form",
        "critical context: keep legacy sessions readable",
    ] {
        assert!(
            second.contains(component),
            "{component:?} must survive the rolling pass: {second:?}"
        );
    }

    for line in second.lines() {
        let (name, value) = line.split_once(": ").expect("component line");
        let limit = ROLLING_SUMMARY_COMPONENTS
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .expect("known component")
            .1;
        assert!(
            value.chars().count() <= limit,
            "{name} exceeded its {limit}-char cap"
        );
    }
}

#[test]
fn rolling_summary_bounds_every_component_even_with_long_input() {
    let long = "x".repeat(5_000);
    let summary = render_rolling_summary(&long);

    assert!(summary.contains("critical context: "));
    assert!(summary.contains("… [truncated]"));
    for line in summary.lines() {
        let (name, value) = line.split_once(": ").unwrap();
        let limit = ROLLING_SUMMARY_COMPONENTS
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap()
            .1;
        assert!(
            value.chars().count() <= limit,
            "{name} exceeded its {limit}-char cap"
        );
    }
}
