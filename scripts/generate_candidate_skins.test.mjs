import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

const repository = fileURLToPath(new URL("..", import.meta.url));
const catalogPaths = [
  "native/macos/Sources/AgentInputMethodHostKit/CandidateSkinCatalog.swift",
  "src-tauri/src/skin_catalog.rs",
  "src/domain/skinCatalog.ts",
];

async function fixture(run) {
  const root = await mkdtemp(path.join(os.tmpdir(), "candidate-skin-generator-"));
  try {
    for (const file of ["scripts/generate_candidate_skins.mjs", "native/resources/Skins", ...catalogPaths]) {
      await cp(path.join(repository, file), path.join(root, file), { recursive: true });
    }
    const directory = path.join(root, "native/resources/Skins/Builtin/xuanmo");
    const skin = JSON.parse(await readFile(path.join(directory, "skin.json"), "utf8"));
    for (const name of ["mark.png", "scroll-frame.png", "drybrush.png"]) {
      await rm(path.join(directory, name), { force: true });
    }
    await cp(path.join(repository, "docs/current/evidence/skin-m3-art-c/keyed/xuanmo-frame-b.png"),
      path.join(directory, "frame.png"));
    skin.material = "none";
    skin.decorations = [{ type: "frame", sheet: "frame.png", slice: { left: 40, right: 40, top: 34, bottom: 34 } }];
    await run({ root, directory, skin });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

async function check({ root, directory, skin }) {
  await writeFile(path.join(directory, "skin.json"), `${JSON.stringify(skin, null, 2)}\n`);
  return spawnSync(process.execPath, [path.join(root, "scripts/generate_candidate_skins.mjs"), "--check"], { encoding: "utf8" });
}

test("frame and none use the existing format without changing any catalog bytes", async () => {
  await fixture(async (context) => {
    for (const material of ["none", "popover"]) {
      context.skin.material = material;
      for (const opacity of [undefined, 0, 1]) {
        if (opacity === undefined) delete context.skin.decorations[0].opacity;
        else context.skin.decorations[0].opacity = opacity;
        const result = await check(context);
        assert.equal(result.status, 0, result.stderr);
      }
    }
    context.skin.material = "none";
    context.skin.decorations[0].slice = { left: 0, right: 39.5, top: 0, bottom: 33.5 };
    assert.equal((await check(context)).status, 0);
    for (const file of catalogPaths) {
      assert.deepEqual(await readFile(path.join(context.root, file)), await readFile(path.join(repository, file)));
    }
  });
});

test("none also accepts an existing static sprite", async () => {
  await fixture(async (context) => {
    context.skin.decorations = [{ type: "sprite", sheet: "frame.png", frames: 1, size: { width: 256, height: 72 } }];
    const result = await check(context);
    assert.equal(result.status, 0, result.stderr);
  });
});

test("twitch accepts a short burst with rest and rejects invalid timing", async () => {
  const schema = JSON.parse(await readFile(path.join(repository, "native/resources/Skins/skin.schema.json"), "utf8"));
  const definition = schema.$defs.motion;
  assert.ok(definition.properties.kind.enum.includes("twitch"));
  const rule = definition.allOf.find((entry) => entry.if?.properties.kind.const === "twitch");
  assert.deepEqual(new Set(rule.then.required), new Set(["degrees", "delay", "duration"]));
  assert.equal(definition.properties.duration.minimum, 0.3);
  assert.equal(definition.properties.duration.maximum, 0.5);
  await fixture(async (context) => {
    const motion = { kind: "twitch", degrees: 8, period: 6, delay: 1.2, duration: 0.4 };
    context.skin.decorations = [{ type: "sprite", sheet: "frame.png", frames: 1,
      size: { width: 256, height: 72 }, motion: [motion] }];
    assert.equal((await check(context)).status, 0);
    for (const delay of [0, 0.5]) {
      context.skin.decorations[0].motion = [{ ...motion, period: 1, delay, duration: 0.5 }];
      assert.equal((await check(context)).status, 0);
    }
    for (const [field, value] of [["degrees", 0.2], ["degrees", 12.1], ["delay", -0.1],
      ["delay", 5.7], ["duration", 0.29], ["duration", 0.51], ["period", 0.9],
      ["period", 20.1], ["unknown", 1], ["duration", undefined], ["delay", undefined]]) {
      context.skin.decorations[0].motion = [{ ...motion, [field]: value }];
      const result = await check(context);
      assert.equal(result.status, 1, `${field}=${value}`);
      assert.match(result.stderr, /motion/);
    }
  });
});

const rejected = [
  ["missing slice", (frame) => { delete frame.slice; }, /slice/],
  ["missing cut", (frame) => { delete frame.slice.bottom; }, /slice/],
  ["extra cut", (frame) => { frame.slice.extra = 1; }, /slice/],
  ["array slice", (frame) => { frame.slice = [40, 40, 34, 34]; }, /slice/],
  ["null slice", (frame) => { frame.slice = null; }, /slice/],
  ["negative cut", (frame) => { frame.slice.left = -1; }, /slice.left/],
  ["oversized cut", (frame) => { frame.slice.left = 2049; }, /slice.left/],
  ["string cut", (frame) => { frame.slice.left = "40"; }, /slice.left/],
  ["null cut", (frame) => { frame.slice.left = null; }, /slice.left/],
  ["no horizontal middle", (frame) => { frame.slice.left = 472; }, /stretch/],
  ["no vertical middle", (frame) => { frame.slice.top = 110; }, /stretch/],
  ["negative opacity", (frame) => { frame.opacity = -0.1; }, /opacity/],
  ["oversized opacity", (frame) => { frame.opacity = 1.1; }, /opacity/],
  ["null opacity", (frame) => { frame.opacity = null; }, /opacity/],
  ["extra frame field", (frame) => { frame.layer = "below"; }, /frame/],
  ["missing sheet", (frame) => { delete frame.sheet; }, /sheet/],
  ["unknown sheet", (frame) => { frame.sheet = "absent.png"; }, /sheet/],
  ["unsafe sheet", (frame) => { frame.sheet = "../frame.png"; }, /sheet/],
];
for (const [name, mutate, error] of rejected) {
  test(`rejects ${name}`, async () => {
    await fixture(async (context) => {
      mutate(context.skin.decorations[0]);
      const result = await check(context);
      assert.equal(result.status, 1);
      assert.match(result.stderr, error);
    });
  });
}

test("a frame counts as an asset use while an unreferenced png still fails", async () => {
  await fixture(async (context) => {
    await cp(path.join(context.directory, "frame.png"), path.join(context.directory, "unused.png"));
    const result = await check(context);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /unused.png: is not used by any decoration/);
  });
});

test("rejects a frame sheet without a PNG header", async () => {
  await fixture(async (context) => {
    await writeFile(path.join(context.directory, "frame.png"), "not a png");
    const result = await check(context);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /png/i);
  });
});
