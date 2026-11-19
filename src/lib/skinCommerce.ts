import { isTauriRuntime } from "./tauriRuntime";
export type SkinAccount = { id: string; email: string };
export type SkinOrder = { id: string; productId: string; amount: number; currency: string; status: string; created: number; updated: number };
export type SkinAccountSnapshot = {
  serviceUrl: string; account: SkinAccount | null; orders: SkinOrder[]; ownedProductIds: string[];
  refreshedAt: number | null; sessionOnly: boolean; sessionAvailable: boolean;
  nextOrderCursor: {created: number; id: string} | null;
};
export const emptySkinAccount: SkinAccountSnapshot = { serviceUrl: "", account: null, orders: [], ownedProductIds: [], refreshedAt: null, sessionOnly: false, sessionAvailable: false, nextOrderCursor: null };
async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) throw new Error("请在知字管理台中操作账号与订单。");
  const api = await import("@tauri-apps/api/core");
  return api.invoke<T>(command, args);
}
export const cachedSkinAccount = () => isTauriRuntime() ? invoke<SkinAccountSnapshot>("skin_account_cached") : Promise.resolve(emptySkinAccount);
export const setSkinAccountService = (url: string) => invoke<SkinAccountSnapshot>("skin_account_set_service", { url });
export const requestSkinLoginCode = (email: string) => invoke<void>("skin_account_request_code", { email });
export const loginSkinAccount = (email: string, code: string) => invoke<SkinAccountSnapshot>("skin_account_login", { email, code });
export const refreshSkinAccount = () => invoke<SkinAccountSnapshot>("skin_account_refresh");
export const logoutSkinAccount = () => invoke<SkinAccountSnapshot>("skin_account_logout");
export const resumeSkinOrder = (orderId: string) => invoke<void>("skin_account_resume", { orderId });
export const moreSkinOrders = () => invoke<SkinAccountSnapshot>("skin_account_more_orders");
export const cancelSkinOrder = (orderId: string) => invoke<SkinAccountSnapshot>("skin_account_cancel", {orderId});
