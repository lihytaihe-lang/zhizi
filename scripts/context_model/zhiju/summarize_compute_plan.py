#!/usr/bin/env python3
"""Summarize a GLM compute plan JSON into an estimated-compute report.

Stdlib only. CLI:
    python summarize_compute_plan.py --input plan.json --out report.json

Scope: Core ML estimated compute plan only; not measured runtime or
40ms/controller acceptance.
"""

import argparse
import hashlib
import json
import math
import os
import sys

SCOPE = (
    "Core ML estimated compute plan only; not measured runtime or "
    "40ms/controller acceptance"
)

FP16_TOKENS = ("fp16", "float16")
FP32_TOKENS = ("fp32", "float32")
MAX_TOP_OPERATIONS = 10


class InputError(Exception):
    """Raised when the input payload fails validation."""


def _require_nonempty_str(value, label):
    if not isinstance(value, str) or not value:
        raise InputError("%s must be a nonempty string" % label)
    return value


def _require_operation_list(value, label):
    if not isinstance(value, list):
        raise InputError("%s must be a list" % label)
    return value


def _validate_dtype_list(value, label):
    if not isinstance(value, list):
        raise InputError("%s must be a list of nonempty strings" % label)
    for item in value:
        if not isinstance(item, str) or not item:
            raise InputError("%s must contain only nonempty strings" % label)
    return list(value)


def _validate_estimated_cost(value, label):
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise InputError("%s must be null or a finite number" % label)
    cost = float(value)
    if not math.isfinite(cost):
        raise InputError("%s must be a finite number" % label)
    if cost < 0.0 or cost > 1.0:
        raise InputError("%s must be within [0, 1]" % label)
    return cost


def _validate_payload(payload):
    if not isinstance(payload, dict):
        raise InputError("top-level JSON must be a mapping")

    variants = payload.get("variants")
    if not isinstance(variants, list) or not variants:
        raise InputError("variants must be a nonempty list")

    seen_variant_names = set()
    for variant_index, variant in enumerate(variants):
        if not isinstance(variant, dict):
            raise InputError("variants[%d] must be a mapping" % variant_index)
        name = _require_nonempty_str(
            variant.get("name"), "variants[%d].name" % variant_index
        )
        if name in seen_variant_names:
            raise InputError("duplicate variant name: %s" % name)
        seen_variant_names.add(name)

        operations = _require_operation_list(
            variant.get("operations"), "variants[%s].operations" % name
        )

        seen_operation_names = set()
        for op_index, operation in enumerate(operations):
            label = "variants[%s].operations[%d]" % (name, op_index)
            if not isinstance(operation, dict):
                raise InputError("%s must be a mapping" % label)
            op_name = _require_nonempty_str(
                operation.get("name"), label + ".name"
            )
            if op_name in seen_operation_names:
                raise InputError(
                    "duplicate operation name in variant %s: %s"
                    % (name, op_name)
                )
            seen_operation_names.add(op_name)
            _require_nonempty_str(
                operation.get("operator"), label + ".operator"
            )
            _require_nonempty_str(operation.get("device"), label + ".device")
            _validate_estimated_cost(
                operation.get("estimatedCost"), label + ".estimatedCost"
            )
            _validate_dtype_list(
                operation.get("inputDtypes"), label + ".inputDtypes"
            )
            _validate_dtype_list(
                operation.get("outputDtypes"), label + ".outputDtypes"
            )


def _dtypes_contain(tokens, dtypes):
    # Membership probes for aliases must not consume the source iterator.
    lowered = {dtype.lower() for dtype in dtypes}
    return any(token in lowered for token in tokens)


def _summarize_variant(variant):
    operations = variant.get("operations") or []

    operation_count = len(operations)
    missing_cost_count = 0
    cost_by_operator = {}
    cost_by_device = {}
    operator_counts = {}
    device_counts = {}
    fp16_to_fp32_cast_count = 0
    fp32_to_fp16_cast_count = 0
    ranked_known_cost = []

    for index, operation in enumerate(operations):
        operator = operation.get("operator")
        device = operation.get("device")
        cost = operation.get("estimatedCost")

        operator_counts[operator] = operator_counts.get(operator, 0) + 1
        device_counts[device] = device_counts.get(device, 0) + 1

        if cost is None:
            missing_cost_count += 1
        else:
            cost_by_operator[operator] = (
                cost_by_operator.get(operator, 0.0) + float(cost)
            )
            cost_by_device[device] = cost_by_device.get(device, 0.0) + float(cost)
            ranked_known_cost.append((index, float(cost), operation))

        if operator == "cast":
            input_dtypes = operation.get("inputDtypes") or []
            output_dtypes = operation.get("outputDtypes") or []
            if _dtypes_contain(FP16_TOKENS, input_dtypes) and _dtypes_contain(
                FP32_TOKENS, output_dtypes
            ):
                fp16_to_fp32_cast_count += 1
            elif _dtypes_contain(FP32_TOKENS, input_dtypes) and _dtypes_contain(
                FP16_TOKENS, output_dtypes
            ):
                fp32_to_fp16_cast_count += 1

    ranked_known_cost.sort(key=lambda item: (-item[1], item[0]))
    top_operations = [item[2] for item in ranked_known_cost[:MAX_TOP_OPERATIONS]]

    return {
        "name": variant.get("name"),
        "operationCount": operation_count,
        "missingCostCount": missing_cost_count,
        "costByOperator": {
            key: round(value, 10) for key, value in cost_by_operator.items()
        },
        "costByDevice": {
            key: round(value, 10) for key, value in cost_by_device.items()
        },
        "operatorCounts": operator_counts,
        "deviceCounts": device_counts,
        "fp16ToFp32CastCount": fp16_to_fp32_cast_count,
        "fp32ToFp16CastCount": fp32_to_fp16_cast_count,
        "topOperations": top_operations,
    }


def build_report(payload, input_sha256):
    return {
        "inputSHA256": input_sha256,
        "scope": SCOPE,
        "variants": [_summarize_variant(variant) for variant in payload["variants"]],
    }


def _write_report_exclusive(report, out_path):
    text = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_TRUNC"):
        flags |= getattr(os, "O_TRUNC", 0)
    fd = os.open(out_path, flags, 0o644)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
    except BaseException:
        try:
            os.unlink(out_path)
        except OSError:
            pass
        raise


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Summarize a Core ML estimated compute plan JSON."
    )
    parser.add_argument("--input", required=True, help="path to plan JSON")
    parser.add_argument("--out", required=True, help="path for report JSON")
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)

    if os.path.exists(args.out):
        sys.stderr.write("error: output already exists: %s\n" % args.out)
        return 1

    try:
        with open(args.input, "rb") as handle:
            raw = handle.read()
    except OSError as exc:
        sys.stderr.write("error: cannot read input: %s\n" % exc)
        return 1

    input_sha256 = hashlib.sha256(raw).hexdigest()

    try:
        payload = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        sys.stderr.write("error: invalid JSON input: %s\n" % exc)
        return 1

    try:
        _validate_payload(payload)
        report = build_report(payload, input_sha256)
        _write_report_exclusive(report, args.out)
    except InputError as exc:
        sys.stderr.write("error: %s\n" % exc)
        return 1
    except OSError as exc:
        sys.stderr.write("error: cannot write output: %s\n" % exc)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
