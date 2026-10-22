import { createHash } from "node:crypto";
import { lstat, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const allowMissing = process.argv.includes("--allow-missing");
const bundleArgumentIndex = process.argv.indexOf("--bundle");
const bundle = bundleArgumentIndex >= 0 ? process.argv[bundleArgumentIndex + 1] : null;
if (bundleArgumentIndex >= 0 && (!bundle || !path.isAbsolute(bundle))) {
  throw new Error("--bundle requires an absolute input-method bundle path");
}
const manifestPath = path.join(root, "docs/current/licenses/NATIVE_LICENSE_ASSETS.json");
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const expectedComponents = [
  "Boost",
  "Darts-clone",
  "LevelDB",
  "OpenCC",
  "RapidJSON",
  "Rust standard library Apache license",
  "Rust standard library MIT license",
  "Rust standard library copyright notices",
  "glog",
  "librime",
  "jieba dictionary",
  "pypinyin phrase dictionary",
  "marisa-trie",
  "rime-pinyin-simp",
  "rime-pinyin-simp AUTHORS",
  "THUOCL lexicons (chengyu, IT, caijing)",
  "yaml-cpp",
].sort();

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

assert(manifest.formatVersion === 1 && Array.isArray(manifest.assets), "native license manifest format changed");
assert(JSON.stringify(manifest.assets.map((asset) => asset.component).sort()) === JSON.stringify(expectedComponents),
  "native license manifest component inventory changed");
assert(new Set(manifest.assets.map((asset) => asset.component)).size === manifest.assets.length,
  "duplicate native license component");
assert(new Set(manifest.assets.map((asset) => asset.bundleFilename)).size === manifest.assets.length,
  "duplicate native license bundle filename");

let locked = 0;
const missing = [];
for (const asset of manifest.assets) {
  assert(/^[A-Za-z0-9._+-]+\.txt$/u.test(asset.bundleFilename),
    `invalid bundle filename for ${asset.component}`);
  assert(!path.isAbsolute(asset.repositoryPath) && !asset.repositoryPath.split(path.sep).includes(".."),
    `invalid repository path for ${asset.component}`);
  const absolutePath = path.join(root, asset.repositoryPath);
  if (asset.status === "missing" || asset.status === "provenance-missing") {
    assert(asset.bytes === null && asset.sha256 === null, `missing asset has locked metadata: ${asset.component}`);
    if (asset.status === "provenance-missing") {
      assert(asset.sourceUrl === null, `unverified provenance has a source URL: ${asset.component}`);
    } else {
      assert(/^https:\/\/raw\.githubusercontent\.com\//u.test(asset.sourceUrl),
        `non-raw license source for ${asset.component}`);
      assert(!/(?:\/main\/|\/master\/|\/HEAD\/)/u.test(asset.sourceUrl),
        `moving license source for ${asset.component}`);
    }
    try {
      await lstat(absolutePath);
      throw new Error(`unregistered license file exists for ${asset.component}`);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
    missing.push(asset.component);
    continue;
  }
  assert(asset.status === "locked", `invalid license status for ${asset.component}`);
  if (asset.sourceKind === "locked-rust-toolchain-sysroot") {
    assert(asset.sourceUrl === null, `Rust toolchain asset unexpectedly has a remote source: ${asset.component}`);
    assert(asset.versionOrCommit === "1.94.1/e408947bfd200af42db322daf0fadfe7e26d3bd1",
      `Rust toolchain provenance changed for ${asset.component}`);
    assert(typeof asset.toolchainRelativePath === "string"
      && !path.isAbsolute(asset.toolchainRelativePath)
      && !asset.toolchainRelativePath.split(path.sep).includes("..")
      && asset.repositoryPath.startsWith(".build-deps/rust/1.94.1/licenses/"),
    `invalid Rust toolchain path for ${asset.component}`);
  } else {
    assert(asset.sourceKind === undefined, `unknown locked source kind for ${asset.component}`);
    assert(/^https:\/\/raw\.githubusercontent\.com\//u.test(asset.sourceUrl),
      `non-raw license source for ${asset.component}`);
    assert(!/(?:\/main\/|\/master\/|\/HEAD\/)/u.test(asset.sourceUrl),
      `moving license source for ${asset.component}`);
  }
  assert(Number.isSafeInteger(asset.bytes) && asset.bytes > 0, `invalid byte count for ${asset.component}`);
  assert(/^[0-9a-f]{64}$/u.test(asset.sha256), `invalid checksum for ${asset.component}`);
  const metadata = await lstat(absolutePath);
  assert(metadata.isFile() && !metadata.isSymbolicLink(), `license input is not a regular file: ${asset.component}`);
  const contents = await readFile(absolutePath);
  assert(contents.length === asset.bytes, `license byte count changed for ${asset.component}`);
  const actual = createHash("sha256").update(contents).digest("hex");
  assert(actual === asset.sha256, `license checksum changed for ${asset.component}: ${actual}`);
  if (bundle) {
    const bundledPath = path.join(bundle, "Contents/Resources/Licenses", asset.bundleFilename);
    const bundledMetadata = await lstat(bundledPath);
    assert(bundledMetadata.isFile() && !bundledMetadata.isSymbolicLink() && bundledMetadata.nlink === 1,
      `bundled license is not a regular file: ${asset.component}`);
    const bundledContents = await readFile(bundledPath);
    assert(bundledContents.length === asset.bytes, `bundled license byte count changed for ${asset.component}`);
    const bundledSHA256 = createHash("sha256").update(bundledContents).digest("hex");
    assert(bundledSHA256 === asset.sha256, `bundled license checksum changed for ${asset.component}`);
  }
  locked += 1;
}

if (bundle) {
  const licenseDirectory = path.join(bundle, "Contents/Resources/Licenses");
  const actualEntries = await readdir(licenseDirectory, { withFileTypes: true });
  assert(actualEntries.every((entry) => entry.isFile()), "bundle license directory contains a non-file entry");
  const actualFilenames = actualEntries.map((entry) => entry.name).sort();
  const expectedFilenames = manifest.assets.map((asset) => asset.bundleFilename).sort();
  assert(JSON.stringify(actualFilenames) === JSON.stringify(expectedFilenames),
    `bundle license inventory changed: ${JSON.stringify(actualFilenames)}`);
}

console.log(`native_license_assets=${missing.length ? "INCOMPLETE" : "PASS"} locked=${locked} missing=${missing.length} bundled=${bundle ? locked : 0}`);
for (const component of missing) console.log(`missing=${component}`);
if (missing.length && !allowMissing) process.exit(6);
