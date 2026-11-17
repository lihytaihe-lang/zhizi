import unittest
import numpy as np
from check_quantized import statistics,accepts

class QuantizedGateTests(unittest.TestCase):
    protocol={'maxTotalVariation':.02,'maxNormalizationError':.02,'maxChangedTop1LogOddsGap':float(np.log(1.1))}
    def test_identical_normalized_probabilities(self):
        x=np.log([[.7,.2,.1]]);self.assertTrue(accepts(statistics(x,x),self.protocol))
    def test_small_close_tie_flip_is_explicitly_bounded(self):
        old=np.log([[.505,.495]]);new=np.log([[.495,.505]])
        d=statistics(new,old);self.assertEqual(d['top1Agreement'],0);self.assertTrue(accepts(d,self.protocol))
    def test_confident_flip_and_mass_loss_fail(self):
        x=np.log([[.8,.2]])
        self.assertFalse(accepts(statistics(np.log([[.2,.8]]),x),self.protocol))
        self.assertFalse(accepts(statistics(x+.1,x),self.protocol))
    def test_nonfinite_rejected(self):
        self.assertFalse(accepts(statistics(np.array([[-np.inf,0.]]),np.log([[.1,.9]])),self.protocol))

if __name__=='__main__':unittest.main()
