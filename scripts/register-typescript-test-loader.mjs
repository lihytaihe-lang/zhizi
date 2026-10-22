import { register } from "node:module";
import { pathToFileURL } from "node:url";

register(new URL("./typescript-test-loader.mjs", import.meta.url), pathToFileURL("./"));
