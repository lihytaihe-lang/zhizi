import torch


def stable_layer_norm(x, weight, bias, epsilon: float, dimension: int, mode: str):
    if isinstance(dimension, bool) or not isinstance(dimension, int):
        raise ValueError("dimension must be a positive integer")
    if dimension <= 0:
        raise ValueError("dimension must be a positive integer")
    if isinstance(epsilon, bool) or not isinstance(epsilon, (int, float)):
        raise ValueError("epsilon must be a finite positive number")
    if not (epsilon > 0) or epsilon == float("inf"):
        raise ValueError("epsilon must be a finite positive number")

    normalized_shape = (dimension,)

    if mode == "native":
        return torch.nn.functional.layer_norm(
            x, normalized_shape, weight, bias, epsilon
        )

    if mode == "centered-native":
        centered = x - x.mean(dim=-1, keepdim=True)
        return torch.nn.functional.layer_norm(
            centered, normalized_shape, weight, bias, epsilon
        )

    if mode == "centered-explicit":
        centered = x - x.mean(dim=-1, keepdim=True)
        variance = (centered * centered).mean(dim=-1, keepdim=True)
        return centered * torch.rsqrt(variance + epsilon) * weight + bias

    raise ValueError("unknown mode")
