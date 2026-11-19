import { useEffect, useRef } from "react";
import type { StoreSkin } from "../lib/skinStore";
import { formatSkinPrice } from "../lib/skinStore";
import { SkinStoreAction, type SkinStoreOperation } from "./SkinStoreCard";
import { SkinStorePreview } from "./SkinStorePreview";

type SkinStoreDetailsProps = {
  skin: StoreSkin;
  operation?: SkinStoreOperation;
  disabled: boolean;
  previewEnabled: boolean;
  loadPreview: () => Promise<string>;
  message: string;
  error: string;
  onAction: () => void;
  onClose: () => void;
};

export function SkinStoreDetails({ previewEnabled, loadPreview, message, error, onClose, ...action }: SkinStoreDetailsProps) {
  const drawer = useRef<HTMLDivElement>(null);
  const { skin } = action;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    drawer.current?.querySelector<HTMLButtonElement>("button")?.focus();
    function keyDown(event: KeyboardEvent) {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); }
      if (event.key !== "Tab") return;
      const buttons = Array.from(drawer.current?.querySelectorAll<HTMLButtonElement>("button") ?? []).filter((button) => !button.disabled);
      const first = buttons[0];
      const last = buttons[buttons.length - 1];
      if (!buttons.includes(document.activeElement as HTMLButtonElement)) {
        event.preventDefault(); (event.shiftKey ? last : first)?.focus();
      } else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
    document.addEventListener("keydown", keyDown, true);
    return () => { document.removeEventListener("keydown", keyDown, true); previous?.focus(); };
  }, [onClose]);
  const bytes = skin.bundleBytes;
  const size = bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KiB` : `${(bytes / (1024 * 1024)).toFixed(2)} MiB`;
  return (
    <div className="skin-store-drawer-backdrop" onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="skin-store-drawer" role="dialog" aria-modal="true" aria-label={`${skin.name.zh} 详情`} ref={drawer}>
        <div className="skin-store-drawer-heading">
          <h3>{skin.name.zh}<small>{skin.name.en}</small></h3>
          <button className="secondary-button" onClick={onClose} aria-label="关闭皮肤详情" type="button">关闭</button>
        </div>
        <SkinStorePreview name={skin.name.zh} enabled={previewEnabled} load={loadPreview} />
        <p>{skin.description}</p>
        <dl className="skin-store-details">
          <dt>价格</dt><dd>{formatSkinPrice(skin.priceCents, skin.currency)}</dd>
          <dt>适用系统</dt><dd>{(skin.platforms ?? ["macos"]).map((platform) => (({ macos: "macOS", windows: "Windows", linux: "Ubuntu / Linux" } as Record<string, string>)[platform] ?? platform)).join("、")}</dd>
          <dt>版本</dt><dd>{skin.version}</dd>
          <dt>大小</dt><dd>{size}</dd>
          <dt>更新日期</dt><dd>{skin.updatedAt}</dd>
          <dt>作者</dt><dd>{skin.author}</dd>
          <dt>许可</dt><dd>{skin.license}</dd>
          <dt>更新说明</dt><dd>{skin.changelog}</dd>
          <dt>预览</dt><dd>{skin.animated ? "动画皮肤，本期展示静态预览" : "静态皮肤"}</dd>
        </dl>
        <SkinStoreAction {...action} />
        {message ? <p className="settings-message" role="status">{message}</p> : null}
        {error ? <p className="skin-store-notice" role="alert">{error}</p> : null}
      </div>
    </div>
  );
}
