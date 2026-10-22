import assert from "node:assert/strict";
import { test } from "node:test";

test("frontend test runner exposes DOM and localStorage globals", () => {
  window.localStorage.clear();

  const button = document.createElement("button");
  button.textContent = "Compose";
  document.body.appendChild(button);
  window.localStorage.setItem("agent-input-layer.test", "ready");

  assert.equal(document.body.children.at(-1), button);
  assert.equal(window.localStorage.getItem("agent-input-layer.test"), "ready");
});
