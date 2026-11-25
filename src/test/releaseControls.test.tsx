import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { ReleaseControls } from "../components/ReleaseControls";
import { findButton, installMiniDom, MiniMouseEvent, type MiniElement } from "./support/miniDom";

async function click(button: MiniElement | null) {
  assert.ok(button);
  await act(async () => {
    button.dispatchEvent(new MiniMouseEvent("click", { bubbles: true, cancelable: true }));
  });
}

test("release details hide builds while same-version install and restart prompts track loaded identity", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const dom = installMiniDom();
  const desired = { version: "0.3.54", build: "26373" };
  const info = {
    manager: desired,
    bundled: desired,
    installed: { ...desired, build: "26372" },
    loaded: { ...desired, build: "26371" },
  };
  const runtime = window as unknown as {
    __TAURI_INTERNALS__?: { invoke: (command: string) => Promise<unknown> };
    IS_REACT_ACT_ENVIRONMENT?: boolean;
  };
  runtime.IS_REACT_ACT_ENVIRONMENT = true;
  runtime.__TAURI_INTERNALS__ = {
    invoke: async (command) => {
      if (command === "get_release_info") return { ...info };
      assert.equal(command, "check_stable_update");
      return {
        status: "available",
        release: { ...desired, build: "26374" },
        downloadUrl: "https://example.test/download",
        notesUrl: "https://example.test/notes",
      };
    },
  };
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<ReleaseControls />); });
    const versionButton = findButton(dom.container, "0.3.54");
    assert.equal(versionButton?.textContent, "0.3.54");
    await click(versionButton);
    const details = dom.container.querySelector("#zhizi-release");
    assert.ok(details);
    assert.deepEqual(details.querySelectorAll("dd").map((element) => element.textContent),
      ["0.3.54", "0.3.54", "0.3.54"]);
    assert.match(details.textContent, /内置输入法 0\.3\.54。请退出并重新打开管理台/);
    assert.match(details.textContent, /已安装的输入法尚未加载/);

    info.installed = desired;
    await act(async () => { t.mock.timers.tick(10_000); });
    assert.doesNotMatch(details.textContent, /此管理台内置输入法/);
    assert.match(details.textContent, /已安装的输入法尚未加载/);

    info.loaded = desired;
    await act(async () => { t.mock.timers.tick(10_000); });
    assert.doesNotMatch(details.textContent, /此管理台内置输入法|已安装的输入法尚未加载/);

    await click(findButton(details, "检查更新"));
    assert.match(details.textContent, /有新版本 0\.3\.54。/);
    assert.ok(findButton(details, "下载安装包"));
    assert.doesNotMatch(dom.container.textContent, /构建|2637[1-4]/);
  } finally {
    await act(async () => { root.unmount(); });
    delete runtime.__TAURI_INTERNALS__;
    delete runtime.IS_REACT_ACT_ENVIRONMENT;
    dom.cleanup();
  }
});
