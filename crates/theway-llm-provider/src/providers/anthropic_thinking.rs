use crate::types::{ThinkingBudgets, ThinkingLevel};

pub(super) fn budget_for(budgets: &ThinkingBudgets, level: ThinkingLevel) -> Option<u32> {
    match level {
        ThinkingLevel::Minimal => budgets.minimal,
        ThinkingLevel::Low => budgets.low,
        ThinkingLevel::Medium => budgets.medium,
        ThinkingLevel::High | ThinkingLevel::Xhigh => budgets.high,
    }
}

pub(super) fn default_budget_for(level: ThinkingLevel) -> u32 {
    match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 4096,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High => 16_384,
        ThinkingLevel::Xhigh => 32_768,
    }
}
