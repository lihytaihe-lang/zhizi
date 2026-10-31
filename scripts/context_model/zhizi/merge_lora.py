"""Fold a personal LoRA (personal_lora.py) into a copy of the base checkpoint, so the existing exporters
(export_onnx.py, the Core ML conversion) see an ordinary 顺手·知字 checkpoint. Local only: the result
carries the owner's adaptation and is never shipped in a public build.

  python merge_lora.py --base clean-v2/soup/best.pt --lora runs/lora-v4-s1200/lora.pt --out runs/lora-v4-s1200/merged.pt
"""
import argparse
import os
import sys

import torch

sys.path.insert(0, os.path.dirname(__file__))
from model import load  # noqa: E402
from personal_lora import add_lora, LoRALinear  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--lora", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--rank", type=int, default=8)
    ap.add_argument("--alpha", type=float, default=16)
    ap.add_argument("--strength", type=float, default=1.0,
                    help="fraction of the adapter folded in (interpolation toward the base; the full adapter overrode "
                         "context homophones such as 在/再 in the owner replay, 2026-10-28)")
    args = ap.parse_args()

    model, _, ck = load(args.base, "cpu")
    add_lora(model, args.rank, args.alpha)
    weights = torch.load(args.lora, map_location="cpu")
    missing = set(n for n, _ in model.named_parameters() if n.endswith(".a") or n.endswith(".b")) - set(weights)
    if missing:
        sys.exit(f"adapter lacks {len(missing)} tensors, e.g. {sorted(missing)[:2]}")
    model.load_state_dict(weights, strict=False)

    # W' = W + scale * B A, then put the plain Linear back where the adapter was.
    with torch.no_grad():
        for blk in model.blocks:
            for owner, name in ((blk, "qkv"), (blk, "proj"), (blk.mlp, "0"), (blk.mlp, "2")):
                wrapped = owner[int(name)] if name.isdigit() else getattr(owner, name)
                assert isinstance(wrapped, LoRALinear)
                wrapped.base.weight += (wrapped.b @ wrapped.a) * wrapped.scale * args.strength
                if name.isdigit():
                    owner[int(name)] = wrapped.base
                else:
                    setattr(owner, name, wrapped.base)
    merged = dict(ck)
    merged["state"] = model.state_dict()
    merged["personal_lora"] = {"adapter": os.path.abspath(args.lora), "rank": args.rank, "alpha": args.alpha,
                              "strength": args.strength}
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    torch.save(merged, args.out)
    print(f"merged -> {args.out}")


if __name__ == "__main__":
    main()
