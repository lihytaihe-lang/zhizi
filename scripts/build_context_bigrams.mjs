#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const MAGIC = Buffer.from("AIMCTX01", "ascii");
const VERSION = 1;
const MAX_OUTPUT_BYTES = 20 * 1024 * 1024;
const SEGMENTER_LOCALE = "zh-CN";

function fail(message) {
  process.stderr.write(`context bigram builder: ${message}\n`);
  process.exit(2);
}

function validToken(value) {
  const bytes = Buffer.from(value, "utf8");
  return value.length > 0
    && bytes.length <= 0xffff
    && !value.includes("\t")
    && !value.includes("\n")
    && !value.includes("\r")
    && ![...value].some((character) => character.codePointAt(0) < 0x20);
}

function addPair(pairs, left, right, count, source) {
  if (!validToken(left) || !validToken(right)) fail(`invalid token in ${source}`);
  if (!Number.isSafeInteger(count) || count <= 0 || count > 0xffffffff) {
    fail(`invalid count in ${source}`);
  }
  const key = `${left}\u0000${right}`;
  const next = (pairs.get(key)?.count ?? 0) + count;
  if (next > 0xffffffff) fail(`count overflow in ${source}`);
  pairs.set(key, { left, right, count: next });
}

function addTokenSequence(pairs, tokens, source) {
  for (let tokenIndex = 1; tokenIndex < tokens.length; tokenIndex += 1) {
    addPair(pairs, tokens[tokenIndex - 1], tokens[tokenIndex], 1, source);
  }
}

function parseSegmentedChinese(path, pairs, text) {
  const segmenter = new Intl.Segmenter(SEGMENTER_LOCALE, { granularity: "word" });
  for (const [index, rawLine] of text.split(/\r?\n/u).entries()) {
    if (rawLine.trim().length === 0 || rawLine.trimStart().startsWith("#")) continue;
    const source = `${path}:${index + 1}`;
    let tokens = [];
    for (const item of segmenter.segment(rawLine)) {
      if (item.isWordLike) {
        tokens.push(item.segment);
      } else if (tokens.length > 0) {
        addTokenSequence(pairs, tokens, source);
        tokens = [];
      }
    }
    if (tokens.length > 0) addTokenSequence(pairs, tokens, source);
  }
}

function parseInput(path, pairs, segmentChinese) {
  const text = readFileSync(path, "utf8");
  if (segmentChinese) {
    parseSegmentedChinese(path, pairs, text);
    return;
  }
  for (const [index, rawLine] of text.split(/\r?\n/u).entries()) {
    const line = rawLine.trim();
    if (line.length === 0 || line.startsWith("#")) continue;
    const source = `${path}:${index + 1}`;
    const columns = rawLine.split("\t");
    if (columns.length === 3 && /^[1-9][0-9]*$/u.test(columns[2])) {
      addPair(pairs, columns[0], columns[1], Number(columns[2]), source);
      continue;
    }
    const tokens = line.split(/\s+/u);
    if (tokens.length < 2) fail(`expected TSV pair or tokenized text at ${source}`);
    addTokenSequence(pairs, tokens, source);
  }
}

function compareEntries(left, right) {
  const leftWord = Buffer.from(left.left, "utf8");
  const rightWord = Buffer.from(right.left, "utf8");
  const first = Buffer.compare(leftWord, rightWord);
  return first !== 0
    ? first
    : Buffer.compare(Buffer.from(left.right, "utf8"), Buffer.from(right.right, "utf8"));
}

function build(inputs, segmentChinese) {
  const pairs = new Map();
  inputs.forEach((path) => parseInput(path, pairs, segmentChinese));
  const entries = [...pairs.values()].sort(compareEntries);
  const chunks = [MAGIC, Buffer.alloc(8)];
  chunks[1].writeUInt32LE(VERSION, 0);
  chunks[1].writeUInt32LE(entries.length, 4);
  for (const entry of entries) {
    const left = Buffer.from(entry.left, "utf8");
    const right = Buffer.from(entry.right, "utf8");
    const header = Buffer.alloc(8);
    header.writeUInt16LE(left.length, 0);
    header.writeUInt16LE(right.length, 2);
    header.writeUInt32LE(entry.count, 4);
    chunks.push(header, left, right);
  }
  const bytes = Buffer.concat(chunks);
  if (bytes.length > MAX_OUTPUT_BYTES) fail("model exceeds the owner-frozen 20 MiB limit");
  return { bytes, entries: entries.length };
}

const arguments_ = process.argv.slice(2);
const check = arguments_[0] === "--check";
if (check) arguments_.shift();
const segmentChinese = arguments_[0] === "--segment-zh";
if (segmentChinese) arguments_.shift();
if (arguments_.length < 2) fail("usage: [--check] [--segment-zh] <input>... <output.bin>");
const output = resolve(arguments_.pop());
const inputs = arguments_.map((path) => resolve(path));
const result = build(inputs, segmentChinese);
const checksum = createHash("sha256").update(result.bytes).digest("hex");
if (check) {
  const existing = readFileSync(output);
  if (!existing.equals(result.bytes)) fail("checked model is not reproducible from frozen input");
} else {
  writeFileSync(output, result.bytes, { mode: 0o644 });
}
process.stdout.write(
  `context_bigram_model=${check ? "CHECKED" : "BUILT"} entries=${result.entries} bytes=${result.bytes.length} sha256=${checksum} segmenter=${segmentChinese ? SEGMENTER_LOCALE : "tokenized"} icu=${process.versions.icu}\n`,
);
