#!/bin/bash
# Runs ON the rented GPU (恒源云, image PyTorch 2.9.1 / CUDA 12.8 / Python 3.11).
# The image already ships torch; only numpy is checked. No owner data is uploaded:
# the bundle holds the public/synthetic corpus, the homophone set and the code.
# Evaluation on the owner's real records happens back on the Mac.
#
#   bash gpu_run.sh <run-name> [preset] [pretrain-steps] [homophone-steps]
set -euo pipefail
RUN=${1:?run name}; PRESET=${2:-30m}; PRE_STEPS=${3:-28000}; HOMO_STEPS=${4:-8000}; SEED=${5:-1337}
MIX=${MIX:-wikipedia:0.4,un:0.1,asap:0.2,tatoeba:0.05,synth:0.25}
cd "$(dirname "$0")"
nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv
python -c "import torch, numpy; print('torch', torch.__version__, 'cuda', torch.cuda.is_available(), torch.cuda.get_device_name(0))"
mkdir -p runs
{
  echo "=== pretrain $(date '+%F %T')"
  python train.py --corpus corpus/v2 --out runs/$RUN-pre --preset $PRESET --batch 256 --lr 1.5e-3 \
      --steps $PRE_STEPS --eval-every 2000 --compile --seed $SEED \
      --mix $MIX
  echo "=== homophone $(date '+%F %T')"
  python train_homophone.py --init runs/$RUN-pre/best.pt --data homophone/all --corpus corpus/v2 \
      --out runs/$RUN-homo --steps $HOMO_STEPS --batch 128 --eval-every 1000 --seed $SEED
  echo "=== pack $(date '+%F %T')"
  tar -czf /hy-tmp/result-$RUN.tar.gz runs/$RUN-pre/best.pt runs/$RUN-pre/log.jsonl \
      runs/$RUN-homo/best.pt runs/$RUN-homo/log.jsonl
  echo "=== DONE $(date '+%F %T')"
} 2>&1 | tee runs/$RUN.log
