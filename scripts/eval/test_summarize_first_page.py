"""Synthetic-fixture tests for scripts/eval/summarize_first_page.py."""

import copy
import hashlib
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import summarize_first_page as sfp


def make_case(case_id="c1", input_text="ni", expected="ni hao", **overrides):
    case = {"id": case_id, "input": input_text, "expected": expected}
    case.update(overrides)
    return case


def make_row(case=None, **overrides):
    if case is None:
        case = make_case()
    row = {
        "input": case["input"],
        "expected": case["expected"],
        "recalledCandidates": [],
        "immediatePage": [case["expected"]],
        "displayedPage": [case["expected"]],
        "groupScoredCandidates": [],
        "compositionScoredCandidates": [],
        "shortWordScoredCandidates": [],
        "contextOutcome": "captured",
        "zhijuOutcome": "committed",
        "pageMatchesCore": True,
        "digitSelectionMatchesDisplay": True,
    }
    row.update(overrides)
    return row


def summarize_pair(cases, rows):
    return sfp.summarize(cases, {"rows": rows})


class HitCountTests(unittest.TestCase):
    def test_already_correct_top1(self):
        cases = [make_case()]
        summary = summarize_pair(cases, [make_row(cases[0])])
        self.assertEqual(summary["cases_count"], 1)
        self.assertEqual(summary["hits"]["immediate_top1"], 1)
        self.assertEqual(summary["hits"]["final_top1"], 1)
        self.assertEqual(summary["diagnostics"]["already_correct"], 1)
        self.assertEqual(summary["rows"][0]["category"], "already_correct")

    def test_accepted_alternative_counts_as_hit(self):
        cases = [make_case(expected="a", accepted=["a", "b"])]
        rows = [make_row(cases[0], immediatePage=["b"], displayedPage=["c", "b"])]
        summary = summarize_pair(cases, rows)
        self.assertEqual(summary["hits"]["immediate_top1"], 1)
        self.assertEqual(summary["hits"]["immediate_top5"], 1)
        self.assertEqual(summary["hits"]["final_top1"], 0)
        self.assertEqual(summary["hits"]["final_top5"], 1)

    def test_top5_boundary(self):
        cases = [make_case(expected="t")]
        inside = make_row(cases[0], displayedPage=["a", "b", "c", "d", "t"])
        outside = make_row(cases[0], displayedPage=["a", "b", "c", "d", "e", "t"])
        self.assertEqual(summarize_pair(cases, [inside])["hits"]["final_top5"], 1)
        self.assertEqual(summarize_pair(cases, [outside])["hits"]["final_top5"], 0)

    def test_empty_pages_have_no_hits(self):
        cases = [make_case()]
        rows = [make_row(cases[0], immediatePage=[], displayedPage=[])]
        summary = summarize_pair(cases, rows)
        self.assertEqual(summary["hits"]["immediate_top1"], 0)
        self.assertEqual(summary["hits"]["final_top5"], 0)


class DiagnosticCategoryTests(unittest.TestCase):
    def test_target_not_observed_in_pool(self):
        cases = [make_case(expected="x")]
        rows = [make_row(cases[0], immediatePage=["a"], displayedPage=["a"])]
        summary = summarize_pair(cases, rows)
        self.assertEqual(summary["rows"][0]["category"], "target_not_observed_in_pool")
        self.assertEqual(summary["observation"]["candidate_pool_observed"], 0)
        self.assertEqual(summary["observation"]["scoring_union_observed"], 0)

    def test_target_observed_but_not_scored(self):
        cases = [make_case(expected="x")]
        rows = [
            make_row(
                cases[0],
                recalledCandidates=["x"],
                immediatePage=["a"],
                displayedPage=["a"],
            )
        ]
        summary = summarize_pair(cases, rows)
        self.assertEqual(summary["observation"]["candidate_pool_observed"], 1)
        self.assertEqual(summary["observation"]["scoring_union_observed"], 0)
        self.assertEqual(summary["rows"][0]["category"], "target_observed_but_not_scored")

    def test_scored_target_not_first(self):
        cases = [make_case(expected="x")]
        rows = [
            make_row(
                cases[0],
                immediatePage=["a"],
                displayedPage=["a"],
                compositionScoredCandidates=["x"],
            )
        ]
        summary = summarize_pair(cases, rows)
        self.assertEqual(summary["rows"][0]["category"], "scored_target_not_first")
        self.assertEqual(summary["observation"]["scoring_union_observed"], 1)

    def test_scored_union_spans_three_lists(self):
        for field in (
            "groupScoredCandidates",
            "compositionScoredCandidates",
            "shortWordScoredCandidates",
        ):
            cases = [make_case(expected="x")]
            rows = [
                make_row(
                    cases[0],
                    immediatePage=["a"],
                    displayedPage=["a"],
                    **{field: ["x"]},
                )
            ]
            summary = summarize_pair(cases, rows)
            self.assertEqual(
                summary["rows"][0]["category"], "scored_target_not_first", field
            )
            self.assertEqual(summary["observation"]["scoring_union_observed"], 1, field)


class RepeatedInputTests(unittest.TestCase):
    def test_repeated_input_with_different_expected_and_id(self):
        cases = [
            make_case(case_id="c1", expected="a"),
            make_case(case_id="c2", expected="b"),
        ]
        rows = [
            make_row(cases[0], immediatePage=["a"], displayedPage=["a"]),
            make_row(cases[1], immediatePage=["b"], displayedPage=["b"]),
        ]
        summary = summarize_pair(cases, rows)
        self.assertEqual(summary["cases_count"], 2)
        self.assertEqual([row["id"] for row in summary["rows"]], ["c1", "c2"])
        self.assertEqual(summary["hits"]["final_top1"], 2)
        self.assertEqual(summary["diagnostics"]["already_correct"], 2)


class ValidationFailureTests(unittest.TestCase):
    def assert_invalid(self, cases, rows):
        with self.assertRaises(sfp.ValidationError):
            summarize_pair(cases, rows)

    def test_row_count_mismatch(self):
        cases = [make_case(), make_case(case_id="c2")]
        self.assert_invalid(cases, [make_row(cases[0])])

    def test_row_input_mismatch(self):
        cases = [make_case()]
        self.assert_invalid(cases, [make_row(cases[0], input="ta")])

    def test_row_expected_mismatch(self):
        cases = [make_case()]
        self.assert_invalid(cases, [make_row(cases[0], expected="zaijian")])

    def test_duplicate_ids_rejected(self):
        cases = [make_case(), make_case()]
        self.assert_invalid(cases, [make_row(cases[0]), make_row(cases[1])])

    def test_empty_cases_rejected(self):
        self.assert_invalid([], [])

    def test_empty_accepted_rejected(self):
        cases = [make_case(accepted=[])]
        self.assert_invalid(cases, [make_row(cases[0])])

    def test_expected_missing_from_accepted_rejected(self):
        cases = [make_case(expected="a", accepted=["b"])]
        self.assert_invalid(cases, [make_row(cases[0])])

    def test_non_lowercase_input_rejected(self):
        cases = [make_case(input_text="NI")]
        self.assert_invalid(cases, [make_row(cases[0])])

    def test_unknown_case_key_rejected(self):
        cases = [make_case(extra="nope")]
        self.assert_invalid(cases, [make_row(cases[0])])

    def test_invalid_candidate_field_type(self):
        cases = [make_case()]
        self.assert_invalid(cases, [make_row(cases[0], immediatePage="ni hao")])

    def test_invalid_candidate_element_type(self):
        cases = [make_case()]
        self.assert_invalid(
            cases, [make_row(cases[0], shortWordScoredCandidates=["ok", 3])]
        )

    def test_missing_candidate_field(self):
        cases = [make_case()]
        row = make_row(cases[0])
        del row["recalledCandidates"]
        self.assert_invalid(cases, [row])

    def test_integrity_field_not_boolean(self):
        cases = [make_case()]
        self.assert_invalid(cases, [make_row(cases[0], pageMatchesCore=1)])

    def test_missing_integrity_field(self):
        cases = [make_case()]
        row = make_row(cases[0])
        del row["digitSelectionMatchesDisplay"]
        self.assert_invalid(cases, [row])

    def test_missing_outcome_field(self):
        cases = [make_case()]
        row = make_row(cases[0])
        del row["contextOutcome"]
        self.assert_invalid(cases, [row])


class IntegrityAndPurityTests(unittest.TestCase):
    def test_integrity_false_preserved(self):
        cases = [make_case(), make_case(case_id="c2")]
        rows = [
            make_row(cases[0], pageMatchesCore=False),
            make_row(cases[1], digitSelectionMatchesDisplay=False),
        ]
        summary = summarize_pair(cases, rows)
        self.assertEqual(
            summary["integrity"]["page_matches_core"], {"true": 1, "false": 1}
        )
        self.assertEqual(
            summary["integrity"]["digit_selection_matches_display"],
            {"true": 1, "false": 1},
        )

    def test_passed_false_not_required(self):
        cases = [make_case()]
        report = {"rows": [make_row(cases[0])], "passed": False}
        summary = sfp.summarize(cases, report)
        self.assertEqual(summary["cases_count"], 1)

    def test_summarize_does_not_mutate_inputs(self):
        cases = [make_case(accepted=["ni hao", "ni"], context="ctx")]
        report = {"rows": [make_row(cases[0])], "passed": False}
        cases_copy = copy.deepcopy(cases)
        report_copy = copy.deepcopy(report)
        sfp.summarize(cases, report)
        self.assertEqual(cases, cases_copy)
        self.assertEqual(report, report_copy)

    def test_context_never_in_summary(self):
        cases = [make_case(context="SECRET-CONTEXT-VALUE")]
        rows = [make_row(cases[0])]
        text = json.dumps(summarize_pair(cases, rows), ensure_ascii=False)
        self.assertNotIn("SECRET-CONTEXT-VALUE", text)


class CliAndHashBindingTests(unittest.TestCase):
    def test_cli_writes_summary_with_bound_hashes(self):
        cases = [
            make_case(),
            make_case(case_id="c2", expected="zai", accepted=["zai", "bai"]),
        ]
        rows = [
            make_row(cases[0]),
            make_row(
                cases[1],
                immediatePage=["bai"],
                displayedPage=["x", "bai"],
                pageMatchesCore=False,
            ),
        ]
        with tempfile.TemporaryDirectory() as tmp:
            cases_path = os.path.join(tmp, "cases.json")
            report_path = os.path.join(tmp, "report.json")
            output_path = os.path.join(tmp, "summary.json")
            with open(cases_path, "w", encoding="utf-8") as handle:
                json.dump(cases, handle)
            with open(report_path, "w", encoding="utf-8") as handle:
                json.dump({"rows": rows, "passed": True}, handle)
            exit_code = sfp.main([cases_path, report_path, output_path])
            self.assertEqual(exit_code, 0)
            with open(output_path, "r", encoding="utf-8") as handle:
                summary = json.load(handle)
            with open(cases_path, "rb") as handle:
                expected_cases_sha = hashlib.sha256(handle.read()).hexdigest()
            with open(report_path, "rb") as handle:
                expected_report_sha = hashlib.sha256(handle.read()).hexdigest()
        self.assertEqual(summary["inputs"]["cases_sha256"], expected_cases_sha)
        self.assertEqual(summary["inputs"]["report_sha256"], expected_report_sha)
        self.assertEqual(summary["cases_count"], 2)
        self.assertEqual(summary["hits"]["immediate_top1"], 2)
        self.assertEqual(summary["hits"]["immediate_top5"], 2)
        self.assertEqual(summary["hits"]["final_top1"], 1)
        self.assertEqual(summary["hits"]["final_top5"], 2)
        self.assertEqual(
            summary["integrity"]["page_matches_core"], {"true": 1, "false": 1}
        )
        self.assertEqual(len(summary["limits"]), 3)

    def test_cli_rejects_malformed_cases(self):
        with tempfile.TemporaryDirectory() as tmp:
            cases_path = os.path.join(tmp, "cases.json")
            report_path = os.path.join(tmp, "report.json")
            output_path = os.path.join(tmp, "summary.json")
            with open(cases_path, "w", encoding="utf-8") as handle:
                json.dump({"rows": "not-an-array"}, handle)
            with open(report_path, "w", encoding="utf-8") as handle:
                json.dump({"rows": []}, handle)
            exit_code = sfp.main([cases_path, report_path, output_path])
            self.assertEqual(exit_code, 2)
            self.assertFalse(os.path.exists(output_path))


if __name__ == "__main__":
    unittest.main()
