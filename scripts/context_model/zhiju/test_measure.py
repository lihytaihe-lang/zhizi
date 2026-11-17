import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from measure_dist import accepted_round, select_cases


class MeasurementTests(unittest.TestCase):
    def test_overloaded_samples_are_not_accepted(self):
        self.assertTrue(accepted_round([(7.9, 7.0, 6.0), (8.0, 7.0, 6.0)]))
        self.assertFalse(accepted_round([(7.0, 8.01, 6.0)]))
        self.assertFalse(accepted_round([(7.0, 7.0, 8.01)]))
        self.assertFalse(accepted_round([(7.0, 7.0, 6.0), (8.01, 7.0, 6.0)]))

    def test_stratified_subset_is_sorted_and_has_175_per_style(self):
        styles = ("all_initials", "head_full", "random_mix", "tail_full")
        rows = [{"id": f"{sub}-{i:04}", "sub": sub} for sub in styles for i in range(200)]
        chosen = select_cases(list(reversed(rows)))
        self.assertEqual(len(chosen), 700)
        self.assertEqual([r["id"] for r in chosen], sorted(r["id"] for r in chosen))
        for sub in styles:
            ids = [r["id"] for r in chosen if r["sub"] == sub]
            self.assertEqual(ids, [f"{sub}-{i:04}" for i in range(175)])

    def test_short_subset_fails_instead_of_silently_changing_denominator(self):
        with self.assertRaises(ValueError):
            select_cases([])


if __name__ == "__main__":
    unittest.main()
