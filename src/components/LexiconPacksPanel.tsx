import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import {
  lexiconPackInstall,
  lexiconPackRemove,
  lexiconPacksAvailable,
  lexiconPacksInstalled,
  type InstalledPack,
  type PackIndex,
} from "../lib/lexiconPacks";

type ListedPack = { id: string; name: string; description: string; rows: number | null };

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function releaseBusy(id: string) {
  return (current: ReadonlySet<string>) => {
    const next = new Set(current);
    next.delete(id);
    return next;
  };
}

/** 官方词库包的下载面板。可下载列表来自 GitHub；已安装列表只读本机文件，断网时也能看到和管理已装的包。 */
export function LexiconPacksPanel() {
  const [index, setIndex] = useState<PackIndex | null>(null);
  const [installed, setInstalled] = useState<InstalledPack[]>([]);
  const [indexError, setIndexError] = useState("");
  // 按包记录忙碌状态：同一包进行中时禁用并忽略重复提交，不同包互不影响。
  const [busyIds, setBusyIds] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [message, setMessage] = useState("");

  async function load() {
    setMessage("");
    void lexiconPacksInstalled().then(setInstalled).catch((error) => setMessage(errorMessage(error)));
    try {
      setIndex(await lexiconPacksAvailable());
      setIndexError("");
    } catch (error) {
      setIndex(null);
      setIndexError(errorMessage(error));
    }
  }

  useEffect(() => { void load(); }, []);

  async function install(pack: ListedPack) {
    if (busyIds.has(pack.id)) return;
    setBusyIds((current) => new Set(current).add(pack.id));
    setMessage("");
    try {
      const result = await lexiconPackInstall(pack.id);
      setInstalled((current) => [...current.filter((item) => item.id !== result.id), result]);
    } catch (error) {
      setMessage(`${pack.name} 安装失败：${errorMessage(error)}`);
    } finally {
      setBusyIds(releaseBusy(pack.id));
    }
  }

  async function remove(pack: ListedPack) {
    if (!window.confirm(`移除词库包「${pack.name}」？`)) return;
    if (busyIds.has(pack.id)) return;
    setBusyIds((current) => new Set(current).add(pack.id));
    setMessage("");
    try {
      setInstalled(await lexiconPackRemove(pack.id));
    } catch (error) {
      setMessage(`${pack.name} 移除失败：${errorMessage(error)}`);
    } finally {
      setBusyIds(releaseBusy(pack.id));
    }
  }

  const listed: ListedPack[] = [
    ...(index?.packs ?? []).map((pack) => ({
      id: pack.id, name: pack.name, description: pack.description, rows: pack.rows,
    })),
    ...installed
      .filter((pack) => !(index?.packs ?? []).some((entry) => entry.id === pack.id))
      .map((pack) => ({ id: pack.id, name: pack.id, description: "", rows: null })),
  ];
  const installedIds = new Set(installed.map((pack) => pack.id));

  return (
    <section className="panel packs-panel" aria-labelledby="packs-title">
      <div className="panel-heading">
        <div>
          <h2 id="packs-title">词库包</h2>
          <p>下载官方整理的整包词库，安装后离线使用</p>
        </div>
        {indexError ? <button className="secondary-button" onClick={() => void load()} type="button"><RefreshCw size={15} />重试</button> : null}
      </div>

      {indexError ? <p className="packs-error" role="status">取词库包列表失败：{indexError}</p> : null}
      {message ? <p className="packs-error" role="status">{message}</p> : null}

      <div className="packs-list">
        {listed.map((pack) => {
          const isInstalled = installedIds.has(pack.id);
          const busy = busyIds.has(pack.id);
          return (
            <article className="packs-row" key={pack.id}>
              <div className="packs-row-info">
                <strong>{pack.name}</strong>
                {pack.description ? <span>{pack.description}</span> : null}
                {pack.rows !== null ? <small>{pack.rows} 条</small> : null}
              </div>
              <div className="packs-row-actions">
                {isInstalled ? (
                  <>
                    <span className="packs-installed">已安装</span>
                    <button className="secondary-button" disabled={busy} onClick={() => void remove(pack)} type="button">{busy ? "正在移除…" : "移除"}</button>
                  </>
                ) : (
                  <button className="primary-button" disabled={busy} onClick={() => void install(pack)} type="button">{busy ? "正在下载…" : "下载并安装"}</button>
                )}
              </div>
            </article>
          );
        })}
        {listed.length === 0 && indexError === "" ? (
          <div className="empty-state">{index === null ? "正在读取词库包列表…" : "现在没有可下载的词库包。"}</div>
        ) : null}
      </div>

      <p className="packs-note">安装或移除后，在输入法菜单的『词库…』里打开或关闭对应词库；输入法会在下次切换到它时自动载入新词库。</p>
    </section>
  );
}
