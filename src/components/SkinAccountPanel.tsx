import { useEffect, useRef, useState } from "react";
import { cachedSkinAccount, cancelSkinOrder, emptySkinAccount, loginSkinAccount, logoutSkinAccount, moreSkinOrders, refreshSkinAccount, requestSkinLoginCode,
  resumeSkinOrder, setSkinAccountService, type SkinAccountSnapshot } from "../lib/skinCommerce";
import { formatSkinMoney, skinOrderHelp, skinOrderLabel } from "../domain/skinOrders";
import { copyText } from "../lib/clipboard";

type Props = { onChange: () => void; onFindProduct: (id: string) => void; onBusyChange: (busy: boolean) => void };

export function SkinAccountPanel({ onChange, onFindProduct, onBusyChange }: Props) {
  const [snapshot, setSnapshot] = useState<SkinAccountSnapshot>(emptySkinAccount);
  const [url, setUrl] = useState("");
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [cooldown, setCooldown] = useState(0);
  const [cancelId, setCancelId] = useState<string | null>(null);
  const operation = useRef(false);
  const mounted = useRef(false);
  const notifyBusy = useRef(onBusyChange); notifyBusy.current = onBusyChange;
  useEffect(() => {
    mounted.current = true;
    void cachedSkinAccount().then((value) => { if (mounted.current) { setSnapshot(value); setUrl(value.serviceUrl); } }).catch((error) => { if (mounted.current) setError(String(error)); });
    return () => { mounted.current = false; notifyBusy.current(false); };
  }, []);
  useEffect(() => { if (cooldown <= 0) return; const timer = setTimeout(() => setCooldown((value) => Math.max(0, value - 1)), 1000); return () => clearTimeout(timer); }, [cooldown]);
  async function act(work: () => Promise<void>) {
    if (operation.current) return;
    operation.current = true; setBusy(true); notifyBusy.current(true); setError(""); setMessage("");
    try { await work(); }
    catch (error) {
      if (mounted.current) setError(error instanceof Error ? error.message : String(error));
      try { const value = await cachedSkinAccount(); if (mounted.current) { setSnapshot(value); onChange(); } } catch { /* Preserve the last readable account state. */ }
    } finally { operation.current = false; if (mounted.current) { setBusy(false); notifyBusy.current(false); } }
  }
  async function refresh() {
    const value = await refreshSkinAccount(); if (!mounted.current) return;
    setSnapshot(value); onChange(); setMessage("订单和已购皮肤已刷新。");
  }
  return <section className="skin-account" aria-label="皮肤账号与订单">
    <div className="skin-account-heading"><div><h3>账号与订单</h3><p>同一账号登录后可恢复已购皮肤；安装好的皮肤可离线使用。</p></div>
      {snapshot.account ? <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => { const value = await logoutSkinAccount(); if (mounted.current) { setSnapshot(value); setCode(""); onChange(); setMessage("已退出账号。"); } })}>退出登录</button> : null}</div>
    <details className="skin-account-connection"><summary>服务连接设置</summary>
      <p>填写发布方提供的账号服务地址。更换服务会退出当前账号。</p>
      <label className="field-label" htmlFor="skin-service-url">账号服务地址</label>
      <div className="skin-account-connection-controls"><input id="skin-service-url" type="url" value={url} placeholder="https://皮肤服务地址" disabled={busy} onChange={(event) => setUrl(event.currentTarget.value)} />
        <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => { const value = await setSkinAccountService(url); if (mounted.current) { setSnapshot(value); setUrl(value.serviceUrl); setCode(""); onChange(); setMessage("服务地址已保存。"); } })}>保存连接</button></div>
    </details>
    {!snapshot.serviceUrl ? <p className="skin-account-notice">皮肤购买服务尚未连接。连接发布方服务后可登录、购买及恢复订单。</p> : null}
    {snapshot.account ? <>
      <p className="skin-account-identity">已登录：<strong>{snapshot.account.email}</strong></p>
      {snapshot.sessionOnly ? <p role="status">系统凭据存储暂不可用，本次退出后需重新登录。</p> : null}
      <button type="button" className="primary-button" disabled={busy} onClick={() => void act(refresh)}>刷新订单 / 恢复购买</button>
      {snapshot.refreshedAt ? <small> 最近刷新：{new Date(snapshot.refreshedAt * 1000).toLocaleString("zh-CN")}</small> : null}
      <h4>已购皮肤 · {snapshot.ownedProductIds.length}</h4>
      {snapshot.ownedProductIds.length ? <div className="skin-owned-list">{snapshot.ownedProductIds.map((id) => <div key={id}><span>{id}</span><button type="button" className="secondary-button" disabled={busy} onClick={() => onFindProduct(id)}>查看 / 重新下载</button></div>)}</div> : <p>当前账号暂无已购皮肤。已付款但未显示时，可先刷新订单。</p>}
      <h4>订单记录</h4>
      {!snapshot.orders.length ? <p>暂无订单记录。</p> : <div className="skin-orders">{snapshot.orders.map((order) => <article className="skin-order" key={order.id}>
        <div className="skin-account-heading"><strong>{order.productId}</strong><span>{formatSkinMoney(order.amount, order.currency)} · {skinOrderLabel(order.status)}</span></div>
        <p>{skinOrderHelp(order.status)}</p><small>{new Date(order.created * 1000).toLocaleString("zh-CN")} · 订单号 {order.id}</small>
        <div className="skin-account-actions"><button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => { if (!await copyText(order.id)) throw new Error("订单号未能复制，请手动复制订单记录中的编号。"); setMessage("订单号已复制，可用于订单支持。"); })}>复制订单号</button>
          {order.status === "pending" || order.status === "creating" ? <button type="button" className="primary-button" disabled={busy} onClick={() => void act(async () => { await resumeSkinOrder(order.id); setMessage("已打开付款页面，完成后请刷新订单确认。"); })}>继续付款</button> : null}
          {order.status === "pending" ? <button type="button" className="secondary-button" disabled={busy} onClick={() => setCancelId(order.id)}>关闭未付款订单</button> : null}
          {order.status === "paid" && snapshot.ownedProductIds.includes(order.productId) ? <button type="button" className="secondary-button" disabled={busy} onClick={() => onFindProduct(order.productId)}>获取皮肤</button> : null}
        </div>
      </article>)}</div>}
      {snapshot.nextOrderCursor ? <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => { const value = await moreSkinOrders(); if (mounted.current) setSnapshot(value); })}>加载更早的订单</button> : null}
      {cancelId ? <div className="skin-store-confirm" role="group" aria-label="关闭未付款订单确认"><p>确认关闭订单 {cancelId}？当前付款页面随后将失效。</p>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(async () => { const value = await cancelSkinOrder(cancelId); if (mounted.current) { setSnapshot(value); setCancelId(null); onChange(); setMessage("未付款订单已关闭。"); } })}>确认关闭</button>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => setCancelId(null)}>保留订单</button></div> : null}
    </> : <form className="skin-account-login" onSubmit={(event) => { event.preventDefault(); void act(async () => { const value = await loginSkinAccount(email.trim(), code.trim()); if (mounted.current) { setSnapshot(value); setCode(""); onChange(); setMessage("已登录，已购皮肤已恢复。"); } }); }}>
      <label className="field-label" htmlFor="skin-email">邮箱</label><input id="skin-email" type="email" autoComplete="email" required disabled={busy || !snapshot.serviceUrl} value={email} onChange={(event) => setEmail(event.currentTarget.value)} />
      <div className="skin-account-connection-controls"><button type="button" className="secondary-button" disabled={busy || cooldown > 0 || !snapshot.serviceUrl || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/u.test(email.trim())} onClick={() => void act(async () => { await requestSkinLoginCode(email.trim()); setCooldown(60); setMessage("验证码已发送，10 分钟内有效。"); })}>{cooldown > 0 ? `${cooldown} 秒后重发` : "发送验证码"}</button></div>
      <label className="field-label" htmlFor="skin-code">6 位验证码</label><input id="skin-code" type="text" inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" required maxLength={6} disabled={busy || !snapshot.serviceUrl} value={code} onChange={(event) => setCode(event.currentTarget.value)} />
      <div className="skin-account-actions"><button className="primary-button" type="submit" disabled={busy || !snapshot.serviceUrl || !/^[0-9]{6}$/u.test(code)}>登录 / 恢复已购</button>
        {snapshot.sessionAvailable ? <button type="button" className="secondary-button" disabled={busy} onClick={() => void act(refresh)}>恢复上次登录</button> : null}</div>
    </form>}
    <details className="skin-order-help"><summary>订单帮助</summary><p>付款后请刷新订单确认。下载中断后可在已购皮肤中重新获取，重复刷新不会再次扣款。退款或权益撤销后会暂停新下载，已安装的皮肤可继续离线使用。如需联系发布方，请复制订单号。</p></details>
    {message ? <p role="status" className="settings-message">{message}</p> : null}{error ? <p role="alert" className="skin-account-error">{error}</p> : null}
  </section>;
}
