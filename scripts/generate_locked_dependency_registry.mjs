import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputPath = path.join(root, "docs/current/licenses/LOCKED_DEPENDENCIES.json");
const generatedFrom = ["package-lock.json", "src-tauri/Cargo.lock", "native/core/Cargo.lock", "native/ipc/Cargo.lock"];
const sha256 = (contents) => createHash("sha256").update(contents).digest("hex");
const packageLock = JSON.parse(readFileSync(path.join(root, "package-lock.json"), "utf8"));
const npm = Object.entries(packageLock.packages)
  .filter(([location]) => location !== "")
  .map(([location, value]) => ({
    name: location.replace(/^node_modules\//u, ""),
    version: value.version,
    license: value.license,
  }))
  .sort((left, right) => left.name.localeCompare(right.name) || left.version.localeCompare(right.version));

const cargoByID = new Map();
for (const manifest of ["src-tauri/Cargo.toml", "native/core/Cargo.toml", "native/ipc/Cargo.toml"]) {
  const metadata = JSON.parse(execFileSync("cargo", [
    "metadata", "--manifest-path", manifest, "--locked", "--offline", "--format-version", "1",
  ], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    env: { ...process.env, CARGO_NET_OFFLINE: "true" },
  }));
  for (const value of metadata.packages) {
    if (!value.source) continue;
    cargoByID.set(`${value.name}@${value.version}@${value.source}`, {
      name: value.name,
      version: value.version,
      license: value.license,
      source: value.source,
    });
  }
}
const cargo = [...cargoByID.values()].sort((left, right) =>
  left.name.localeCompare(right.name) || left.version.localeCompare(right.version));

const missing = [...npm, ...cargo].filter((value) => !value.license);
const copyleftOnly = [...npm, ...cargo].filter((value) => {
  const license = value.license ?? "";
  return /GPL/u.test(license) && !/(MIT|Apache|BSD|ISC|Zlib|BSL|MPL|Unicode|Unlicense|0BSD|CDLA|CC0)/u.test(license);
});
if (missing.length || copyleftOnly.length) {
  throw new Error(`dependency license gate failed: missing=${missing.length} copyleftOnly=${copyleftOnly.length}`);
}

const registry = {
  formatVersion: 2,
  generatedFrom,
  generatedFromSha256: Object.fromEntries(generatedFrom.map((relativePath) => [
    relativePath,
    sha256(readFileSync(path.join(root, relativePath))),
  ])),
  generationMode: "cargo metadata --locked --offline; package-lock.json packages",
  policy: "No unknown license and no dependency whose only declared option is GPL/LGPL/AGPL.",
  npm,
  cargo,
  summary: {
    npmPackages: npm.length,
    cargoPackages: cargo.length,
    missingLicenses: missing.length,
    copyleftOnlyPackages: copyleftOnly.length,
  },
};
const serialized = `${JSON.stringify(registry, null, 2)}\n`;
if (process.argv.includes("--check")) {
  const current = readFileSync(outputPath, "utf8");
  if (current !== serialized) throw new Error("LOCKED_DEPENDENCIES.json is stale");
} else {
  writeFileSync(outputPath, serialized);
}
console.log(JSON.stringify(registry.summary));
