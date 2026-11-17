import math

import torch


def gemm_layer_norm(
    x,
    weight,
    bias,
    epsilon: float,
    dimension: int,
    mean_weight,
    hierarchical: bool = False,
):
    if not isinstance(dimension, int) or isinstance(dimension, bool) or dimension <= 0:
        raise ValueError("dimension must be a positive int")
    if isinstance(epsilon, bool) or not isinstance(epsilon, (int, float)):
        raise ValueError("epsilon must be a finite positive number")
    if not math.isfinite(epsilon) or epsilon <= 0:
        raise ValueError("epsilon must be a finite positive number")
    if not isinstance(hierarchical, bool):
        raise ValueError("hierarchical must be a bool")
    if mean_weight is None:
        raise ValueError("mean_weight must not be None")
    if hierarchical and dimension % 16 != 0:
        raise ValueError("dimension must be divisible by 16 when hierarchical is True")

    def statistical_mean(value):
        if not hierarchical:
            return torch.matmul(value, mean_weight)
        chunks = torch.stack(torch.split(value, 16, dim=-1), dim=-2)
        block_means = torch.matmul(chunks, mean_weight[:16] * (dimension / 16)).squeeze(-1)
        return torch.matmul(block_means, mean_weight[: dimension // 16] * 16)

    centered = x - statistical_mean(x)
    variance = statistical_mean(centered * centered)
    return centered * torch.rsqrt(variance + epsilon) * weight + bias
