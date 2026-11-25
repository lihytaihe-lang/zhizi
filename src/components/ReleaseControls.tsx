import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauriRuntime } from "../lib/tauriRuntime";

interface ReleaseIdentity {
  version: string;
  build: string;
}

interface ReleaseInfo {
  manager: ReleaseIdentity;
  installed: ReleaseIdentity | null;
  loaded: ReleaseIdentity | null;
  bundled: ReleaseIdentity | null;
}

type CheckResult =
  | { status: "current" | "incompatible" | "unavailable" }
  | { status: "available"; release: ReleaseIdentity; downloadUrl: string; notesUrl: string };

function sameRelease(first: ReleaseIdentity, second: ReleaseIdentity): boolean {
  return first.version === second.version && first.build === second.build;
}

export function ReleaseControls() {
  const [info, setInfo] = useState<ReleaseInfo | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CheckResult | null>(null);
  const [linkError, setLinkError] = useState(false);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let mounted = true;
    const refresh = () => {
      void invoke<ReleaseInfo>("get_release_info").then(
        (value) => { if (mounted) setInfo(value); },
        () => { if (mounted) setInfo(null); },
      );
    };
    refresh();
    const timer = open ? window.setInterval(refresh, 10_000) : undefined;
    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, [open]);

  if (!isTauriRuntime()) return null;
  const installed = info?.installed;
  const loaded = info?.loaded;
  const desired = info?.bundled;
  const installedDiffers = !!desired && (!installed || !sameRelease(desired, installed));
  const loadedDiffers = !!loaded && !!installed && !sameRelease(loaded, installed);

  async function check() {
    setBusy(true);
    setResult(null);
    setLinkError(false);
    try {
      setResult(await invoke<CheckResult>("check_stable_update"));
    } catch {
      setResult({ status: "unavailable" });
    } finally {
      setBusy(false);
    }
  }

  async function openRelease(url: string) {
    setLinkError(false);
    try {
      await openUrl(url);
    } catch {
      setLinkError(true);
    }
  }

  return (
    <div className="release-controls">
      <button type="button" className="status-pill" aria-expanded={open} aria-controls="zhizi-release"
        onClick={() => setOpen(!open)}>
        {info ? info.manager.version : "版本与更新"}
      </button>
      {open && (
        <section id="zhizi-release" className="release-details" aria-label="版本与更新">
          <dl>
            <dt>管理台</dt>
            <dd>{info ? info.manager.version : "读取中"}</dd>
            <dt>输入法已加载</dt>
            <dd>{loaded ? loaded.version : "等待确认。请选择「知字拼音」并输入一次。"}</dd>
            <dt>输入法已安装</dt>
            <dd>{installed ? installed.version : "未找到"}</dd>
          </dl>
          {installedDiffers && <p>此管理台内置输入法 {desired.version}。请退出并重新打开管理台，再选择「知字拼音」。</p>}
          {loadedDiffers && <p>已安装的输入法尚未加载。请切换到其他输入法，再切回「知字拼音」。若提示仍在，请注销后重新登录。</p>}
          <div role="status">
            {busy && <p>正在检查更新…</p>}
            {result?.status === "current" && <p>目前没有可用的管理台更新。</p>}
            {result?.status === "incompatible" && <p>当前系统不适用于官网最新安装包。</p>}
            {result?.status === "unavailable" && <p>暂时无法检查更新。请检查网络后重试。</p>}
            {result?.status === "available" && <p>有新版本 {result.release.version}。</p>}
            {linkError && <p>链接打开失败。请稍后重试。</p>}
          </div>
          <div className="release-actions">
            <button type="button" className="view-tab" disabled={busy} onClick={() => void check()}>检查更新</button>
            {result?.status === "available" && <>
              <button type="button" className="view-tab" onClick={() => void openRelease(result.notesUrl)}>更新说明</button>
              <button type="button" className="view-tab" onClick={() => void openRelease(result.downloadUrl)}>下载安装包</button>
              <button type="button" className="view-tab" onClick={() => { setResult(null); setOpen(false); }}>稍后</button>
            </>}
          </div>
        </section>
      )}
    </div>
  );
}
