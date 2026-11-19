import assert from "node:assert/strict";
import { test } from "node:test";
import { formatSkinMoney, skinOrderHelp, skinOrderLabel } from "../domain/skinOrders";
test("order copy never treats an unknown or pending state as payment proof", () => {
  for (const unknown of ["returned", "__proto__", "toString"]) assert.equal(skinOrderLabel(unknown), "状态待确认");
  assert.match(skinOrderHelp("pending"), /刷新订单/);
  for (const status of ["refunded", "disputed", "revoked"]) assert.match(skinOrderHelp(status), /暂停.*离线/);
});
test("prices preserve currency minor units and reject malformed amounts", () => {
  assert.equal(formatSkinMoney(1234, "cny"), "¥12.34");
  assert.match(formatSkinMoney(1200, "JPY"), /1,200/u);
  assert.match(formatSkinMoney(1200, "USD"), /12/u);
  for (const value of [-1, NaN, Infinity, 1.5]) assert.equal(formatSkinMoney(value, "CNY"), "价格待确认");
  assert.equal(formatSkinMoney(10, "INVALID"), "价格待确认");
});
