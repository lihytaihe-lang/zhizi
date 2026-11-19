export type SkinOrderStatus = "creating"|"pending"|"paid"|"failed"|"expired"|"refunded"|"disputed"|"revoked";

const SKIN_ORDER_LABELS: Record<string, string> = {
  creating: "正在创建订单",
  pending: "等待付款",
  paid: "已付款",
  failed: "付款未完成",
  expired: "订单已过期",
  refunded: "已退款",
  disputed: "付款争议处理中",
  revoked: "权益已撤销",
};

export function skinOrderLabel(status: string): string {
  return Object.prototype.hasOwnProperty.call(SKIN_ORDER_LABELS, status) ? SKIN_ORDER_LABELS[status] : "状态待确认";
}

const SKIN_ORDER_HELP: Record<string, string> = {
  creating: "订单正在创建，请稍候后刷新查看。",
  pending: "请在支付页面完成付款，然后返回刷新订单查看结果。",
  paid: "付款成功，可随时重新下载或恢复权益。",
  failed: "付款未完成，重新发起支付会创建新订单。",
  expired: "订单已过期，重新下单即可创建新订单。",
  refunded: "订单已退款，重新下载已暂停，可联系订单客服；已安装皮肤可继续离线使用。",
  disputed: "付款争议处理中，重新下载已暂停，可联系订单客服；已安装皮肤可继续离线使用。",
  revoked: "权益已撤销，重新下载已暂停，可联系订单客服；已安装皮肤可继续离线使用。",
};

export function skinOrderHelp(status: string): string {
  return Object.prototype.hasOwnProperty.call(SKIN_ORDER_HELP, status) ? SKIN_ORDER_HELP[status] : "请刷新订单状态，若仍有问题请联系订单客服。";
}

const SKIN_CURRENCIES = new Set(["CNY", "USD", "HKD", "EUR", "GBP", "JPY"]);

export function formatSkinMoney(amountCents: number, currency: string): string {
  const cur = currency.toUpperCase();
  if (!Number.isFinite(amountCents) || !Number.isInteger(amountCents) || amountCents < 0 || !SKIN_CURRENCIES.has(cur)) {
    return "价格待确认";
  }
  const zeroDecimal = cur === "JPY";
  const digits = zeroDecimal || amountCents % 100 === 0 ? 0 : 2;
  return new Intl.NumberFormat("zh-CN", {
    style: "currency",
    currency: cur,
    minimumFractionDigits: digits,
    maximumFractionDigits: zeroDecimal ? 0 : 2,
  }).format(zeroDecimal ? amountCents : amountCents / 100);
}