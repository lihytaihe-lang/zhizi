#!/bin/bash
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
# Waits for the synthesis runner, then builds corpus v2, the full homophone set and the GPU bundle.
set -u
ML="${ZHIZI_WORK:-$ROOT_DIR/ime_context_model}"; PY=$ML/.venv/bin/python
until grep -qE "all batches 8-10 done|FAILED" $ML/synth/v1/runner.log; do sleep 60; done
grep -E "FAILED" $ML/synth/v1/runner.log && echo "runner stopped on a failed batch; packaging what is DONE"
set -e
mkdir -p $ML/corpus/v2
for f in $ML/corpus/v1/*; do ln -sf "$f" $ML/corpus/v2/; done
rm -f $ML/corpus/v2/manifest.json && cp $ML/corpus/v1/manifest.json $ML/corpus/v2/manifest.json
cd $ML/src
$PY prepare_synth.py --corpus $ML/corpus/v2
$PY make_homophone_set.py --corpus $ML/corpus/v2 --sources synth:0 --out $ML/homophone/synth
mkdir -p $ML/homophone/all
cat $ML/homophone/base/train.jsonl $ML/homophone/synth/train.jsonl > $ML/homophone/all/train.jsonl
cat $ML/homophone/base/valid.jsonl $ML/homophone/synth/valid.jsonl > $ML/homophone/all/valid.jsonl
wc -l $ML/homophone/all/*.jsonl
# GPU bundle: code + corpus v2 bins/vocab + homophone/all. No owner records.
B=$ML/bundle/zhizi-v2; rm -rf $B; mkdir -p $B/corpus/v2 $B/homophone/all
cp $ML/src/{model,train,train_homophone}.py $ML/src/gpu_run.sh $B/
cp -L $ML/corpus/v2/*.bin $ML/corpus/v2/vocab.json $ML/corpus/v2/manifest.json $B/corpus/v2/
cp $ML/homophone/all/*.jsonl $B/homophone/all/
tar -C $ML/bundle -czf $ML/bundle/zhizi-v2.tar.gz zhizi-v2
ls -la $ML/bundle/zhizi-v2.tar.gz; md5 $ML/bundle/zhizi-v2.tar.gz
echo AFTER_SYNTH_DONE
