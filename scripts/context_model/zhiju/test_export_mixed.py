"""Check graph-selection boundaries without exporting or loading model weights."""
import unittest
from types import SimpleNamespace

from export_mixed import TailPrecision


def operation(kind, name, inputs=(), scope=(), shape=(8, 16, 512)):
    return SimpleNamespace(op_type=kind, name=name,
        scopes={'ScopeSource.TORCHSCRIPT_MODULE_NAME': list(scope)},
        inputs={'x': [SimpleNamespace(name=value) for value in inputs]},
        outputs=[SimpleNamespace(name=name, shape=shape)])


class PrecisionBoundaryTests(unittest.TestCase):
    def test_output_tail_follows_dependencies_without_capturing_other_head(self):
        policy = TailPrecision(stream_mode='norm')
        self.assertFalse(policy(operation('layer_norm', 'final', scope=['ln_f'])))
        self.assertTrue(policy(operation('linear', 'unrelated', scope=['head'])))
        self.assertFalse(policy(operation('linear', 'vocabulary', ['final'], ['head'])))
        self.assertFalse(policy(operation('reduce_max', 'maximum', ['vocabulary'])))
        self.assertFalse(policy(operation('sub', 'logprobs', ['vocabulary', 'maximum'])))
        self.assertTrue(policy.found_norm)
        self.assertTrue(policy.found_head)

    def test_norm_residual_keeps_stream_but_not_attention_or_mlp_activations(self):
        policy = TailPrecision(stream_mode='norm-residual')
        self.assertFalse(policy(operation('layer_norm', 'block_norm', scope=['ln1'])))
        self.assertFalse(policy(operation('add', 'embedding_sum')))
        self.assertFalse(policy(operation('add', 'residual_sum')))
        self.assertTrue(policy(operation('add', 'attention_mask', shape=(8, 8, 16, 16))))
        self.assertTrue(policy(operation('softmax', 'attention', shape=(8, 8, 16, 16))))
        self.assertTrue(policy(operation('gelu', 'mlp_activation')))
        self.assertTrue(policy(operation('linear', 'projection', ['residual_sum'])))

    def test_norm_policy_does_not_capture_residual_add(self):
        policy = TailPrecision(stream_mode='norm')
        self.assertFalse(policy(operation('layer_norm', 'norm', scope=['ln2'])))
        self.assertTrue(policy(operation('add', 'residual')))
        self.assertTrue(policy(operation('linear', 'projection', ['norm'])))

    def test_old_stable_flag_preserves_previous_operator_selection(self):
        policy = TailPrecision(stable_stream=True)
        for kind in ['layer_norm', 'add', 'softmax', 'gelu']:
            self.assertFalse(policy(operation(kind, kind)))
        self.assertTrue(policy(operation('matmul', 'projection')))

    def test_default_tail_does_not_capture_block_normalization(self):
        policy = TailPrecision()
        self.assertTrue(policy(operation('layer_norm', 'block', scope=['ln1'])))
        self.assertFalse(policy(operation('layer_norm', 'final', scope=['ln_f'])))
        self.assertEqual(policy.kept[-1]['reason'], 'output-tail')

    def test_stream_state_is_not_shared_across_conversions(self):
        first = TailPrecision(stream_mode='norm')
        first(operation('layer_norm', 'final', scope=['ln_f']))
        second = TailPrecision(stream_mode='norm')
        self.assertTrue(second(operation('linear', 'head', ['final'], ['head'])))
        self.assertFalse(second.found_norm)

    def test_invalid_or_conflicting_modes_are_rejected(self):
        with self.assertRaises(ValueError):
            TailPrecision(stream_mode='all-fp32')
        with self.assertRaises(ValueError):
            TailPrecision(stable_stream=True, stream_mode='norm')

    def test_legacy_export_metadata_tags_are_preserved(self):
        self.assertEqual(TailPrecision().precision_tag, 'fp16-tail32')
        self.assertEqual(TailPrecision(stable_stream=True).precision_tag, 'fp16-stable-stream')
        self.assertEqual(TailPrecision(stream_mode='norm').precision_tag, 'fp16-norm')


if __name__ == '__main__':
    unittest.main()
