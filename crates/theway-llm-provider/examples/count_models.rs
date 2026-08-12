fn main() {
    let all = theway_llm_provider::list_models();
    println!("Total: {}", all.len());
    let mut by_provider: std::collections::HashMap<String, usize> = Default::default();
    for m in &all {
        *by_provider.entry(m.provider.0.clone()).or_default() += 1;
    }
    let mut entries: Vec<_> = by_provider.into_iter().collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (p, n) in entries.iter().take(15) {
        println!("  {:30} {}", p, n);
    }
}
