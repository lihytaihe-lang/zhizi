#!/bin/bash
# Runs ON the rented GPU.  One pretraining, then two homophone fine-tunings from it: A = with the owner's own rows, B = the same data without them.
set -euo pipefail
cd "$(dirname "$0")"
MIX=wikipedia:0.3,un:0.1,asap:0.2,tatoeba:0.05,synth:0.35
mkdir -p runs homophone/noowner
{
  echo "=== pretrain $(date '+%F %T')"
  python train.py --corpus corpus/v2 --out runs/c2-pre --preset 30m --batch 256 --lr 1.5e-3 --steps 28000 --eval-every 2000 --compile --seed 1337 --mix $MIX
  echo "=== homophone A (with owner rows) $(date '+%F %T')"
  python train_homophone.py --init runs/c2-pre/best.pt --data homophone/all --corpus corpus/v2 --out runs/c2-homoA --steps 8000 --batch 128 --eval-every 1000 --seed 0 --mix $MIX
  echo "=== homophone B (without owner rows) $(date '+%F %T')"
  grep -v '"src": "owner"' homophone/all/train.jsonl > homophone/noowner/train.jsonl
  cp homophone/all/valid.jsonl homophone/noowner/valid.jsonl
  python train_homophone.py --init runs/c2-pre/best.pt --data homophone/noowner --corpus corpus/v2 --out runs/c2-homoB --steps 8000 --batch 128 --eval-every 1000 --seed 0 --mix $MIX
  echo "=== pack $(date '+%F %T')"
  tar -czf /hy-tmp/result-c2.tar.gz runs/c2-pre/best.pt runs/c2-pre/log.jsonl runs/c2-homoA/best.pt runs/c2-homoA/log.jsonl runs/c2-homoB/best.pt runs/c2-homoB/log.jsonl
  echo "=== DONE $(date '+%F %T')"
} 2>&1 | tee runs/c2.log
