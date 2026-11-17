"""Weight-rounding attribution helper (PyTorch + stdlib only)."""

import copy
import re

import torch

_GROUPS = ("all-body", "embeddings", "normalization", "linear")

_EXACT = {"embeddings": frozenset({"tok.weight", "pos.weight"})}

_REGEX = {
    "normalization": re.compile(r"^blocks\.\d+\.(ln1|ln2)\.(weight|bias)$"),
    "linear": re.compile(r"^blocks\.\d+\.(qkv|proj|mlp\.(0|2))\.(weight|bias)$"),
}


def _selected(name, group):
    if group == "all-body":
        return any(_selected(name, g) for g in ("embeddings", "normalization", "linear"))
    if group in _EXACT:
        return name in _EXACT[group]
    return _REGEX[group].fullmatch(name) is not None


def rounded_copy(module, group):
    if group not in _GROUPS:
        raise ValueError(f"unknown group: {group!r}")

    copied = copy.deepcopy(module)

    selected_names = []
    element_count = 0
    max_abs_delta = 0.0
    underflow_to_zero = 0

    for name, param in copied.named_parameters():
        if not _selected(name, group):
            continue
        if param.dtype != torch.float32:
            raise ValueError(f"selected parameter {name} is not float32")
        original = param.detach()
        if not bool(torch.isfinite(original).all()):
            raise ValueError(f"selected parameter {name} contains non-finite values")
        with torch.no_grad():
            roundtripped = original.to(torch.float16).to(torch.float32)
        if not bool(torch.isfinite(roundtripped).all()):
            raise ValueError(f"fp16 roundtrip produced non-finite values in {name}")
        delta = (roundtripped - original).abs()
        if delta.numel() > 0:
            max_abs_delta = max(max_abs_delta, delta.max().item())
        underflow_to_zero += int(((original != 0) & (roundtripped == 0)).sum().item())
        with torch.no_grad():
            param.copy_(roundtripped)
        selected_names.append(name)
        element_count += original.numel()

    report = {
        "group": group,
        "selectedNames": selected_names,
        "selectedTensorCount": len(selected_names),
        "selectedElementCount": element_count,
        "maxAbsoluteDelta": max_abs_delta,
        "underflowToZeroCount": underflow_to_zero,
    }
    return copied, report
