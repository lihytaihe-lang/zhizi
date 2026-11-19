import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { SkinAccountPanel } from "../components/SkinAccountPanel";
import { emptySkinAccount, type SkinAccountSnapshot } from "../lib/skinCommerce";
import { findButton, installMiniDom, MiniEvent, MiniMouseEvent, type MiniElement } from "./support/miniDom";

async function click(element: MiniElement | null) {
  assert.ok(element);
  await act(async () => { element.dispatchEvent(new MiniMouseEvent("click", {bubbles:true,cancelable:true})); await new Promise((resolve) => setTimeout(resolve, 0)); });
}
async function mount(handler: (command: string, args?: Record<string, unknown>) => unknown,
  check: (container: MiniElement, calls: string[], found: string[]) => Promise<void>) {
  const dom = installMiniDom(); const calls: string[] = [], found: string[] = [];
  const runtime = window as unknown as { __TAURI_INTERNALS__?: {invoke: (command:string,args?:Record<string,unknown>) => Promise<unknown>} };
  runtime.__TAURI_INTERNALS__ = {invoke: async (command,args) => { calls.push(command); return handler(command,args); }};
  (globalThis as unknown as {IS_REACT_ACT_ENVIRONMENT:boolean}).IS_REACT_ACT_ENVIRONMENT = true;
  const { createRoot } = await import("react-dom/client"); const root = createRoot(dom.container as unknown as Element);
  try {
    await act(async () => { root.render(<SkinAccountPanel onChange={() => {}} onBusyChange={() => {}} onFindProduct={(id) => found.push(id)} />); });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve,0)); });
    await check(dom.container,calls,found);
  } finally { await act(async () => { root.unmount(); }); delete runtime.__TAURI_INTERNALS__; dom.cleanup(); }
}

test("account view reads cached status without starting authentication or payment", async () => {
  await mount(() => emptySkinAccount, async (container,calls) => {
    assert.deepEqual(calls,["skin_account_cached"]);
    assert.match(container.textContent,/购买服务尚未连接/);
    assert.equal(findButton(container,"发送验证码")?.disabled,true);
    assert.equal(findButton(container,"登录 / 恢复已购")?.disabled,true);
  });
});
test("restored order view separates continuing payment, valid downloads and revoked orders", async () => {
  const snapshot: SkinAccountSnapshot = { ...emptySkinAccount, serviceUrl:"https://skins.example.invalid", account:{id:"acct_fixture",email:"fixture@example.invalid"}, ownedProductIds:["skin.paid"], sessionAvailable:true,
    orders:[{id:"ord_pending",productId:"skin.pending",amount:1200,currency:"CNY",status:"pending",created:1,updated:1},
      {id:"ord_paid",productId:"skin.paid",amount:1200,currency:"CNY",status:"paid",created:1,updated:1},
      {id:"ord_refunded",productId:"skin.refunded",amount:1200,currency:"CNY",status:"refunded",created:1,updated:1}] };
  await mount((command,args) => {
    if(command==="skin_account_resume") {assert.equal(args?.orderId,"ord_pending");return;}
    if(command==="skin_account_cancel") {assert.equal(args?.orderId,"ord_pending");return {...snapshot,orders:snapshot.orders.map((order)=>order.id==="ord_pending"?{...order,status:"expired"}:order)};}
    return snapshot;
  }, async(container,calls,found) => {
    assert.equal(container.querySelectorAll(".skin-order").length,3);
    assert.match(container.textContent,/等待付款/);assert.match(container.textContent,/已退款/);
    await click(findButton(container,"继续付款"));assert.ok(calls.includes("skin_account_resume"));
    await click(findButton(container,"关闭未付款订单"));
    assert.ok(!calls.includes("skin_account_cancel"));
    await click(findButton(container,"保留订单"));
    await click(findButton(container,"关闭未付款订单"));await click(findButton(container,"确认关闭"));
    assert.ok(calls.includes("skin_account_cancel"));assert.equal(findButton(container,"继续付款"),null);
    const refunded=container.querySelectorAll(".skin-order").find((row)=>row.textContent.includes("ord_refunded"));assert.ok(refunded);
    assert.equal(findButton(refunded,"获取皮肤"),null);
    await click(findButton(container,"获取皮肤"));assert.deepEqual(found,["skin.paid"]);
    assert.ok(!calls.includes("skin_store_purchase"));
  });
});
test("login submits an explicit code without creating any order", async () => {
  let submitted: Record<string,unknown>|undefined;
  const snapshot={...emptySkinAccount,serviceUrl:"https://skins.example.invalid"};
  await mount((command,args)=>{if(command==="skin_account_login"){submitted=args;return {...snapshot,account:{id:"acct_test",email:"fixture@example.invalid"}};}return snapshot;},async(container,calls)=>{
    const email=container.querySelector("#skin-email"),code=container.querySelector("#skin-code");assert.ok(email&&code);
    await act(async()=>{email.value="fixture@example.invalid";email.dispatchEvent(new MiniEvent("input",{bubbles:true}));code.value="123456";code.dispatchEvent(new MiniEvent("input",{bubbles:true}));});
    const form=container.querySelector("form");assert.ok(form);
    await act(async()=>{form.dispatchEvent(new MiniEvent("submit",{bubbles:true,cancelable:true}));await new Promise(resolve=>setTimeout(resolve,0));});
    assert.deepEqual(submitted,{email:"fixture@example.invalid",code:"123456"});
    assert.ok(!calls.includes("skin_store_purchase"));assert.match(container.textContent,/已登录/);
  });
});
