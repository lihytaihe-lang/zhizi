import assert from "node:assert/strict";
import { test } from "node:test";
import { loadLastResultAction, saveLastResultAction } from "../lib/resultAction";

test("last explicit result action persists only as the next highlight", () => {
  window.localStorage.clear();
  assert.equal(loadLastResultAction(), "copy");

  saveLastResultAction("insert");
  assert.equal(loadLastResultAction(), "insert");

  window.localStorage.setItem("agent-input-layer.last-result-action", "invalid");
  assert.equal(loadLastResultAction(), "copy");
});
