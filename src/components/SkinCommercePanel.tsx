import { useCallback, useEffect, useState } from "react";
import { SkinAccountPanel } from "./SkinAccountPanel";
import { SkinStore } from "./SkinStore";

export function SkinCommercePanel() {
  const [tab, setTab] = useState<"store" | "account">("store");
  const [accountVisited, setAccountVisited] = useState(false);
  const [storeBusy, setStoreBusy] = useState(false);
  const [accountBusy, setAccountBusy] = useState(false);
  const [catalogRevision, setCatalogRevision] = useState(0);
  const [focusProduct, setFocusProduct] = useState<{ id: string; nonce: number }>();
  const busy = storeBusy || accountBusy;

  const rereadCatalog = useCallback(() => setCatalogRevision((revision) => revision + 1), []);
  useEffect(() => {
    // Returning from the native gallery can change which packages are installed.
    if (busy) return;
    window.addEventListener("focus", rereadCatalog);
    return () => window.removeEventListener("focus", rereadCatalog);
  }, [busy, rereadCatalog]);

  function openAccount() {
    setAccountVisited(true);
    setTab("account");
  }

  function findProduct(id: string) {
    setFocusProduct((previous) => ({ id, nonce: (previous?.nonce ?? 0) + 1 }));
    setTab("store");
  }

  return <>
    <div className="skin-segments" role="group" aria-label="皮肤商店与订单">
      <button type="button" className={`skin-segment${tab === "store" ? " is-active" : ""}`}
        aria-pressed={tab === "store"} disabled={busy} onClick={() => setTab("store")}>皮肤商店</button>
      <button type="button" className={`skin-segment${tab === "account" ? " is-active" : ""}`}
        aria-pressed={tab === "account"} disabled={busy} onClick={openAccount}>账号与订单</button>
    </div>
    <div hidden={tab !== "store"}>
      <SkinStore busy={accountBusy} localCatalogRevision={catalogRevision} onBusyChange={setStoreBusy}
        onOpenAccount={openAccount} focusProduct={focusProduct} />
    </div>
    {accountVisited ? <div hidden={tab !== "account"}>
      <SkinAccountPanel onChange={rereadCatalog} onFindProduct={findProduct} onBusyChange={setAccountBusy} />
    </div> : null}
  </>;
}
