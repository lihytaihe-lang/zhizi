import unittest
import numpy as np
from prefix_routing import route_distributions


class RoutingTests(unittest.TestCase):
    def tokens(self,lengths):
        x=np.zeros((len(lengths),40),np.int32)
        for row,n in enumerate(lengths):x[row,:n]=3;x[row,0]=1
        return x

    def test_mixed_rows_restore_order_and_route_exact_boundary(self):
        x=self.tokens([1,16,15,40]);before=x.copy();calls=[]
        def predict(group,role):
            calls.append((role,len(group),group.flags.c_contiguous))
            return np.repeat((np.count_nonzero(group,axis=1)+role).astype(np.float32)[:,None],7,axis=1)
        actual=route_distributions(x,7,lambda g:predict(g,0),lambda g:predict(g,100))
        np.testing.assert_array_equal(actual[:,0],[1,116,15,140]);np.testing.assert_array_equal(x,before)
        self.assertEqual(calls,[(0,2,True),(100,2,True)]);self.assertTrue(actual.flags.c_contiguous)

    def test_permutation_and_batching_do_not_change_selected_model(self):
        x=self.tokens([39,2,16,8]);p=np.array([2,0,3,1])
        base=lambda g:np.full((len(g),7),-1,np.float32)
        adapted=lambda g:np.full((len(g),7),-2,np.float32)
        a=route_distributions(x,7,base,adapted)
        b=route_distributions(x[p],7,base,adapted)
        np.testing.assert_array_equal(a[p],b)
        for i in range(len(x)):np.testing.assert_array_equal(a[i],route_distributions(x[i:i+1],7,base,adapted)[0])

    def test_callbacks_cannot_alias_original_input_or_final_output(self):
        x=self.tokens([1]);before=x.copy();returned=[]
        def callback(group):
            group[:]=0;value=np.ones((1,7),np.float32);returned.append(value);return value
        output=route_distributions(x,7,callback,lambda _:self.fail('empty adapted group called'))
        returned[0][:]=9;np.testing.assert_array_equal(output,np.ones((1,7)))
        np.testing.assert_array_equal(x,before)

    def test_only_nonempty_groups_are_called(self):
        x=self.tokens([16,40]);count=[]
        def adapted(group):count.append(len(group));return np.zeros((len(group),7),np.float32)
        route_distributions(x,7,lambda _:self.fail('empty baseline group called'),adapted)
        self.assertEqual(count,[2])

    def test_invalid_source_and_parameters_are_rejected(self):
        good=self.tokens([3]);callback=lambda g:np.zeros((len(g),7),np.float32)
        bad=[]
        x=good.copy();x[0,0]=3;bad.append(x)
        x=good.copy();x[0,1]=0;bad.append(x)
        x=good.copy();x[0,1]=-1;bad.append(x)
        x=good.copy();x[0,1]=7;bad.append(x)
        bad.extend([good.astype(np.int64),np.zeros((0,40),np.int32),np.zeros((9,40),np.int32)])
        for x in bad:
            with self.subTest(x=x.shape),self.assertRaises((ValueError,TypeError)):route_distributions(x,7,callback,callback)
        for v,n in [(True,16),(2,16),(7,True),(7,1),(7,41)]:
            with self.subTest(v=v,n=n),self.assertRaises((ValueError,TypeError)):route_distributions(good,v,callback,callback,n)

    def test_invalid_callback_outputs_are_rejected(self):
        x=self.tokens([1])
        for value in [np.zeros((1,7),np.float64),np.zeros((1,6),np.float32),np.full((1,7),np.nan,np.float32)]:
            with self.subTest(dtype=value.dtype),self.assertRaises((ValueError,TypeError)):
                route_distributions(x,7,lambda g:value,lambda g:value)


if __name__=='__main__':unittest.main()
