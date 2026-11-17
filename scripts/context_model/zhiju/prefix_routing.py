import numpy as np

SEQUENCE_LENGTH = 40
BOS_TOKEN_ID = 1
MAX_ROWS = 8


def _validate_int(value, name, low, high=None):
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(name + " must be a Python int")
    if value < low or (high is not None and value > high):
        raise ValueError(name + " is out of range")


def _row_valid_length(row, vocabulary_count):
    if int(row[0]) != BOS_TOKEN_ID:
        raise ValueError("every row must start with the BOS token id 1")
    valid_length = 0
    while valid_length < SEQUENCE_LENGTH:
        token = int(row[valid_length])
        if token == 0:
            break
        if token < BOS_TOKEN_ID or token >= vocabulary_count:
            raise ValueError("valid token ids must satisfy 1 <= id < vocabulary_count")
        valid_length += 1
    if np.any(row[valid_length:] != 0):
        raise ValueError("padding must be zero-only right padding")
    return valid_length


def _validate_predictions(predictions, row_count, vocabulary_count):
    if not isinstance(predictions, np.ndarray):
        raise TypeError("predict callbacks must return a numpy.ndarray")
    if predictions.dtype != np.dtype(np.float32):
        raise TypeError("predict callbacks must return float32 distributions")
    if predictions.shape != (row_count, vocabulary_count):
        raise ValueError("predict callbacks must return one distribution per row")
    if not np.all(np.isfinite(predictions)):
        raise ValueError("predict callbacks must return finite distributions")


def route_distributions(
    tokens,
    vocabulary_count,
    baseline_predict,
    adapted_predict,
    minimum_tokens=16,
):
    _validate_int(vocabulary_count, "vocabulary_count", 3)
    _validate_int(minimum_tokens, "minimum_tokens", 2, SEQUENCE_LENGTH)
    if not callable(baseline_predict):
        raise TypeError("baseline_predict must be callable")
    if not callable(adapted_predict):
        raise TypeError("adapted_predict must be callable")
    if not isinstance(tokens, np.ndarray):
        raise TypeError("tokens must be a numpy.ndarray")
    if tokens.dtype != np.dtype(np.int32):
        raise TypeError("tokens must have dtype int32")
    if tokens.ndim != 2 or tokens.shape[1] != SEQUENCE_LENGTH:
        raise ValueError("tokens must be a [N, 40] matrix")
    row_count = int(tokens.shape[0])
    if not 1 <= row_count <= MAX_ROWS:
        raise ValueError("tokens must contain between 1 and 8 rows")

    valid_lengths = [
        _row_valid_length(tokens[index], vocabulary_count)
        for index in range(row_count)
    ]

    baseline_indices = [
        index for index in range(row_count) if valid_lengths[index] < minimum_tokens
    ]
    adapted_indices = [
        index for index in range(row_count) if valid_lengths[index] >= minimum_tokens
    ]

    output = np.empty((row_count, vocabulary_count), dtype=np.float32)

    for indices, predict in (
        (baseline_indices, baseline_predict),
        (adapted_indices, adapted_predict),
    ):
        if not indices:
            continue
        group = np.array(tokens[indices], dtype=np.int32, copy=True, order="C")
        predictions = predict(group)
        _validate_predictions(predictions, len(indices), vocabulary_count)
        output[indices] = predictions

    return output
