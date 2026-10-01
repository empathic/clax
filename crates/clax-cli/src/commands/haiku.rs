//! `clax haiku`: one of ten haiku about Clax, chosen at random on each call.

use std::hash::{BuildHasher, Hasher};

/// The ten haiku, each three lines of five, seven and five syllables.
pub const HAIKU: [&str; 10] = [
    "A page, freshly made\na red pin lands on the chart\nthe agent wakes up",
    "Version two arrives\nthe comments you left last night\nnow marked as addressed",
    "Localhost at dusk\nseventy-four eighty hums\nno cloud overhead",
    "Drag a quiet square\ncircle what bothers you most\nand say what you see",
    "Comments pile like leaves\ncheck each one, then send them all\nthe agent rakes them",
    "The page cannot fake\nthe press of your own finger\nthe shell knows your touch",
    "Working, says the dot\na small green breath in the bar\nsomeone is building",
    "Brown ink, pink margin\ngreen for go, red for the pin\nthe palette of care",
    "Stop hook, end of turn\none more comment slipped in late\nthe work carries on",
    "Old name in the dust\nthe rename settled in now\nsame light, shorter word",
];

/// A haiku chosen at random from [`HAIKU`].
pub fn pick() -> &'static str {
    // RandomState is seeded from the OS on each construction, which is
    // random enough to choose one of ten.
    let n = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    HAIKU[(n % HAIKU.len() as u64) as usize]
}

pub fn run(cli: &crate::Cli) -> anyhow::Result<()> {
    let haiku = pick();
    super::print(cli, serde_json::json!({ "haiku": haiku }), |_| {
        haiku.to_string()
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_ten_three_line_haiku() {
        assert_eq!(HAIKU.len(), 10);
        for h in HAIKU {
            assert_eq!(h.lines().count(), 3, "{h}");
        }
    }

    #[test]
    fn pick_returns_one_of_them_and_varies() {
        let seen: std::collections::HashSet<&str> = (0..500).map(|_| pick()).collect();
        assert!(seen.iter().all(|h| HAIKU.contains(h)));
        assert!(seen.len() > 1, "500 picks gave only one haiku");
    }
}
