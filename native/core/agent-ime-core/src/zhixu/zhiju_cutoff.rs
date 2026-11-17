//! Select a per-span beam cutoff while retaining every numeric tie.
//! Scores are computed once by the caller; production supplies only finite scores.

use std::cmp::Ordering;

fn cmp_desc_score_asc_id(a: &(usize, f64), b: &(usize, f64)) -> Ordering {
    b.1.total_cmp(&a.1).then(a.0.cmp(&b.0))
}

pub fn keep_cutoff_ties(mut candidates: Vec<(usize, f64)>, k: usize) -> Vec<(usize, f64)> {
    if candidates.is_empty() || k == 0 {
        return Vec::new();
    }

    let cutoff_index = k.min(candidates.len()) - 1;
    candidates.select_nth_unstable_by(cutoff_index, cmp_desc_score_asc_id);
    let cutoff = candidates[cutoff_index].1;

    let mut retained: Vec<(usize, f64)> = candidates
        .into_iter()
        .filter(|&(_, score)| score >= cutoff)
        .collect();
    retained.sort_by(cmp_desc_score_asc_id);
    retained
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(candidates: &[(usize, f64)], k: usize) -> Vec<(usize, f64)> {
        if candidates.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut sorted = candidates.to_vec();
        sorted.sort_by(cmp_desc_score_asc_id);
        let cutoff = sorted[k.min(sorted.len()) - 1].1;
        sorted
            .into_iter()
            .filter(|&(_, score)| score >= cutoff)
            .collect()
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(keep_cutoff_ties(Vec::new(), 3), Vec::<(usize, f64)>::new());
        assert_eq!(keep_cutoff_ties(Vec::new(), 0), Vec::<(usize, f64)>::new());
    }

    #[test]
    fn zero_k_returns_empty() {
        assert_eq!(
            keep_cutoff_ties(vec![(1, 1.0), (2, 2.0)], 0),
            Vec::<(usize, f64)>::new()
        );
    }

    #[test]
    fn k_at_or_above_len_returns_all_sorted() {
        let input = vec![(3, -1.5), (1, 2.0), (2, 0.25)];
        let expected = vec![(1, 2.0), (2, 0.25), (3, -1.5)];
        assert_eq!(keep_cutoff_ties(input.clone(), 3), expected);
        assert_eq!(keep_cutoff_ties(input, 50), expected);
    }

    #[test]
    fn ties_at_cutoff_survive_even_beyond_k() {
        let input = vec![(9, 1.0), (5, 0.5), (4, 0.5), (7, 0.5), (2, 0.5), (1, 0.5)];
        let expected = vec![(9, 1.0), (1, 0.5), (2, 0.5), (4, 0.5), (5, 0.5), (7, 0.5)];
        assert_eq!(keep_cutoff_ties(input, 2), expected);
    }

    #[test]
    fn lower_scores_are_dropped() {
        let input = vec![(1, 3.0), (2, 2.0), (3, 1.0), (4, 0.0), (5, -4.0)];
        let expected = vec![(1, 3.0), (2, 2.0), (3, 1.0)];
        assert_eq!(keep_cutoff_ties(input, 3), expected);
    }

    #[test]
    fn signed_zero_ties_are_numeric() {
        let input = vec![(1, 1.0), (2, 0.0), (3, -0.0), (4, 0.0), (5, -0.0), (6, -1.0)];
        let out = keep_cutoff_ties(input, 2);
        assert_eq!(out, vec![(1, 1.0), (2, 0.0), (4, 0.0), (3, -0.0), (5, -0.0)]);
        let signs: Vec<bool> = out.iter().map(|&(_, s)| s.is_sign_negative()).collect();
        assert_eq!(signs, vec![false, false, false, true, true]);
    }

    #[test]
    fn cutoff_on_negative_zero_keeps_positive_zero() {
        let input = vec![(1, 0.0), (2, -0.0), (3, -1.0)];
        let out = keep_cutoff_ties(input, 2);
        assert_eq!(out, vec![(1, 0.0), (2, -0.0)]);
        assert!(!out[0].1.is_sign_negative());
        assert!(out[1].1.is_sign_negative());
    }

    #[test]
    fn negative_scores_cutoff() {
        let input = vec![(1, -1.0), (2, -2.0), (3, -2.0), (4, -3.0)];
        let expected = vec![(1, -1.0), (2, -2.0), (3, -2.0)];
        assert_eq!(keep_cutoff_ties(input, 3), expected);
    }

    #[test]
    fn matches_naive_reference() {
        let input = vec![
            (10, 0.0),
            (3, -0.0),
            (7, 5.5),
            (1, -2.25),
            (9, 5.5),
            (4, 0.0),
            (2, -0.0),
            (8, -1.0),
            (6, 3.25),
            (5, 5.5),
        ];
        for k in 0..=input.len() + 2 {
            assert_eq!(keep_cutoff_ties(input.clone(), k), naive(&input, k));
        }
        let single = vec![(42, -7.5)];
        for k in 0..3 {
            assert_eq!(keep_cutoff_ties(single.clone(), k), naive(&single, k));
        }
    }
}
