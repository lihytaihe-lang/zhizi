import { readdir } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

const rootDirectory = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const testPattern = /\.(test|spec)\.(ts|tsx|js|jsx|mjs|cjs)$/;
const ignoredDirectories = new Set([
  ".git",
  "dist",
  "node_modules",
  "src-tauri",
]);

async function collectTests(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const tests = [];

  for (const entry of entries) {
    const fullPath = path.join(directory, entry.name);

    if (entry.isDirectory()) {
      if (!ignoredDirectories.has(entry.name)) {
        tests.push(...(await collectTests(fullPath)));
      }
      continue;
    }

    if (entry.isFile() && testPattern.test(entry.name)) {
      tests.push(fullPath);
    }
  }

  return tests;
}

function splitArguments(args) {
  const nodeTestArgs = [];
  const requestedFiles = [];

  for (const arg of args) {
    if (arg === "--run") {
      continue;
    }

    if (arg.startsWith("-")) {
      nodeTestArgs.push(arg);
    } else {
      requestedFiles.push(path.resolve(rootDirectory, arg));
    }
  }

  return { nodeTestArgs, requestedFiles };
}

const { nodeTestArgs, requestedFiles } = splitArguments(process.argv.slice(2));
const discoveredTests = requestedFiles.length > 0
  ? requestedFiles
  : await collectTests(path.join(rootDirectory, "src"));

if (discoveredTests.length === 0) {
  console.error("No frontend test files found.");
  process.exit(1);
}

const nodeArgs = [
  "--import",
  pathToFileURL(path.join(rootDirectory, "scripts/register-typescript-test-loader.mjs")).href,
  "--import",
  pathToFileURL(path.join(rootDirectory, "scripts/frontend-test-environment.mjs")).href,
  "--test",
  ...nodeTestArgs,
  ...discoveredTests.sort(),
];

const result = spawnSync(process.execPath, nodeArgs, {
  cwd: rootDirectory,
  stdio: "inherit",
});

process.exit(result.status ?? 1);
