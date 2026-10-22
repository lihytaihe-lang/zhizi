"""Weight interpolation between two same-architecture checkpoints: (1 - alpha) * base + alpha * tuned.

Used to keep the language prior of the base model while taking part of a candidate-set fine-tune
(MODEL-V3-PLAN: trial2 helped in-distribution but hurt the product evaluation).
  python soup.py --base v2.pt --tuned trial2.pt --alpha 0.5 --out soup.pt
"""
import argparse
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, str(WORK_DIR / 'src'))
import torch  # noqa: E402

from model import load, save  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--tuned", required=True)
    ap.add_argument("--alpha", type=float, required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    base, vocab, _ = load(args.base, "cpu")
    tuned, _, _ = load(args.tuned, "cpu")
    ts = tuned.state_dict()
    mixed = {k: ((1 - args.alpha) * v.float() + args.alpha * ts[k].float()).to(v.dtype) if v.is_floating_point() else v
             for k, v in base.state_dict().items()}
    base.load_state_dict(mixed)
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    save(args.out, base, str(WORK_DIR / 'corpus/v2/vocab.json'), 0, {"soup_alpha": args.alpha})
    print("saved", args.out)


if __name__ == "__main__":
    main()
