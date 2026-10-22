import { access, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import ts from "typescript";

const sourceExtensions = [".ts", ".tsx", ".js", ".jsx", ".mjs"];
const transpileExtensions = new Set([".ts", ".tsx"]);
const assetExtensions = new Set([
  ".avif",
  ".css",
  ".gif",
  ".jpeg",
  ".jpg",
  ".png",
  ".svg",
  ".webp",
]);

async function exists(filePath) {
  try {
    await access(filePath);
    return true;
  } catch {
    return false;
  }
}

function shouldTryFileResolution(specifier) {
  return (
    specifier.startsWith(".") ||
    specifier.startsWith("/") ||
    specifier.startsWith("file:")
  );
}

async function resolveSourceFile(specifier, parentURL) {
  const parentDirectory = parentURL ? path.dirname(fileURLToPath(parentURL)) : process.cwd();
  const basePath = specifier.startsWith("file:")
    ? fileURLToPath(specifier)
    : specifier.startsWith("/")
      ? specifier
      : path.resolve(parentDirectory, specifier);

  const candidates = path.extname(basePath)
    ? [basePath]
    : [
        ...sourceExtensions.map((extension) => `${basePath}${extension}`),
        ...sourceExtensions.map((extension) => path.join(basePath, `index${extension}`)),
      ];

  for (const candidate of candidates) {
    if (await exists(candidate)) {
      return pathToFileURL(candidate).href;
    }
  }

  return undefined;
}

export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (error) {
    if (!shouldTryFileResolution(specifier)) {
      throw error;
    }

    const resolved = await resolveSourceFile(specifier, context.parentURL);
    if (!resolved) {
      throw error;
    }

    return {
      shortCircuit: true,
      url: resolved,
    };
  }
}

export async function load(url, context, nextLoad) {
  if (!url.startsWith("file:")) {
    return nextLoad(url, context);
  }

  const extension = path.extname(fileURLToPath(url));

  if (transpileExtensions.has(extension)) {
    const source = await readFile(fileURLToPath(url), "utf8");
    const result = ts.transpileModule(source, {
      compilerOptions: {
        isolatedModules: true,
        jsx: ts.JsxEmit.ReactJSX,
        module: ts.ModuleKind.ES2022,
        moduleResolution: ts.ModuleResolutionKind.Bundler,
        target: ts.ScriptTarget.ES2020,
      },
      fileName: fileURLToPath(url),
    });

    return {
      format: "module",
      shortCircuit: true,
      source: result.outputText,
    };
  }

  if (assetExtensions.has(extension)) {
    return {
      format: "module",
      shortCircuit: true,
      source: `export default ${JSON.stringify(url)};`,
    };
  }

  return nextLoad(url, context);
}
