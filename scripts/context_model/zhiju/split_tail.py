import numpy as np


def _validated_float32_array(value, name):
    if not isinstance(value, np.ndarray):
        raise ValueError(f"{name} must be a numpy.ndarray")
    if value.dtype != np.float32:
        raise ValueError(f"{name} must have dtype float32")
    if not bool(np.all(np.isfinite(value))):
        raise ValueError(f"{name} must contain only finite values")
    return np.array(value, dtype=np.float32, order="C", copy=True)


class FP32Tail:
    def __init__(self, gamma, beta, weight, epsilon=1e-5):
        self._gamma = _validated_float32_array(gamma, "gamma")
        self._beta = _validated_float32_array(beta, "beta")
        self._weight = _validated_float32_array(weight, "weight")

        if self._gamma.ndim != 1 or self._beta.ndim != 1 or self._weight.ndim != 2:
            raise ValueError("gamma/beta must be 1-D and weight must be 2-D")
        if self._gamma.shape[0] != self._beta.shape[0] or self._weight.shape[1] != self._gamma.shape[0]:
            raise ValueError("inconsistent parameter shapes")

        d = int(self._gamma.shape[0])
        v = int(self._weight.shape[0])
        if d <= 0 or v < 3:
            raise ValueError("require D>0 and V>=3")

        if isinstance(epsilon, (bool, np.bool_)):
            raise ValueError("epsilon must not be bool")
        if not isinstance(epsilon, (int, float, np.integer, np.floating)):
            raise ValueError("epsilon must be a real number")
        epsilon_f = float(epsilon)
        if not bool(np.isfinite(epsilon_f)) or epsilon_f <= 0.0:
            raise ValueError("epsilon must be finite and positive")

        self._epsilon = np.float32(epsilon_f)
        if not bool(np.isfinite(self._epsilon)) or self._epsilon <= np.float32(0):
            raise ValueError('epsilon must remain finite and positive in float32')
        self._gamma.flags.writeable = False
        self._beta.flags.writeable = False
        self._weight.flags.writeable = False
        self.input_dimension = d
        self.vocab_size = v

    def __call__(self, hidden):
        if not isinstance(hidden, np.ndarray):
            raise ValueError("hidden must be a numpy.ndarray")
        if hidden.dtype != np.float32:
            raise ValueError("hidden must have dtype float32")
        if hidden.ndim != 2 or int(hidden.shape[1]) != self.input_dimension:
            raise ValueError("hidden must have shape (B, input_dimension)")
        if not 1 <= int(hidden.shape[0]) <= 8:
            raise ValueError("require 1 <= B <= 8")
        if not bool(np.all(np.isfinite(hidden))):
            raise ValueError("hidden must contain only finite values")

        x = np.ascontiguousarray(hidden, dtype=np.float32)

        mean = np.mean(x, axis=1, dtype=np.float32)
        centered = x - mean[:, None]
        variance = np.mean(centered * centered, axis=1, dtype=np.float32)
        std = np.sqrt(variance + self._epsilon)
        if not bool(np.all(np.isfinite(std))):
            raise ValueError("nonfinite normalization")
        normalized = centered / std[:, None] * self._gamma + self._beta
        if not bool(np.all(np.isfinite(normalized))):
            raise ValueError("nonfinite normalized values")

        logits = normalized @ self._weight.T
        if not bool(np.all(np.isfinite(logits))):
            raise ValueError("nonfinite logits")

        row_max = np.max(logits, axis=1)
        shifted = logits - row_max[:, None]
        exp = np.exp(shifted)
        sum_exp = np.sum(exp, axis=1, dtype=np.float32)
        log_probs = shifted - np.log(sum_exp)[:, None]

        result = np.ascontiguousarray(log_probs, dtype=np.float32)
        if not bool(np.all(np.isfinite(result))):
            raise ValueError("nonfinite log-softmax output")
        return result
