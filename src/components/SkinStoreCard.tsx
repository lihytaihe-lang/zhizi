import type { StoreSkin } from "../lib/skinStore";
import { formatSkinPrice } from "../lib/skinStore";
import { SkinStorePreview } from "./SkinStorePreview";

export type SkinStoreOperation = "install" | "update" | "purchase";

type SkinStoreActionProps = {
  skin: StoreSkin;
  operation?: SkinStoreOperation;
  disabled: boolean;
  onAction: () => void;
};

export function SkinStoreAction({ skin, operation, disabled, onAction }: SkinStoreActionProps) {
  const upgrade = skin.state === "needsAppUpgrade";
  const incompatible = skin.state === "unsupportedPlatform";
  const delisted = skin.availableForSale === false && skin.owned !== true && skin.state === "notInstalled";
  const installed = skin.state === "installed";
  const busyLabels: Record<SkinStoreOperation, string> = {
    install: "正在获取…", update: "正在更新…", purchase: "正在购买…",
  };
  const label = operation ? busyLabels[operation] : incompatible ? "暂不支持当前系统" : delisted ? "已停止新购" : upgrade ? "需要升级知字"
    : skin.state === "updateAvailable" ? "更新"
    : installed ? "已安装"
    : skin.tier === "paid" && skin.owned !== true ? `购买 ${formatSkinPrice(skin.priceCents, skin.currency)}` : "获取";
  return (
    <>
      <button className={`primary-button skin-store-action${installed ? " is-current" : ""}`}
        disabled={disabled || Boolean(operation) || upgrade || incompatible || delisted || installed} aria-busy={Boolean(operation)}
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
};

export function SkinStoreCard({ previewEnabled, loadPreview, onOpen, ...action }: SkinStoreCardProps) {
  const { skin } = action;
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
        <span className="skin-store-price">{formatSkinPrice(skin.priceCents, skin.currency)}</span>
        <SkinStoreAction {...action} />
      </div>
    </article>
  );
}
