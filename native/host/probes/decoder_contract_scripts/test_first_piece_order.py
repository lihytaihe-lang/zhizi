import copy
from pathlib import Path
import tempfile
import unittest

import first_piece_order


class FirstPieceOrderTests(unittest.TestCase):
    def trace(self, raw='leiss'):
        return {'name': f'{raw}.keys', 'steps': [{'state': {
            'raw': raw, 'page_no': 0, 'preedit': 'lei s s'},
            'candidate_texts': ['类似是', '类似'], 'shapes': [
                {'covered_letters': 5, 'whole': True, 'single_character': False},
                {'covered_letters': 4, 'whole': False, 'single_character': False}],
            'result': {'Ok': ''}, 'observation_errors': []}]}

    def test_records_actual_coverage_and_prefix_slot_without_claiming_top1(self):
        result = first_piece_order.first_pages([self.trace()])['leiss']
        self.assertEqual(result['target_slot'], 1)
        self.assertEqual(result['target_shape'], {
            'covered_letters': 4, 'whole': False, 'single_character': False})
        self.assertEqual(result['whole_slots'], [0])
        self.assertEqual(result['preedit'], 'lei s s')
        self.assertTrue(result['target_matches'])

    def test_rejects_wrong_coverage_and_failed_observation(self):
        for mutate in [lambda t: t['steps'][0]['shapes'][1].update(covered_letters=5),
                       lambda t: t['steps'][0]['observation_errors'].append('unknown'),
                       lambda t: t['steps'][0].update(result={'Err': -1})]:
            trace = self.trace()
            mutate(trace)
            self.assertFalse(first_piece_order.first_pages([trace])['leiss']['target_matches'])

    def test_comparison_rejects_missing_cases_and_changed_coverage_but_preserves_y0_contract(self):
        pages = first_piece_order.first_pages([self.trace()])
        self.assertFalse(first_piece_order.compare_pages(pages, pages)['passed'])
        complete = {raw: copy.deepcopy(pages['leiss']) for raw in first_piece_order.CASES}
        self.assertTrue(first_piece_order.compare_pages(complete, complete)['passed'])
        changed = copy.deepcopy(complete)
        changed['leiss']['entries'].reverse()
        self.assertTrue(first_piece_order.compare_pages(complete, changed)['passed'])
        self.assertEqual(first_piece_order.compare_pages(complete, changed)['ordered_structure_differences'], ['leiss'])
        changed['leiss']['entries'][0]['covered_letters'] = 1
        self.assertFalse(first_piece_order.compare_pages(complete, changed)['passed'])
        changed = copy.deepcopy(complete)
        changed['leiss']['observation_errors'] = ['unknown']
        self.assertFalse(first_piece_order.compare_pages(complete, changed)['passed'])

    def test_same_shapes_cannot_hide_missing_or_moved_target(self):
        original = first_piece_order.first_pages([self.trace()])['leiss']
        reference = {raw: copy.deepcopy(original) for raw in first_piece_order.CASES}
        for move in [False, True]:
            trace = self.trace()
            if move:
                trace['steps'][0]['candidate_texts'].reverse()
                trace['steps'][0]['shapes'].reverse()
            else:
                trace['steps'][0]['candidate_texts'][1] = '雷斯'
            changed = copy.deepcopy(reference)
            changed['leiss'] = first_piece_order.first_pages([trace])['leiss']
            result = first_piece_order.compare_pages(reference, changed)
            self.assertFalse(result['passed'])
            self.assertEqual(result['structure_differences'], [])
            self.assertEqual(result['target_differences'], ['leiss'])

    def test_reference_missing_target_is_reported_without_inventing_a_rank(self):
        original = first_piece_order.first_pages([self.trace()])['leiss']
        tested = {raw: copy.deepcopy(original) for raw in first_piece_order.CASES}
        trace = self.trace()
        trace['steps'][0]['candidate_texts'][1] = '雷斯'
        reference = copy.deepcopy(tested)
        reference['leiss'] = first_piece_order.first_pages([trace])['leiss']
        result = first_piece_order.compare_pages(reference, tested)
        self.assertTrue(result['passed'])
        self.assertEqual(result['target_differences'], [])
        self.assertEqual(result['targets_newly_visible'], ['leiss'])

    def test_complete_abbreviation_text_difference_is_separate_from_prefix_gate(self):
        original = first_piece_order.first_pages([self.trace()])['leiss']
        reference = {raw: copy.deepcopy(original) for raw in first_piece_order.CASES}
        tested = copy.deepcopy(reference)
        tested['henkyi'].update(target_slot=None, target_shape=None, target_matches=False)
        result = first_piece_order.compare_pages(reference, tested)
        self.assertTrue(result['passed'])
        self.assertEqual(result['target_differences'], [])
        self.assertEqual(result['complete_abbreviation_target_differences'], ['henkyi'])

    def test_scripts_cover_all_requested_inputs_and_end_on_first_page(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / 'scripts'
            first_piece_order.write_scripts(destination)
            self.assertEqual({p.stem for p in destination.glob('*.keys')},
                             set(first_piece_order.CASES))
            for path in destination.glob('*.keys'):
                actions = [line for line in path.read_text().splitlines() if not line.startswith('#')]
                self.assertEqual(actions, ['clear'] + [f'type {key}' for key in path.stem])



if __name__ == '__main__':
    unittest.main()
