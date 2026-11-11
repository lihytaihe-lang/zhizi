import type { StoreSkin } from "../lib/skinStore";
import { formatSkinPrice } from "../lib/skinStore";
import { SkinStorePreview } from "./SkinStorePreview";

export type SkinStoreOperation = "install" | "update" | "purchase" | "use" | "uninstall";

type SkinStoreActionProps = {
  skin: StoreSkin;
  current: boolean;
  operation?: SkinStoreOperation;
  disabled: boolean;
  onAction: () => void;
};

export function SkinStoreAction({ skin, current, operation, disabled, onAction }: SkinStoreActionProps) {
  const upgrade = skin.state === "needsAppUpgrade";
  const using = skin.state === "installed" && current;
  const busyLabels: Record<SkinStoreOperation, string> = {
    install: "正在获取…", update: "正在更新…", purchase: "正在购买…", use: "正在切换…", uninstall: "正在卸载…",
  };
  const label = operation ? busyLabels[operation] : upgrade ? "需要升级知字"
    : skin.state === "updateAvailable" ? "更新"
    : skin.state === "installed" ? using ? "使用中" : "设为当前"
    : skin.tier === "paid" && skin.owned !== true ? `购买 ${formatSkinPrice(skin.priceCents)}` : "获取";
  return (
    <>
      <button className={`primary-button skin-store-action${using ? " is-current" : ""}`}
        disabled={disabled || Boolean(operation) || upgrade || using} aria-busy={Boolean(operation)}
        aria-label={`${label} ${skin.name.zh}`}
        onClick={onAction} type="button" title={upgrade ? `需要升级知字至 ${skin.minAppVersion} 或更高版本` : undefined}>
        {operation ? <span role="progressbar" aria-label={`${skin.name.zh} ${label}`} /> : null}
        {label}
      </button>
      {upgrade ? <small className="skin-store-upgrade">需要升级知字至 {skin.minAppVersion} 或更高版本</small> : null}
    </>
  );
}

type SkinStoreCardProps = SkinStoreActionProps & {
  previewEnabled: boolean;
  loadPreview: () => Promise<string>;
  onOpen: () => void;
  onUninstall: () => void;
};

export function SkinStoreCard({ previewEnabled, loadPreview, onOpen, onUninstall, ...action }: SkinStoreCardProps) {
  const { skin, operation, disabled } = action;
  return (
    <article className="skin-store-card" onClick={onOpen}>
      {skin.state === "updateAvailable" ? <span className="skin-store-new">新版</span> : null}
      <button className="skin-store-card-open" aria-label={`查看 ${skin.name.zh} 详情`} type="button" onClick={onOpen}>
        <SkinStorePreview name={skin.name.zh} enabled={previewEnabled} load={loadPreview} />
        <span className="skin-store-card-copy">
          <strong>{skin.name.zh}</strong><small>{skin.name.en}</small><span>{skin.author}</span>
          <span className="skin-store-tags">{skin.animated ? "动画" : "静态"}</span>
        </span>
      </button>
      <div className="skin-store-card-footer" onClick={(event) => event.stopPropagation()}>
        <span className="skin-store-price">{formatSkinPrice(skin.priceCents)}</span>
        <SkinStoreAction {...action} />
        {skin.installedVersion !== null ? (
          <button className="secondary-button" disabled={disabled || Boolean(operation)} onClick={onUninstall}
            aria-label={`卸载 ${skin.name.zh}`} type="button">卸载</button>
        ) : null}
      </div>
    </article>
  );
}
