//! Shared fuzzy matcher built on top of `nucleo-matcher` (the Helix port of
//! the fzf algorithm).
//!
//! All sources funnel their `filter` / `filter_scored` impls through
//! [`match_indices`] so the launcher gets consistent matching semantics
//! across apps / windows / clipboard:
//!
//! * subsequence + smart-case (uppercase letters in the needle force a
//!   case-sensitive match for that letter; an all-lowercase needle is fully
//!   case-insensitive)
//! * unicode normalization (NFD) so accented haystacks match plain ASCII
//!   needles
//! * scoring rewards word-boundary, contiguous, and near-start matches —
//!   sources no longer need their own prefix/substring tier ladders
//!
//! `nucleo_matcher::Matcher` carries scratch buffers and is *not* `Send`,
//! so we keep one per OS thread via `thread_local!`. zofi's UI thread is
//! the only caller in production; tests get their own.

use std::cell::RefCell;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

thread_local! {
    static MATCHER: RefCell<Matcher> = RefCell::new(Matcher::new(Config::DEFAULT));
}

/// Score every haystack against `needle`. Returns `(orig_index, score)` for
/// items that matched, sorted by descending score (ties broken by original
/// index for determinism).
///
/// An empty needle short-circuits to `(0, 0), (1, 0), …` so callers can use
/// this as a single entry point regardless of query state.
pub fn match_indices<S: AsRef<str>>(needle: &str, haystacks: &[S]) -> Vec<(usize, i32)> {
    if needle.is_empty() {
        return (0..haystacks.len()).map(|i| (i, 0)).collect();
    }
    let pattern = Pattern::parse(needle, CaseMatching::Smart, Normalization::Smart);
    MATCHER.with(|m| {
        let mut matcher = m.borrow_mut();
        let mut buf = Vec::new();
        let mut out: Vec<(usize, i32)> = haystacks
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let h = Utf32Str::new(s.as_ref(), &mut buf);
                pattern
                    .score(h, &mut matcher)
                    .map(|score| (i, score as i32))
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_returns_all_in_original_order() {
        let h = ["alpha", "beta", "gamma"];
        let got = match_indices("", &h);
        assert_eq!(got, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn subsequence_matches() {
        // Real fuzzy: "ffx" should hit "Firefox" — the substring approach
        // we replaced could not.
        let h = ["Firefox", "Chromium", "Notes"];
        let got = match_indices("ffx", &h);
        assert!(got.iter().any(|(i, _)| *i == 0), "Firefox should match");
    }

    #[test]
    fn lowercase_needle_is_case_insensitive() {
        // Smart-case: lowercase needle ignores haystack case.
        let h = ["Firefox"];
        assert!(!match_indices("fire", &h).is_empty());
    }

    #[test]
    fn uppercase_in_needle_forces_case_sensitivity() {
        // Smart-case: uppercase letter in needle requires that exact case in
        // the haystack — protects users who deliberately type the case.
        let h = ["firefox"];
        assert!(match_indices("FIRE", &h).is_empty());
    }

    #[test]
    fn no_match_returns_empty() {
        let h = ["alpha", "beta"];
        assert!(match_indices("zzzz", &h).is_empty());
    }

    #[test]
    fn results_are_score_descending() {
        // We don't assert exact scores (that's nucleo's contract), only that
        // our wrapper sorts results highest-score first.
        let h = ["one", "one two three", "two", "three"];
        let got = match_indices("one", &h);
        let scores: Vec<i32> = got.iter().map(|(_, s)| *s).collect();
        assert!(
            scores.windows(2).all(|w| w[0] >= w[1]),
            "results must be score-descending, got {scores:?}",
        );
    }
}
