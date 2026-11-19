use crate::keychain::ProviderKeychain;
use crate::skin_store::{protocol, CheckoutInfo, EntitlementError, SkinEntitlements};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Client = Arc<CommerceClient>;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account { pub id: String, pub email: String }

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: String, pub product_id: String, pub amount: u32, pub currency: String,
    pub status: String, pub created: i64, pub updated: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OrderCursor { created: i64, id: String }

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(default)] pub service_url: String,
    pub account: Option<Account>,
    #[serde(default)] pub orders: Vec<Order>,
    #[serde(default)] pub owned_product_ids: BTreeSet<String>,
    pub refreshed_at: Option<i64>,
    #[serde(default)] pub session_only: bool,
    #[serde(default)] pub session_available: bool,
    pub next_order_cursor: Option<OrderCursor>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings { service_url: String }

struct State { snapshot: Snapshot, token: Option<String>, revision: u64 }

pub struct CommerceClient {
    settings_path: PathBuf,
    vault: Arc<dyn ProviderKeychain>,
    state: Mutex<State>,
    operations: Mutex<()>,
}

fn validate_url(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() { return Ok(String::new()); }
    let raw = raw.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(raw).map_err(|_| "服务地址无效。")?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty()
        || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return Err("请输入发布方提供的 HTTPS 服务根地址。".into());
    }
    Ok(raw.into())
}

fn error_message(code: &str) -> String {
    match code {
        "not_signed_in" => "登录已过期，请重新登录。",
        "invalid_email" => "请输入有效的邮箱地址。",
        "invalid_code" | "invalid_or_expired_code" => "验证码不正确或已过期，请重新获取。",
        "try_later" => "操作较频繁，请稍后重试。",
        "email_service_not_configured" => "发布方尚未启用邮箱登录服务。",
        "email_service_unavailable" => "验证码暂时发送失败，请稍后重试。",
        "payment_not_configured" => "发布方尚未开放购买。",
        "price_changed" => "商品价格已更新，请刷新商店后确认新价格。",
        "pending_quote_changed" => "还有一笔旧报价订单，请先在订单页关闭它，再确认新价格。",
        "order_needs_reconciliation" => "这笔订单需要核实，请复制订单号联系发布方，暂不重复下单。",
        "order_already_paid_or_processing" => "这笔订单已有付款结果或正在处理，请先刷新订单确认。",
        "unsupported_platform" => "这款皮肤暂不支持当前系统，请勿购买。",
        "not_owned" => "当前账号没有此皮肤的有效权益，请刷新订单或联系订单支持。",
        "product_not_available" => "这款皮肤暂不可购买或下载。",
        "order_closed" => "这笔订单已结束，请刷新订单后重新选择。",
        _ => "暂时无法连接账号或购买服务，请稍后重试。",
    }.into()
}

pub(crate) fn approved_service_message(message: &str) -> bool {
    ["not_signed_in", "invalid_email", "invalid_code", "try_later", "email_service_not_configured", "email_service_unavailable", "payment_not_configured",
     "price_changed", "pending_quote_changed", "order_needs_reconciliation", "order_already_paid_or_processing", "unsupported_platform", "not_owned", "product_not_available", "order_closed", "unavailable"]
        .iter().any(|code| error_message(code) == message)
}

impl CommerceClient {
    pub fn new(data_dir: PathBuf) -> Self { Self::with_vault(data_dir, crate::skin_credentials::system_vault()) }

    fn with_vault(data_dir: PathBuf, vault: Arc<dyn ProviderKeychain>) -> Self {
        let settings_path = data_dir.join("skin-commerce.json");
        let url = std::fs::symlink_metadata(&settings_path).ok().filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 4096)
            .and_then(|_| std::fs::read(&settings_path).ok()).and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .and_then(|settings| validate_url(&settings.service_url).ok()).unwrap_or_default();
        let token = if url.is_empty() { None } else { vault.get_password(&Self::vault_account(&url)).ok().flatten() };
        Self { settings_path, vault, state: Mutex::new(State { snapshot: Snapshot { service_url: url, ..Snapshot::default() }, token, revision: 0 }), operations: Mutex::new(()) }
    }

    fn vault_account(url: &str) -> String { format!("skin-commerce-{}", protocol::sha256_hex(url.as_bytes())) }

    pub fn cached(&self) -> Result<Snapshot, String> {
        let state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
        let mut snapshot = state.snapshot.clone(); snapshot.session_available = state.token.is_some();
        Ok(snapshot)
    }

    pub fn set_url(&self, raw: &str) -> Result<Snapshot, String> {
        let url = validate_url(raw)?;
        let _operation = self.operations.lock().map_err(|_| "账号正在操作中。")?;
        let mut state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
        if state.snapshot.service_url == url { return Ok(state.snapshot.clone()); }
        if let Some(parent) = self.settings_path.parent() { std::fs::create_dir_all(parent).map_err(|_| "无法保存服务设置。")?; }
        let bytes = serde_json::to_vec(&Settings { service_url: url.clone() }).map_err(|_| "无法保存服务设置。")?;
        let temporary = self.settings_path.with_extension("pending");
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temporary).map_err(|_| "无法保存服务设置，请稍后重试。")?;
        let result = file.write_all(&bytes).and_then(|_| file.sync_all()).and_then(|_| std::fs::rename(&temporary, &self.settings_path));
        if result.is_err() { let _ = std::fs::remove_file(&temporary); return Err("无法保存服务设置。".into()); }
        if !state.snapshot.service_url.is_empty() { let _ = self.vault.delete_password(&Self::vault_account(&state.snapshot.service_url)); }
        state.revision += 1;
        state.token = None;
        state.snapshot = Snapshot { service_url: url, ..Snapshot::default() };
        Ok(state.snapshot.clone())
    }

    fn credentials(&self, authenticated: bool) -> Result<(String, Option<String>, u64), String> {
        let state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
        if state.snapshot.service_url.is_empty() { return Err("请先连接发布方的账号服务。".into()); }
        if authenticated && state.token.is_none() { return Err(error_message("not_signed_in")); }
        Ok((state.snapshot.service_url.clone(), state.token.clone(), state.revision))
    }

    async fn request_async(url: String, token: Option<String>, method: reqwest::Method, route: &str, body: Option<Value>) -> Result<Value, String> {
        let builder = reqwest::Client::builder().https_only(true).redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20));
        #[cfg(debug_assertions)]
        let builder = if reqwest::Url::parse(&url).ok().is_some_and(|url| url.host_str() == Some("127.0.0.1")) {
            if let Some(root) = crate::skin_qa::root()? {
                let path = root.join("service-ca.pem");
                let metadata = std::fs::symlink_metadata(&path).map_err(|_| "本地验证证书缺失。")?;
                if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16384 { return Err("本地验证证书无效。".into()); }
                builder.no_proxy().add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(path).map_err(|_| "本地验证证书缺失。")?).map_err(|_| "本地验证证书无效。")?)
            } else { builder }
        } else { builder };
        let client = builder.build().map_err(|_| "无法连接账号服务。")?;
        let mut request = client.request(method, format!("{url}{route}"));
        if let Some(token) = token { request = request.bearer_auth(token); }
        if let Some(body) = body { request = request.json(&body); }
        let mut response = request.send().await.map_err(|_| "暂时无法连接账号服务，请稍后重试。")?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "账号服务响应中断，请重试。")? {
            if bytes.len() + chunk.len() > 256 * 1024 { return Err("账号服务响应过大。".into()); }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "账号服务响应无效。")?;
        if !status.is_success() { return Err(error_message(value.get("error").and_then(Value::as_str).unwrap_or(""))); }
        Ok(value)
    }

    fn request(&self, authenticated: bool, method: reqwest::Method, route: &str, body: Option<Value>) -> Result<(Value, u64), String> {
        let (url, token, revision) = self.credentials(authenticated)?;
        let route = route.to_string();
        // Entitlement adapters are synchronous and can be called from an async download command.
        // Enter the async runtime on a dedicated worker instead of nesting block_on in Tokio.
        let result = std::thread::spawn(move || tauri::async_runtime::block_on(Self::request_async(url, token, method, &route, body)))
            .join().map_err(|_| "账号服务暂不可用。")?;
        if authenticated && result.as_ref().is_err_and(|error| error == &error_message("not_signed_in")) {
            let mut state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
            if state.revision == revision {
                let _ = self.vault.delete_password(&Self::vault_account(&state.snapshot.service_url));
                state.token = None; state.revision += 1;
                state.snapshot = Snapshot { service_url: state.snapshot.service_url.clone(), ..Snapshot::default() };
            }
        }
        let value = result?;
        Ok((value, revision))
    }

    pub fn request_code(&self, email: &str) -> Result<(), String> {
        self.request(false, reqwest::Method::POST, "/v1/auth/request-code", Some(json!({"email": email})))?;
        Ok(())
    }

    pub fn login(&self, email: &str, code: &str) -> Result<Snapshot, String> {
        let _operation = self.operations.lock().map_err(|_| "账号正在操作中。")?;
        let (value, revision) = self.request(false, reqwest::Method::POST, "/v1/auth/verify-code", Some(json!({"email": email, "code": code})))?;
        let token = value.get("sessionToken").and_then(Value::as_str).filter(|token| token.starts_with("zsc_") && token.len() <= 128).ok_or("登录服务响应无效。")?.to_string();
        let account: Account = serde_json::from_value(value.get("account").cloned().ok_or("登录服务响应无效。")?).map_err(|_| "登录服务响应无效。")?;
        {
            let mut state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
            if state.revision != revision { return Err("账号设置已改变，请重新登录。".into()); }
            let session_only = self.vault.set_password(&Self::vault_account(&state.snapshot.service_url), &token).is_err();
            state.revision += 1;
            state.token = Some(token);
            state.snapshot = Snapshot { service_url: state.snapshot.service_url.clone(), account: Some(account), session_only, ..Snapshot::default() };
        }
        self.refresh()
    }

    pub fn refresh(&self) -> Result<Snapshot, String> {
        let (value, revision) = self.request(true, reqwest::Method::POST, "/v1/orders/refresh", Some(json!({})))?;
        let mut fresh: Snapshot = serde_json::from_value(value).map_err(|_| "订单响应无效。")?;
        let mut state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
        if state.revision != revision { return Err("账号已切换，请刷新当前账号。".into()); }
        fresh.service_url = state.snapshot.service_url.clone(); fresh.session_only = state.snapshot.session_only; fresh.session_available = state.token.is_some();
        state.snapshot = fresh.clone();
        Ok(fresh)
    }

    pub fn logout(&self) -> Result<Snapshot, String> {
        let _operation = self.operations.lock().map_err(|_| "账号正在操作中。")?;
        let remote_result = self.request(true, reqwest::Method::POST, "/v1/auth/logout", Some(json!({})));
        let mut state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
        let delete_result = self.vault.delete_password(&Self::vault_account(&state.snapshot.service_url));
        state.revision += 1; state.token = None;
        state.snapshot = Snapshot { service_url: state.snapshot.service_url.clone(), ..Snapshot::default() };
        if delete_result.is_err() { return Err("已退出本次会话，但系统凭据未能清除，请检查系统钥匙串。".into()); }
        if remote_result.is_err() { return Err("已退出本机；服务暂时不可达，远端会话将在到期后失效。".into()); }
        Ok(state.snapshot.clone())
    }

    pub fn resume(&self, order_id: &str) -> Result<String, String> {
        let value = self.request(true, reqwest::Method::POST, "/v1/orders/resume", Some(json!({"orderId": order_id})))?.0;
        let url = value.get("checkoutUrl").and_then(Value::as_str).ok_or("订单暂不可继续付款，请刷新订单。")?;
        validate_checkout_url(url)?;
        Ok(url.into())
    }

    pub fn more_orders(&self) -> Result<Snapshot, String> {
        let cursor = self.cached()?.next_order_cursor;
        if cursor.is_none() { return self.cached(); }
        let (value, revision) = self.request(true, reqwest::Method::POST, "/v1/orders/page", Some(json!({"cursor": cursor})))?;
        #[derive(Deserialize)] #[serde(rename_all="camelCase")]
        struct Page { orders: Vec<Order>, next_order_cursor: Option<OrderCursor> }
        let page: Page = serde_json::from_value(value).map_err(|_| "订单响应无效。")?;
        let mut state = self.state.lock().map_err(|_| "账号状态暂不可用。")?;
        if state.revision != revision { return Err("账号已切换，请刷新当前账号。".into()); }
        for order in page.orders { if !state.snapshot.orders.iter().any(|existing| existing.id == order.id) { state.snapshot.orders.push(order); } }
        state.snapshot.next_order_cursor = page.next_order_cursor;
        Ok(state.snapshot.clone())
    }

    pub fn cancel(&self, order_id: &str) -> Result<Snapshot, String> {
        self.request(true, reqwest::Method::POST, "/v1/orders/cancel", Some(json!({"orderId": order_id})))?;
        self.refresh()
    }
}

impl SkinEntitlements for CommerceClient {
    fn owns(&self, product_id: &str) -> Result<bool, EntitlementError> {
        let state = self.state.lock().map_err(|_| EntitlementError::Network(String::new()))?;
        if state.snapshot.service_url.is_empty() { return Err(EntitlementError::NotConfigured); }
        if state.token.is_none() { return Err(EntitlementError::NotSignedIn); }
        Ok(state.snapshot.owned_product_ids.contains(product_id))
    }

    fn purchase(&self, product_id: &str, price: u32, currency: &str) -> Result<CheckoutInfo, EntitlementError> {
        let _ = self.owns(product_id)?;
        let mut random = [0u8; 24];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random).map_err(|_| EntitlementError::Network(String::new()))?;
        let request_key = protocol::sha256_hex(&random);
        let value = self.request(true, reqwest::Method::POST, "/v1/checkout", Some(json!({"productId": product_id, "requestKey": request_key,
            "expectedAmount": price, "currency": currency, "platform": std::env::consts::OS})))
            .map_err(EntitlementError::ServiceMessage)?.0;
        let info: CheckoutInfo = serde_json::from_value(value).map_err(|_| EntitlementError::Network(String::new()))?;
        if let Some(url) = &info.checkout_url { validate_checkout_url(url).map_err(EntitlementError::Declined)?; }
        if info.owned { self.refresh().map_err(EntitlementError::Declined)?; }
        Ok(info)
    }

    fn authorized_bundle_url(&self, product_id: &str) -> Result<String, EntitlementError> {
        let value = self.request(true, reqwest::Method::POST, "/v1/download-ticket", Some(json!({"productId": product_id}))).map_err(EntitlementError::Declined)?.0;
        let url = value.get("url").and_then(Value::as_str).ok_or(EntitlementError::Network(String::new()))?;
        let base = self.credentials(true).map_err(EntitlementError::Declined)?.0;
        protocol::validate_download_url(url).map_err(EntitlementError::Declined)?;
        if !url.starts_with(&format!("{base}/v1/download/dl_")) { return Err(EntitlementError::Declined(String::new())); }
        Ok(url.into())
    }
}

pub fn validate_checkout_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "付款页面地址无效。")?;
    if url.scheme() != "https" || url.host_str() != Some("checkout.stripe.com") || !url.username().is_empty()
        || url.password().is_some() || url.port().is_some_and(|port| port != 443) {
        return Err("付款页面地址未通过校验。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary() -> PathBuf {
        let root = std::env::temp_dir().join(format!("zhizi-commerce-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap(); root
    }
    #[test]
    fn skin_service_configuration_is_opt_in_and_never_persists_credentials_as_json() {
        let root = temporary();
        let vault = Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let client = CommerceClient::with_vault(root.clone(), vault.clone());
        assert!(client.cached().unwrap().service_url.is_empty());
        for url in ["http://example.invalid", "https://example.invalid/path", "https://user:pass@example.invalid", "https://example.invalid?token=private", "file:///tmp"] {
            assert!(client.set_url(url).is_err());
        }
        client.set_url("https://skins.example.invalid").unwrap();
        vault.set_password(&CommerceClient::vault_account("https://skins.example.invalid"), "zsc_private_fixture").unwrap();
        let restored = CommerceClient::with_vault(root.clone(), vault.clone());
        assert!(restored.cached().unwrap().session_available);
        assert!(!std::fs::read_to_string(root.join("skin-commerce.json")).unwrap().contains("private"));
        restored.set_url("https://other.example.invalid").unwrap();
        assert!(!restored.cached().unwrap().session_available);
        assert!(vault.get_password(&CommerceClient::vault_account("https://skins.example.invalid")).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn skin_checkout_opener_rejects_untrusted_schemes_hosts_and_credentials() {
        validate_checkout_url("https://checkout.stripe.com/c/pay/cs_test_example").unwrap();
        for url in ["http://checkout.stripe.com/c/pay/x", "https://checkout.stripe.com.evil.invalid/x", "https://user@checkout.stripe.com/x", "file:///private/data", "javascript:alert(1)", "https://checkout.stripe.com:8443/x"] {
            assert!(validate_checkout_url(url).is_err());
        }
    }

    #[test]
    #[ignore = "requires an explicitly started local HTTPS service fixture"]
    fn skin_account_real_https_transport_restores_orders_and_rejects_revoked_downloads() {
        let root = crate::skin_qa::root().unwrap().expect("isolated QA profile required");
        let endpoint: Value = serde_json::from_slice(&std::fs::read(root.join("endpoint.json")).unwrap()).unwrap();
        let url = endpoint["url"].as_str().unwrap();
        let vault = Arc::new(crate::keychain::MemoryProviderKeychain::default());
        let client = CommerceClient::with_vault(root.join("Client"), vault.clone());
        client.set_url(url).unwrap();
        client.request_code("transport-fixture@example.invalid").unwrap();
        let outbox: Value = serde_json::from_slice(&std::fs::read(root.join("outbox.json")).unwrap()).unwrap();
        let signed_in = client.login("transport-fixture@example.invalid", outbox["code"].as_str().unwrap()).unwrap();
        assert!(signed_in.account.is_some());
        assert!(!client.owns("skin.test").unwrap());
        let checkout = client.purchase("skin.test", 1200, "CNY").unwrap();
        assert!(!checkout.owned);
        let order_id = checkout.order_id.clone().unwrap();
        assert_eq!(client.purchase("skin.test", 1200, "CNY").unwrap().order_id.as_deref(), Some(order_id.as_str()));
        assert!(!client.owns("skin.test").unwrap());
        client.request(true, reqwest::Method::POST, "/_test/payment", Some(json!({"orderId": order_id}))).unwrap();
        client.refresh().unwrap();
        assert!(client.owns("skin.test").unwrap());
        let ticket = client.authorized_bundle_url("skin.test").unwrap();
        let route = ticket.strip_prefix(url).unwrap();
        assert_eq!(client.request(true, reqwest::Method::GET, route, None).unwrap().0["fixture"], "local-https-transport-only");
        let restored = CommerceClient::with_vault(root.join("Client"), vault);
        assert!(restored.cached().unwrap().session_available);
        assert_eq!(restored.refresh().unwrap().account.unwrap().id, signed_in.account.unwrap().id);
        assert!(restored.owns("skin.test").unwrap());
        restored.request(true, reqwest::Method::POST, "/_test/payment", Some(json!({"orderId": order_id, "refunded": true}))).unwrap();
        let refunded = restored.refresh().unwrap();
        assert_eq!(refunded.orders[0].status, "refunded");
        assert!(!restored.owns("skin.test").unwrap());
        assert!(restored.authorized_bundle_url("skin.test").is_err());
        assert!(client.request(true, reqwest::Method::GET, route, None).is_err());
        restored.logout().unwrap();
        assert!(!restored.cached().unwrap().session_available);
        let report = json!({"ok": true, "scope": "real Rust client to local Python HTTPS fixture; synthetic payment transport, not a Stripe sandbox or real transaction",
            "checks": ["email code login", "pending checkout is not owned", "retry reuses order", "paid-order refresh", "authorized HTTPS download", "restart restores account", "refund revokes new and existing tickets", "logout"]});
        std::fs::write(root.join("TRANSPORT-ACCEPTANCE.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
