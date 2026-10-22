"""Write the common English word list the core uses to recognise an English word typed
where Chinese pinyin does not fit (opencode, deepseek, freelance).

Source: the whole-word tokens of the OpenAI CLIP BPE vocabulary (MIT, Copyright (c) 2021
OpenAI), ordered by token id, which follows merge order and so roughly frequency. Kept:
lowercase alphabetic words of three or more letters. Deterministic: one word per line,
most frequent first.

Usage: python3 scripts/generate_english_words.py <clip vocab.json>
"""
import json
import pathlib
import re
import sys

vocab = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
words = sorted(
    (index, token[:-4])
    for token, index in vocab.items()
    if token.endswith("</w>") and re.fullmatch(r"[a-z]{3,}", token[:-4])
)
out = pathlib.Path(__file__).resolve().parents[1] / "native/core/agent-ime-core/resources/english_words.txt"
out.write_text("".join(f"{word}\n" for _, word in words), encoding="utf-8")
print(f"english_words={len(words)} -> {out}")
