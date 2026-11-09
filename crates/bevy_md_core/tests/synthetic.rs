//! The synthetic document generator promises exact block counts.

use bevy_md_core::synthetic;

#[test]
fn counts_are_exact() {
    for blocks in [0, 1, 7, 200, 2_000, 20_000, 100_000] {
        let doc = synthetic::document(blocks);
        assert_eq!(doc.len(), blocks);
    }
}

#[test]
fn blocks_are_not_all_equal() {
    let doc = synthetic::document(200);
    let titles: Vec<_> = doc
        .blocks()
        .filter(|b| b.content.text.starts_with("Session notes"))
        .map(|b| b.content.text.clone())
        .collect();
    assert!(titles.len() > 2);
    assert!(titles.windows(2).all(|pair| pair[0] != pair[1]));
}
