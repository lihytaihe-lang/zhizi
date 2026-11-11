import { useEffect, useRef, useState } from "react";

type SkinStorePreviewProps = {
  name: string;
  enabled: boolean;
  load: () => Promise<string>;
};

export function SkinStorePreview({ name, enabled, load }: SkinStorePreviewProps) {
  const container = useRef<HTMLSpanElement>(null);
  const [source, setSource] = useState("");
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!enabled || !container.current || typeof IntersectionObserver === "undefined") return;
    let active = true;
    const observer = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      observer.disconnect();
      void load().then((value) => {
        if (!value.startsWith("data:image/png;base64,")) throw new Error("预览格式不可用");
        if (active) setSource(value);
      }).catch(() => { if (active) setFailed(true); });
    });
    observer.observe(container.current);
    return () => { active = false; observer.disconnect(); };
  }, [enabled, load]);
  return (
    <span className="skin-store-preview" ref={container}>
      {source && !failed ? (
        <img alt={`${name} 皮肤预览`} src={source} onError={() => setFailed(true)} />
      ) : (
        <span className="skin-store-preview-placeholder">
          {failed ? "预览暂不可用" : enabled ? "皮肤预览" : "刷新后加载预览"}
        </span>
      )}
    </span>
  );
}
