"""Pairwise prefix contrastive loss over cumulative target-only log-probabilities."""

import math

import torch
import torch.nn.functional as F

__all__ = ["prefix_pairwise_loss"]


def _validate_margin(margin):
    if isinstance(margin, bool) or not isinstance(margin, (int, float)):
        raise TypeError("margin must be a finite nonnegative Python number")
    margin_value = float(margin)
    if not math.isfinite(margin_value):
        raise ValueError("margin must be finite")
    if margin_value < 0.0:
        raise ValueError("margin must be nonnegative")
    return margin_value


def _validate_integer(value, name):
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{name} must be a Python integer")
    return value


def prefix_pairwise_loss(cumulative, spans, margin):
    if not isinstance(cumulative, torch.Tensor):
        raise TypeError("cumulative must be a torch.Tensor")
    if not cumulative.dtype.is_floating_point:
        raise TypeError("cumulative must be a floating point tensor")
    if cumulative.dim() != 2:
        raise ValueError("cumulative must have shape [N, T]")

    margin_value = _validate_margin(margin)
    if not isinstance(spans, (list, tuple)):
        raise TypeError("spans must be a Python list of 4-tuples")

    if not spans:
        return cumulative.sum() * 0.0, 0, cumulative.new_zeros((), dtype=torch.int64)

    total_rows, total_columns = cumulative.shape

    validated = []
    term_count = 0
    for index, span in enumerate(spans):
        if not isinstance(span, tuple) or len(span) != 4:
            raise TypeError(
                "each span must be a 4-tuple "
                "(positiveRow, negativeRow, startInclusive, endExclusive)"
            )
        positive_row = _validate_integer(span[0], f"spans[{index}].positiveRow")
        negative_row = _validate_integer(span[1], f"spans[{index}].negativeRow")
        start = _validate_integer(span[2], f"spans[{index}].startInclusive")
        end = _validate_integer(span[3], f"spans[{index}].endExclusive")
        if not (0 <= positive_row < total_rows and 0 <= negative_row < total_rows):
            raise ValueError(f"span rows out of range for tensor with {total_rows} rows")
        if not (0 <= start < end <= total_columns):
            raise ValueError(
                f"span bounds invalid: require 0 <= start < end <= {total_columns}"
            )
        validated.append((positive_row, negative_row, start, end))
        term_count += end - start

    differences = torch.cat(
        [
            cumulative[negative_row, start:end]
            - cumulative[positive_row, start:end]
            + margin_value
            for positive_row, negative_row, start, end in validated
        ]
    )

    loss = F.softplus(differences).mean()
    correct_count = (differences < margin_value).sum()

    return loss, term_count, correct_count
