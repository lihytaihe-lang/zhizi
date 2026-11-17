import math
import torch


def aligned_gemm_layer_norm(x, weight, bias, epsilon: float, dimension: int, mean_weight=None):
    if isinstance(dimension, bool) or not isinstance(dimension, int):
        raise TypeError("dimension must be a Python int")
    if dimension <= 0 or dimension % 32 != 0:
        raise ValueError("dimension must be a positive int divisible by 32")
    if isinstance(epsilon, bool) or not isinstance(epsilon, (int, float)):
        raise TypeError("epsilon must be a Python float")
    if not math.isfinite(epsilon) or epsilon <= 0:
        raise ValueError("epsilon must be finite and positive")

    if mean_weight is None:
        mean_weight = torch.full(
            (dimension, 32), 1.0 / dimension, dtype=x.dtype, device=x.device
        )

    mean32 = torch.matmul(x, mean_weight)

    grouped = x.unflatten(-1, (dimension // 32, 32))
    centered_grouped = grouped - mean32.unsqueeze(-2)
    centered = centered_grouped.flatten(-2, -1)

    variance32 = torch.matmul(centered * centered, mean_weight)
    rstd = torch.rsqrt(variance32 + epsilon)

    normalized = (centered_grouped * rstd.unsqueeze(-2)).flatten(-2, -1)

    return normalized * weight + bias
