use crate::hotword::normalize_pinyin;
use chrono::{DateTime, NaiveDate};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::fmt;

const PUBLIC_KEY_HEX: &str = include_str!("../resources/hotword_pack_pubkey.hex");
const MAX_PACK_BYTES: usize = 2 * 1024 * 1024;
const PACK_SCHEMA: &str = "agent-ime-hotword-pack/1";
const LATEST_SCHEMA: &str = "agent-ime-hotword-latest/1";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub schema: String,
    pub pack_id: String,
    pub generated_at: String,
    pub valid_until: String,
    pub producer: String,
    pub entry_count: usize,
    pub entries: Vec<PackEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackEntry {
    pub text: String,
    pub pinyin: String,
    pub definition: String,
    pub domain: String,
    pub confidence_millis: i64,
    pub expires_at_ms: Option<i64>,
    pub first_seen_at: String,
    pub sources: Vec<PackSource>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackSource {
    pub name: String,
    pub url: String,
    pub license: String,
    pub collected_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Latest {
    pub schema: String,
    pub pack_id: String,
    pub url: String,
    pub sha256: String,
    pub generated_at: String,
}

// Reject duplicate keys instead of giving a signature two possible meanings.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ValueVisitor;
        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("JSON with unique keys and integer numbers")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Err(E::custom("不接受小数"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(value.into())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(value)))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(de::Error::custom("重复字段"));
                    }
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(ValueVisitor)
    }
}

pub(crate) fn canonical_bytes(value: &Value) -> Result<Vec<u8>, String> {
    fn ordered(value: &Value) -> Result<Value, String> {
        match value {
            Value::Object(map) => {
                let mut keys = map.keys().collect::<Vec<_>>();
                keys.sort();
                let mut result = Map::new();
                for key in keys {
                    result.insert(key.clone(), ordered(&map[key])?);
                }
                Ok(Value::Object(result))
            }
            Value::Array(values) => values
                .iter()
                .map(ordered)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Number(value) if !value.is_i64() && !value.is_u64() => {
                Err("规范化 JSON 不接受小数。".into())
            }
            _ => Ok(value.clone()),
        }
    }
    let mut bytes =
        serde_json::to_vec(&ordered(value)?).map_err(|_| "无法规范化词包 JSON。".to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode_hex(value: &str, byte_count: usize) -> Result<Vec<u8>, String> {
    let value = value.trim();
    if value.len() != byte_count * 2 || !value.bytes().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("签名校验失败：签名或公钥格式无效。".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "签名校验失败：十六进制格式无效。".into())
        })
        .collect()
}

fn signed_value(bytes: &[u8], sig: &str, key_hex: &str) -> Result<Value, String> {
    if bytes.len() > MAX_PACK_BYTES {
        return Err("签名校验失败：文件超过 2 MiB，整包已丢弃。".into());
    }
    let signature = decode_hex(sig, 64)?;
    let key = decode_hex(key_hex, 32)?;
    let value = serde_json::from_slice::<UniqueValue>(bytes)
        .map_err(|_| "签名校验失败：JSON 格式无效、字段重复或包含小数。".to_string())?
        .0;
    let normalized = canonical_bytes(&value)?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&normalized, &signature)
        .map_err(|_| "签名校验失败，整包已丢弃。".to_string())?;
    Ok(value)
}

pub(crate) fn parse_timestamp_ms(value: &str) -> Result<i64, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.timestamp_millis())
        .map_err(|_| "日期必须是带时区的 RFC3339 时间。".into())
}

fn required_string<'a>(value: &'a Value, key: &str, label: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label}必须为字符串。"))
}

fn nonempty(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label}不能为空。"));
    }
    Ok(())
}

fn validate_pack_value(value: Value, now_ms: i64) -> Result<Pack, String> {
    if value.get("schema").and_then(Value::as_str) != Some(PACK_SCHEMA) {
        return Err("不支持的词包 schema。".into());
    }
    let valid_until = required_string(&value, "validUntil", "词包有效期")?;
    if parse_timestamp_ms(valid_until)? <= now_ms {
        return Err("词包已过期，整包已丢弃。".into());
    }
    let count = value
        .get("entryCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| "词包条数必须为非负整数。".to_string())?;
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "词包 entries 必须为数组。".to_string())?;
    if count > 2_000 || count != entries.len() as u64 {
        return Err("词包条数不一致或超过 2,000 条，整包已丢弃。".into());
    }
    // Strong deserialization starts only after signature, schema, expiry and count.
    let mut pack: Pack = serde_json::from_value(value)
        .map_err(|_| "词包字段缺失或类型无效，整包已丢弃。".to_string())?;
    nonempty(&pack.pack_id, "词包 ID")?;
    nonempty(&pack.producer, "生成器")?;
    parse_timestamp_ms(&pack.generated_at)?;
    for (index, entry) in pack.entries.iter_mut().enumerate() {
        validate_entry(entry)
            .map_err(|reason| format!("第 {} 条：{reason}整包已丢弃。", index + 1))?;
        entry.pinyin = normalize_pinyin(&entry.pinyin);
    }
    Ok(pack)
}

fn validate_entry(entry: &PackEntry) -> Result<(), String> {
    if !(1..=16).contains(&entry.text.chars().count())
        || entry.text.trim().is_empty()
        || entry.text.chars().any(char::is_control)
    {
        return Err("词条必须为 1–16 个字符，且不能含控制字符。".into());
    }
    if !entry
        .pinyin
        .bytes()
        .all(|ch| ch.is_ascii_lowercase() || ch == b' ' || ch == b'\'')
        || !entry.pinyin.bytes().any(|ch| ch.is_ascii_lowercase())
    {
        return Err("拼音只能使用小写字母、空格和撇号。".into());
    }
    if entry.definition.chars().count() > 120 {
        return Err("释义不能超过 120 字。".into());
    }
    if !(0..=1_000).contains(&entry.confidence_millis) {
        return Err("可信度必须为 0–1000 的整数。".into());
    }
    if entry.expires_at_ms.is_some_and(|value| value < 0) {
        return Err("词条过期时间必须为非负毫秒整数。".into());
    }
    if entry.first_seen_at.len() != 10
        || NaiveDate::parse_from_str(&entry.first_seen_at, "%Y-%m-%d").is_err()
    {
        return Err("首次收录日期必须为 YYYY-MM-DD。".into());
    }
    if entry.sources.is_empty() {
        return Err("每条词至少需要一个来源。".into());
    }
    for source in &entry.sources {
        nonempty(&source.name, "来源名")?;
        nonempty(&source.license, "许可")?;
        let url = reqwest::Url::parse(&source.url)
            .map_err(|_| "来源 URL 必须是有效 HTTPS 地址。".to_string())?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || source
                .url
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err("来源 URL 必须是有效 HTTPS 地址。".into());
        }
        parse_timestamp_ms(&source.collected_at)?;
    }
    Ok(())
}

pub fn verify_pack(bytes: &[u8], sig: &str, now_ms: i64) -> Result<Pack, String> {
    validate_pack_value(signed_value(bytes, sig, PUBLIC_KEY_HEX)?, now_ms)
}

fn validate_latest_value(value: Value) -> Result<Latest, String> {
    if value.get("schema").and_then(Value::as_str) != Some(LATEST_SCHEMA) {
        return Err("不支持的热词指针 schema。".into());
    }
    let latest: Latest =
        serde_json::from_value(value).map_err(|_| "热词指针字段缺失或类型无效。".to_string())?;
    nonempty(&latest.pack_id, "词包 ID")?;
    parse_timestamp_ms(&latest.generated_at)?;
    let url = reqwest::Url::parse(&latest.url)
        .map_err(|_| "热词包地址必须是无参数、无身份信息的 HTTPS URL。".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || latest
            .url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err("热词包地址必须是无参数、无身份信息的 HTTPS URL。".into());
    }
    decode_hex(&latest.sha256, 32).map_err(|_| "热词指针 SHA-256 无效。".to_string())?;
    Ok(latest)
}

pub fn verify_latest(bytes: &[u8], sig: &str) -> Result<Latest, String> {
    validate_latest_value(signed_value(bytes, sig, PUBLIC_KEY_HEX)?)
}

#[cfg(test)]
pub(crate) const TEST_KEY_HEX: &str = include_str!("../tests/fixtures/hotword_test_pubkey.hex");

#[cfg(test)]
pub(crate) fn verify_pack_with_key(
    bytes: &[u8],
    sig: &str,
    now_ms: i64,
    key_hex: &str,
) -> Result<Pack, String> {
    validate_pack_value(signed_value(bytes, sig, key_hex)?, now_ms)
}

#[cfg(test)]
pub(crate) fn verify_latest_with_key(
    bytes: &[u8],
    sig: &str,
    key_hex: &str,
) -> Result<Latest, String> {
    validate_latest_value(signed_value(bytes, sig, key_hex)?)
}

#[cfg(test)]
pub(crate) fn sign_test_value(value: &Value) -> (Vec<u8>, String) {
    use ring::signature::Ed25519KeyPair;
    let seed = decode_hex(include_str!("../tests/fixtures/hotword_test_seed.hex"), 32).unwrap();
    let key = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let bytes = canonical_bytes(value).unwrap();
    let sig = key
        .sign(&bytes)
        .as_ref()
        .iter()
        .map(|ch| format!("{ch:02x}"))
        .collect();
    (bytes, sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::Ed25519KeyPair;
    use serde_json::json;

    const NOW: i64 = 1_794_096_000_000;
    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/hotword-20261108.json");
    const FIXTURE_SIG: &str = include_str!("../tests/fixtures/hotword-20261108.json.sig");

    fn fixture() -> Value {
        serde_json::from_slice(FIXTURE).unwrap()
    }
    fn signed(value: &Value) -> Result<Pack, String> {
        let (bytes, sig) = sign_test_value(value);
        verify_pack_with_key(&bytes, &sig, NOW, TEST_KEY_HEX)
    }

    #[test]
    fn python_generated_pack_and_pointer_pass_with_test_key() {
        let pack = verify_pack_with_key(FIXTURE, FIXTURE_SIG, NOW, TEST_KEY_HEX).unwrap();
        assert_eq!(pack.entry_count, 5);
        assert_eq!(pack.entries[0].text, "测试词");
        let latest = verify_latest_with_key(
            include_bytes!("../tests/fixtures/latest.json"),
            include_str!("../tests/fixtures/latest.json.sig"),
            TEST_KEY_HEX,
        )
        .unwrap();
        assert_eq!(latest.pack_id, pack.pack_id);
        assert_eq!(latest.generated_at, pack.generated_at);
        assert!(verify_pack(FIXTURE, FIXTURE_SIG, NOW)
            .unwrap_err()
            .contains("签名"));
    }

    #[test]
    fn tampering_one_byte_fails_signature() {
        let mut bytes = FIXTURE.to_vec();
        let at = bytes.windows(4).position(|part| part == b"test").unwrap();
        bytes[at] = b'b';
        assert!(verify_pack_with_key(&bytes, FIXTURE_SIG, NOW, TEST_KEY_HEX)
            .unwrap_err()
            .contains("签名"));
    }

    #[test]
    fn different_signing_key_fails() {
        let key = Ed25519KeyPair::from_seed_unchecked(&[99; 32]).unwrap();
        let sig: String = key
            .sign(FIXTURE)
            .as_ref()
            .iter()
            .map(|ch| format!("{ch:02x}"))
            .collect();
        assert!(verify_pack_with_key(FIXTURE, &sig, NOW, TEST_KEY_HEX)
            .unwrap_err()
            .contains("签名"));
    }

    #[test]
    fn expired_pack_rejected() {
        let mut value = fixture();
        value["validUntil"] = json!("2026-11-08T00:00:00Z");
        assert!(signed(&value).unwrap_err().contains("过期"));
    }

    #[test]
    fn wrong_schema_rejected() {
        let mut value = fixture();
        value["schema"] = json!("agent-ime-hotword-pack/2");
        assert!(signed(&value).unwrap_err().contains("schema"));
    }

    #[test]
    fn incorrect_entry_count_rejected() {
        let mut value = fixture();
        value["entryCount"] = json!(4);
        assert!(signed(&value).unwrap_err().contains("条数"));
    }

    #[test]
    fn more_than_two_thousand_entries_rejected() {
        let mut value = fixture();
        value["entries"] = Value::Array(vec![value["entries"][0].clone(); 2001]);
        value["entryCount"] = json!(2001);
        assert!(signed(&value).unwrap_err().contains("2,000"));
    }

    #[test]
    fn signature_then_schema_then_expiry_then_count_then_entry_validation() {
        let mut value = fixture();
        value["schema"] = json!("wrong");
        value["validUntil"] = json!("2000-01-01T00:00:00Z");
        value["entryCount"] = json!(1);
        value["entries"][0]["text"] = json!(42);
        let (bytes, _) = sign_test_value(&value);
        assert!(verify_pack_with_key(&bytes, FIXTURE_SIG, NOW, TEST_KEY_HEX)
            .unwrap_err()
            .contains("签名"));
        assert!(signed(&value).unwrap_err().contains("schema"));
        value["schema"] = json!(PACK_SCHEMA);
        assert!(signed(&value).unwrap_err().contains("过期"));
        value["validUntil"] = json!("2099-01-01T00:00:00Z");
        assert!(signed(&value).unwrap_err().contains("条数"));
        value["entryCount"] = json!(5);
        assert!(signed(&value).unwrap_err().contains("字段"));
    }

    #[test]
    fn field_boundaries_each_reject_the_entire_pack() {
        let cases = vec![
            ("text", json!(""), "1–16"),
            ("text", json!(" "), "1–16"),
            ("text", json!("词".repeat(17)), "1–16"),
            ("text", json!("含\n控制"), "控制"),
            ("pinyin", json!("Ci"), "拼音"),
            ("pinyin", json!("ci1"), "拼音"),
            ("pinyin", json!("  '  "), "拼音"),
            ("definition", json!("字".repeat(121)), "120"),
            ("confidenceMillis", json!(-1), "0–1000"),
            ("confidenceMillis", json!(1001), "0–1000"),
            ("expiresAtMs", json!(-1), "过期时间"),
            ("firstSeenAt", json!("2026-02-30"), "首次收录"),
            ("sources", json!([]), "至少"),
        ];
        for (key, bad, expected) in cases {
            let mut value = fixture();
            value["entries"][0][key] = bad;
            assert!(signed(&value).unwrap_err().contains(expected), "{key}");
        }
        for (key, bad, expected) in [
            ("name", "", "来源名"),
            ("url", "", "HTTPS"),
            ("url", "http://example.invalid/", "HTTPS"),
            ("url", "https://example.invalid/a b", "HTTPS"),
            ("license", " ", "许可"),
            ("collectedAt", "today", "RFC3339"),
        ] {
            let mut value = fixture();
            value["entries"][0]["sources"][0][key] = json!(bad);
            assert!(signed(&value).unwrap_err().contains(expected), "{key}");
        }
    }

    #[test]
    fn field_limits_and_default_expiry_and_pinyin_normalization_pass() {
        let mut value = fixture();
        value["entries"][0]["text"] = json!("字".repeat(16));
        value["entries"][0]["definition"] = json!("字".repeat(120));
        value["entries"][0]["confidenceMillis"] = json!(0);
        value["entries"][0]["pinyin"] = json!(" ce   shi 'ci ");
        value["entries"][0]
            .as_object_mut()
            .unwrap()
            .remove("expiresAtMs");
        let pack = signed(&value).unwrap();
        assert_eq!(pack.entries[0].pinyin, "ce shi 'ci");
        assert_eq!(pack.entries[0].expires_at_ms, None);
        value["entries"][0]["confidenceMillis"] = json!(1000);
        assert!(signed(&value).is_ok());
    }

    #[test]
    fn canonicalization_and_signature_ignore_input_key_order_and_whitespace() {
        let value = fixture();
        let (bytes, sig) = sign_test_value(&value);
        assert_eq!(bytes, FIXTURE);
        assert_eq!(sig, FIXTURE_SIG.trim());
        let reordered = serde_json::to_vec_pretty(&value).unwrap();
        assert!(verify_pack_with_key(&reordered, &sig, NOW, TEST_KEY_HEX).is_ok());
        let left: Value = serde_json::from_str(r#"{"z":{"b":1,"a":"词"},"a":2}"#).unwrap();
        let right: Value = serde_json::from_str(r#"{"a":2,"z":{"a":"词","b":1}}"#).unwrap();
        assert_eq!(sign_test_value(&left), sign_test_value(&right));
        let unicode = json!({"乙": [{"a": "词", "z": 1}], "a": "\u{8}\n"});
        assert_eq!(
            canonical_bytes(&unicode).unwrap(),
            "{\"a\":\"\\b\\n\",\"乙\":[{\"a\":\"词\",\"z\":1}]}\n".as_bytes()
        );
    }

    #[test]
    fn duplicate_keys_and_floats_and_invalid_signatures_rejected() {
        for bytes in [b"{\"a\":1,\"a\":2}".as_slice(), b"{\"a\":0.1}".as_slice()] {
            assert!(signed_value(bytes, FIXTURE_SIG, TEST_KEY_HEX)
                .unwrap_err()
                .contains("签名"));
        }
        assert!(canonical_bytes(&json!({"float": 1.1})).is_err());
        assert!(verify_pack_with_key(FIXTURE, "bad", NOW, TEST_KEY_HEX)
            .unwrap_err()
            .contains("签名"));
    }

    #[test]
    fn pointer_signature_schema_and_download_fields_are_checked() {
        let value: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/latest.json")).unwrap();
        for (key, bad) in [
            ("schema", "wrong"),
            ("sha256", "bad"),
            ("packId", ""),
            ("url", "http://example.invalid/a"),
            ("url", "https://example.invalid/a?q=1"),
            ("url", "https://example.invalid/a#x"),
            ("url", "https://u:p@example.invalid/a"),
            ("generatedAt", "today"),
        ] {
            let mut invalid = value.clone();
            invalid[key] = json!(bad);
            let (bytes, sig) = sign_test_value(&invalid);
            assert!(
                verify_latest_with_key(&bytes, &sig, TEST_KEY_HEX).is_err(),
                "{key} {bad}"
            );
        }
    }

    #[test]
    fn timestamps_validate_calendar_and_timezone() {
        assert_eq!(
            parse_timestamp_ms("2026-11-08T08:00:00+08:00").unwrap(),
            NOW
        );
        assert_eq!(
            parse_timestamp_ms("2026-11-08T00:00:00.123Z").unwrap(),
            NOW + 123
        );
        for date in ["2026-02-30T00:00:00Z", "2026-11-08", "2026-11-08T00:00:00"] {
            assert!(parse_timestamp_ms(date).is_err());
        }
    }
}
