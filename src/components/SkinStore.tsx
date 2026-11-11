import { useCallback, useEffect, useRef, useState } from "react";
import { candidateSkinCatalog } from "../domain/skinCatalog";
import type { CandidateSkinCatalogSnapshot } from "../lib/lexicon";
import {
  getSkinStoreSettings, getSkinStorePreview, installStoreSkin, listStoreSkins,
  purchaseStoreSkin, refreshSkinStore, setSkinStoreUrl,
  type SkinStoreSettings, type StoreSkin, type StoreSnapshot,
} from "../lib/skinStore";
import { SkinStoreCard, type SkinStoreOperation } from "./SkinStoreCard";
import { SkinStoreDetails } from "./SkinStoreDetails";

type SkinStoreProps = {
  currentSkinId: string;
  onUseSkin: (id: string) => Promise<void>;
  onCatalogChange: (catalog: CandidateSkinCatalogSnapshot) => void;
  onUninstallSkin: (id: string) => Promise<void>;
  onBusyChange?: (busy: boolean) => void;
  localCatalogRevision?: number;
  busy?: boolean;
};

const emptySnapshot: StoreSnapshot = { skins: [], refreshedAtMs: null, stale: false, error: null };
const filters = ["全部", "免费", "付费", "动画", "已安装"] as const;

function updatedAgo(time: number | null): string {
  if (time === null) return "尚未更新";
  const minutes = Math.max(0, Math.floor((Date.now() - time) / 60000));
  return minutes < 1 ? "刚刚" : minutes < 60 ? `${minutes} 分钟前`
    : minutes < 1440 ? `${Math.floor(minutes / 60)} 小时前` : `${Math.floor(minutes / 1440)} 天前`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function SkinStore({ currentSkinId, onUseSkin, onCatalogChange, onUninstallSkin, onBusyChange, localCatalogRevision = 0, busy = false }: SkinStoreProps) {
  const [settings, setSettings] = useState<SkinStoreSettings>({ indexUrl: "", lastRefreshedAtMs: null });
  const [url, setUrl] = useState("");
  const [snapshot, setSnapshot] = useState(emptySnapshot);
  const [loading, setLoading] = useState(true);
  const cacheReadSequence = useRef(0);
  const cacheReading = useRef(true);
  const [refreshing, setRefreshing] = useState(false);
  const refreshLock = useRef(false);
  const operationLocks = useRef(new Set<string>());
  const [operations, setOperations] = useState<Record<string, SkinStoreOperation>>({});
  const [message, setMessage] = useState("");
  const [actionError, setActionError] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<typeof filters[number]>("全部");
  const [detailsId, setDetailsId] = useState<string | null>(null);
  const [removeId, setRemoveId] = useState<string | null>(null);
  const removeConfirmation = useRef<HTMLButtonElement>(null);
  const removeTrigger = useRef<HTMLElement | null>(null);
  const observedCatalogRevision = useRef(localCatalogRevision);
  const [previewsEnabled, setPreviewsEnabled] = useState(false);
  const [previewEpoch, setPreviewEpoch] = useState(0);
  const previews = useRef(new Map<string, () => Promise<string>>());
  const closeDetails = useCallback(() => setDetailsId(null), []);

  useEffect(() => {
    let active = true;
    const sequence = ++cacheReadSequence.current;
    void Promise.allSettled([getSkinStoreSettings(), listStoreSkins()]).then(([configuration, cached]) => {
      if (!active) return;
      if (configuration.status === "fulfilled") {
        setSettings(configuration.value);
        setUrl(configuration.value.indexUrl);
      } else setActionError(errorMessage(configuration.reason));
      if (sequence === cacheReadSequence.current) {
        if (cached.status === "fulfilled") setSnapshot(cached.value);
        else setSnapshot((previous) => ({ ...previous, stale: true, error: errorMessage(cached.reason) }));
        cacheReading.current = false;
        setLoading(false);
      }
    });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (observedCatalogRevision.current === localCatalogRevision) return;
    observedCatalogRevision.current = localCatalogRevision;
    let active = true;
    const sequence = ++cacheReadSequence.current;
    cacheReading.current = true;
    setLoading(true);
    void listStoreSkins().then((cached) => { if (active) setSnapshot(cached); })
      .catch((error) => { if (active) setSnapshot((previous) => ({ ...previous, stale: true, error: errorMessage(error) })); })
      .finally(() => {
        if (active && sequence === cacheReadSequence.current) { cacheReading.current = false; setLoading(false); }
      });
    return () => { active = false; };
  }, [localCatalogRevision]);

  useEffect(() => {
    if (removeId === null) return;
    removeConfirmation.current?.focus();
    removeConfirmation.current?.scrollIntoView?.({ block: "nearest" });
  }, [removeId]);

  async function refresh() {
    if (refreshLock.current || operationLocks.current.size || cacheReading.current || busy || loading) return;
    const address = url.trim();
    if (!address) { setActionError("请在商店地址框填写皮肤发布方提供的 HTTPS 地址，再点击连接。"); return; }
    try {
      if (new URL(address).protocol !== "https:") throw new Error();
    } catch { setActionError("皮肤商店地址必须使用 HTTPS。"); return; }
    refreshLock.current = true;
    setRefreshing(true);
    onBusyChange?.(true);
    setActionError("");
    setMessage("");
    try {
      const saved = await setSkinStoreUrl(address);
      setSettings(saved);
      const result = await refreshSkinStore();
      setSnapshot(result);
      if (!result.stale && !result.error) {
        previews.current.clear();
        setPreviewEpoch((value) => value + 1);
        setPreviewsEnabled(true);
      }
    } catch (error) {
      setSnapshot((previous) => ({ ...previous, stale: true, error: errorMessage(error) }));
    } finally {
      refreshLock.current = false;
      setRefreshing(false);
      onBusyChange?.(false);
    }
  }

  function previewFor(skin: StoreSkin) {
    const key = `${skin.id}:${skin.version}`;
    let loader = previews.current.get(key);
    if (!loader) {
      let request: Promise<string> | undefined;
      loader = () => request ?? (request = getSkinStorePreview(skin.id));
      previews.current.set(key, loader);
    }
    return loader;
  }

  async function runAction(skin: StoreSkin, uninstall = false) {
    if (operationLocks.current.size || refreshLock.current || cacheReading.current || busy || loading) return;
    const operation: SkinStoreOperation = uninstall ? "uninstall"
      : skin.state === "updateAvailable" ? "update"
      : skin.state === "installed" ? "use"
      : skin.tier === "paid" && skin.owned !== true ? "purchase" : "install";
    if (!uninstall && (skin.state === "needsAppUpgrade" || (skin.state === "installed" && skin.id === currentSkinId))) return;
    operationLocks.current.add(skin.id);
    setOperations((previous) => ({ ...previous, [skin.id]: operation }));
    onBusyChange?.(true);
    setActionError("");
    setMessage("");
    try {
      if (operation === "use") await onUseSkin(skin.id);
      else if (operation === "uninstall") {
        await onUninstallSkin(skin.id);
        setSnapshot((previous) => ({ ...previous, skins: previous.skins.map((item) => item.id === skin.id
          ? { ...item, state: item.state === "needsAppUpgrade" ? item.state : "notInstalled", installedVersion: null } : item) }));
      } else if (operation === "purchase") {
        const result = await purchaseStoreSkin(skin.id);
        setMessage(result.message);
        if (result.ok) {
          const resultSnapshot = await listStoreSkins();
          setSnapshot(resultSnapshot);
        }
      } else {
        const catalog = await installStoreSkin(skin.id);
        onCatalogChange(catalog);
        if (!catalog.skins.some((entry) => entry.id === skin.id && entry.status === "ready" && entry.origin === "external")) {
          throw new Error("皮肤安装未完成，请查看皮肤校验报告。");
        }
        setSnapshot((previous) => ({ ...previous, skins: previous.skins.map((item) => item.id === skin.id
          ? { ...item, state: "installed", installedVersion: item.version } : item) }));
        setMessage("皮肤已安装；无需重新构建 App，可直接设为当前。");
      }
    } catch (error) { setActionError(errorMessage(error)); }
    finally {
      operationLocks.current.delete(skin.id);
      setOperations((previous) => { const next = { ...previous }; delete next[skin.id]; return next; });
      onBusyChange?.(false);
    }
  }

  const storeSkins = snapshot.skins.filter((skin) => !candidateSkinCatalog.some((builtIn) => builtIn.id === skin.id));
  const search = query.trim().toLocaleLowerCase();
  const visible = storeSkins.filter((skin) => {
    if (filter === "免费" && skin.tier !== "free") return false;
    if (filter === "付费" && skin.tier !== "paid") return false;
    if (filter === "动画" && !skin.animated) return false;
    if (filter === "已安装" && skin.installedVersion === null) return false;
    return `${skin.name.zh} ${skin.name.en} ${skin.author} ${skin.description}`.toLocaleLowerCase().includes(search);
  });
  const selected = storeSkins.find((skin) => skin.id === detailsId);
  const removing = storeSkins.find((skin) => skin.id === removeId);
  const disabled = busy || refreshing || loading || Object.keys(operations).length > 0;
  const timestamp = snapshot.refreshedAtMs ?? settings.lastRefreshedAtMs;
  return (
    <section className="skin-store" aria-label="皮肤商店">
      <div className="skin-store-header">
        <h3>皮肤商店</h3>
        <button className="secondary-button skin-store-account" type="button" onClick={() => setMessage("账号功能尚未开放")}>登录</button>
      </div>
      <div className="skin-store-toolbar">
        <input className="skin-store-search" type="search" aria-label="搜索皮肤" placeholder="搜索名称、作者" value={query}
          onChange={(event) => setQuery(event.currentTarget.value)} />
        <div className="skin-store-filters" role="group" aria-label="筛选皮肤">
          {filters.map((item) => <button className={`skin-store-filter${filter === item ? " is-active" : ""}`} key={item}
            aria-pressed={filter === item} type="button" onClick={() => setFilter(item)}>{item}</button>)}
        </div>
        <div className="skin-store-refresh">
          <small className="skin-store-updated">更新于 {updatedAgo(timestamp)}</small>
          <button className="secondary-button" disabled={disabled || operationLocks.current.size > 0 || !url.trim()}
            type="button" onClick={() => void refresh()}>{refreshing ? "正在刷新…" : "刷新"}</button>
        </div>
      </div>
      <form className="skin-store-connect" onSubmit={(event) => { event.preventDefault(); void refresh(); }}>
        <label htmlFor="skin-store-url">商店地址</label>
        <input id="skin-store-url" type="url" placeholder="https://…" value={url} disabled={disabled}
          aria-describedby="skin-store-url-help" onChange={(event) => setUrl(event.currentTarget.value)} />
        <button className="secondary-button" disabled={disabled || operationLocks.current.size > 0 || !url.trim()} type="submit"
          onClick={(event) => { event.preventDefault(); void refresh(); }}>
          {refreshing ? "正在连接…" : "连接"}
        </button>
        <small id="skin-store-url-help">填写皮肤发布方提供的 HTTPS 地址；点击连接或刷新后才联网。</small>
      </form>
      {message && !selected ? <p className="settings-message" role="status">{message}</p> : null}
      {actionError && !selected ? <p className="skin-store-notice" role="alert">{actionError}</p> : null}
      {snapshot.stale || snapshot.error ? (
        <div className="skin-store-notice" role="alert">
          <p>上次更新于 {updatedAgo(timestamp)}，当前无法刷新</p>
          {timestamp !== null ? <time dateTime={new Date(timestamp).toISOString()}>{new Date(timestamp).toLocaleString("zh-CN")}</time> : null}
          {snapshot.error ? <p>{snapshot.error}</p> : null}
          <button className="secondary-button" type="button" disabled={disabled || !url.trim() || operationLocks.current.size > 0}
            onClick={() => void refresh()}>重试</button>
        </div>
      ) : null}
      {loading ? (
        <div className="skin-store-grid" role="status" aria-label="正在加载皮肤商店" aria-busy="true">
          {[0, 1, 2].map((index) => <div className="skin-store-skeleton" key={index} aria-hidden="true" />)}
          <span>正在加载皮肤商店…</span>
        </div>
      ) : storeSkins.length === 0 ? (
        <div className="empty-state">{timestamp === null ? "还没有连接皮肤商店" : "商店暂时没有皮肤"}</div>
      ) : visible.length === 0 ? <p className="empty-state">没有符合条件的皮肤</p> : (
        <div className="skin-store-grid">
          {visible.map((skin) => <SkinStoreCard key={`${skin.id}:${skin.version}:${previewEpoch}`} skin={skin}
            current={skin.id === currentSkinId} disabled={disabled} operation={operations[skin.id]}
            previewEnabled={previewsEnabled} loadPreview={previewFor(skin)}
            onAction={() => void runAction(skin)} onOpen={() => setDetailsId(skin.id)} onUninstall={() => {
              removeTrigger.current = document.activeElement as HTMLElement | null;
              setRemoveId(skin.id);
            }} />)}
        </div>
      )}
      {removing ? (
        <div className="skin-store-confirm" role="group" aria-label={`确认卸载 ${removing.name.zh}`}>
          <p>卸载「{removing.name.zh}」？{currentSkinId === removing.id ? "当前皮肤将回落简洁。" : ""}</p>
          <button className="secondary-button" type="button" disabled={disabled || Boolean(operations[removing.id])}
            ref={removeConfirmation}
            onClick={() => { setRemoveId(null); void runAction(removing, true); }}>确认卸载</button>
          <button className="secondary-button" type="button" onClick={() => { setRemoveId(null); removeTrigger.current?.focus(); }}>取消</button>
        </div>
      ) : null}
      {selected ? <SkinStoreDetails key={`${selected.id}:${previewEpoch}`} skin={selected} current={selected.id === currentSkinId}
        disabled={disabled} operation={operations[selected.id]} previewEnabled={previewsEnabled}
        message={message} error={actionError}
        loadPreview={previewFor(selected)} onAction={() => void runAction(selected)} onClose={closeDetails} /> : null}
    </section>
  );
}
