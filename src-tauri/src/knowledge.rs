use crate::snapshot::{LexiconSnapshotEntry, LexiconSnapshotSource};
use crate::storage::AppStorage;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const CHUNK_CHARACTERS: usize = 1_200;
const MAX_SEARCH_RESULTS: usize = 50;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSourceInput {
    pub id: String,
    pub name: String,
    pub source_type: String,
    pub authorized_scope: String,
    pub authorization: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default = "default_true")]
    pub sensitive_enabled: bool,
    pub updated_at_ms: i64,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSource {
    pub id: String,
    pub name: String,
    pub source_type: String,
    pub authorized_scope: String,
    pub authorization: String,
    pub status: String,
    pub project_id: String,
    pub retrieval_status: String,
    pub sensitive_enabled: bool,
    pub last_error: Option<String>,
    pub document_count: usize,
    pub chunk_count: usize,
    pub term_count: usize,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeTerm {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub source_path: String,
    pub text: String,
    pub pinyin: String,
    pub kind: String,
    pub relation: String,
    pub definition: String,
    pub candidate_layer: String,
    pub confidence_millis: i64,
    pub review_state: String,
    pub fixed: bool,
    pub last_verified_ms: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HotwordGenerationStageReport {
    pub staged_count: usize,
    pub invalid_count: usize,
    pub missing_pinyin_count: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IngestReport {
    pub document_count: usize,
    pub chunk_count: usize,
    pub extracted_term_count: usize,
    pub skipped_count: usize,
    pub unchanged_count: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchHit {
    pub chunk_id: String,
    pub content: String,
    pub rank: f64,
    pub source_id: String,
    pub source_name: String,
    pub document_path: String,
    pub ordinal: usize,
}

fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn exact_readings_by_text() -> &'static BTreeMap<&'static str, &'static str> {
    static READINGS: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    READINGS.get_or_init(|| {
        include_str!("../../native/resources/Rime/agent_exact_readings.tsv")
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    })
}

fn local_pinyin_for_text(text: &str) -> Option<String> {
    exact_readings_by_text()
        .get(text.trim())
        .and_then(|readings| readings.split(',').next())
        .map(str::trim)
        .filter(|reading| !reading.is_empty())
        .map(str::to_string)
}

pub fn local_pinyin_readings(texts: &[String]) -> BTreeMap<String, String> {
    texts
        .iter()
        .filter_map(|text| local_pinyin_for_text(text).map(|reading| (text.clone(), reading)))
        .collect()
}

fn required(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{label}不能为空。"))
    } else {
        Ok(value.to_string())
    }
}

fn hidden_or_sensitive(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules" | "Library" | "Keychains" | "Secrets"
            )
    })
}

fn supported(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("README") {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "md" | "markdown"
            | "txt"
            | "json"
            | "yaml"
            | "yml"
            | "rs"
            | "swift"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "toml"
    )
}

fn collect_files(root: &Path) -> Result<(Vec<PathBuf>, usize), String> {
    let metadata =
        fs::symlink_metadata(root).map_err(|error| format!("无法访问授权路径：{error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("授权路径不能是符号链接。".into());
    }
    if metadata.is_file() {
        return Ok((
            if supported(root) {
                vec![root.to_path_buf()]
            } else {
                Vec::new()
            },
            0,
        ));
    }
    if !metadata.is_dir() {
        return Err("授权路径不是普通文件或目录。".into());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut skipped = 0;
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("无法读取授权目录：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法枚举授权目录：{error}"))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("无法检查授权文件：{error}"))?;
            if metadata.file_type().is_symlink() || hidden_or_sensitive(relative) {
                skipped += 1;
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() && supported(&path) {
                files.push(path);
            } else {
                skipped += 1;
            }
        }
    }
    files.sort();
    Ok((files, skipped))
}

fn chunks(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = Vec::new();
    let mut current = String::new();
    for line in normalized.lines() {
        if current.chars().count() + line.chars().count() + 1 > CHUNK_CHARACTERS
            && !current.is_empty()
        {
            result.push(current.trim().to_string());
            current.clear();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}

fn extract_terms(text: &str) -> Vec<(String, String, String, i64)> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        if let Some((term, pinyin)) = line.split_once('|') {
            let term = term.trim().trim_start_matches(['-', '*', '#', ' ']);
            let pinyin = pinyin
                .trim()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if (2..=32).contains(&term.chars().count()) && !pinyin.is_empty() {
                found.insert((term.to_string(), pinyin, "glossary".to_string(), 950));
            }
        }
        for token in line.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')) {
            let mixed_case =
                token.chars().any(char::is_uppercase) && token.chars().any(char::is_lowercase);
            if (2..=32).contains(&token.chars().count())
                && (mixed_case || token.contains('_') || token.contains('-'))
            {
                found.insert((
                    token.to_string(),
                    token.to_ascii_lowercase().replace(['_', '-'], ""),
                    "code_term".to_string(),
                    800,
                ));
            }
        }
    }
    found.into_iter().collect()
}

fn normalize_review_pinyin(value: &str) -> Result<String, String> {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !normalized.is_empty()
        && !normalized.chars().all(|character| {
            character.is_ascii_lowercase() || character == '\'' || character == ' '
        })
    {
        return Err("拼音只允许无声调小写字母、空格和分隔撇号。".into());
    }
    Ok(normalized)
}

fn fts_match_expression(query: &str) -> Result<String, String> {
    let tokens = query
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '_' || character == '-')
        })
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err("检索词不能为空。".into());
    }
    Ok(tokens.join(" OR "))
}

fn generated_hotword_rows(output: &str) -> (Vec<(String, String, String)>, usize) {
    let mut rows = Vec::new();
    let mut invalid = 0;
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("```")
            || line.contains("<词>")
            || line.contains("词<TAB>拼音<TAB>简短释义")
        {
            continue;
        }
        let columns = line.splitn(3, '\t').map(str::trim).collect::<Vec<_>>();
        if columns.len() < 2
            || columns[0].is_empty()
            || columns[0].chars().count() > 64
            || (columns.len() == 3 && columns[2].chars().count() > 160)
        {
            invalid += 1;
            continue;
        }
        let Ok(pinyin) = normalize_review_pinyin(columns[1]) else {
            invalid += 1;
            continue;
        };
        rows.push((
            columns[0].to_string(),
            pinyin,
            columns.get(2).copied().unwrap_or_default().to_string(),
        ));
    }
    (rows, invalid)
}

fn preserve_supported_terms_before_document_delete(
    transaction: &Transaction<'_>,
    source_id: &str,
    document_id: &str,
) -> Result<(), String> {
    let affected = {
        let mut statement = transaction
            .prepare(
                "SELECT t.id,t.text
                 FROM knowledge_terms t
                 JOIN knowledge_chunks c ON c.id=t.chunk_id
                 WHERE t.source_id=?1 AND c.document_id=?2
                 ORDER BY t.id",
            )
            .map_err(|error| format!("无法准备待重指知识术语：{error}"))?;
        let rows = statement
            .query_map(params![source_id, document_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("无法读取待重指知识术语：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析待重指知识术语：{error}"))?;
        rows
    };
    if affected.is_empty() {
        return Ok(());
    }

    let candidates = {
        let mut statement = transaction
            .prepare(
                "SELECT c.id,c.content
                 FROM knowledge_chunks c
                 JOIN knowledge_documents d ON d.id=c.document_id
                 WHERE d.source_id=?1 AND d.id!=?2
                 ORDER BY d.relative_path,c.ordinal,c.id",
            )
            .map_err(|error| format!("无法准备知识术语存活支撑：{error}"))?;
        let rows = statement
            .query_map(params![source_id, document_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("无法读取知识术语存活支撑：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析知识术语存活支撑：{error}"))?;
        rows
    };

    for (term_id, text) in affected {
        let supporting_chunk = candidates.iter().find_map(|(chunk_id, content)| {
            extract_terms(content)
                .into_iter()
                .any(|(candidate, _, _, _)| candidate == text)
                .then_some(chunk_id)
        });
        if let Some(chunk_id) = supporting_chunk {
            transaction
                .execute(
                    "UPDATE knowledge_terms SET chunk_id=?2 WHERE id=?1",
                    params![term_id, chunk_id],
                )
                .map_err(|error| format!("无法重指知识术语存活支撑：{error}"))?;
        }
    }
    Ok(())
}

impl AppStorage {
    pub fn stage_generated_hotword_candidates(
        &self,
        output: &str,
        now_ms: i64,
    ) -> Result<HotwordGenerationStageReport, String> {
        if output.len() > 256 * 1024 {
            return Err("单次热词生成结果超过 256 KiB。".into());
        }
        let (rows, invalid_count) = generated_hotword_rows(output);
        let missing_pinyin_count = rows
            .iter()
            .filter(|(_, pinyin, _)| pinyin.is_empty())
            .count();
        if rows.is_empty() {
            return Ok(HotwordGenerationStageReport {
                staged_count: 0,
                invalid_count,
                missing_pinyin_count,
            });
        }
        let source_id = "generated-hotword-review";
        let document_id =
            hash(format!("{source_id}:{now_ms}:{}", hash(output.as_bytes())).as_bytes());
        let chunk_id = hash(format!("{document_id}:0").as_bytes());
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始热词待审事务：{error}"))?;
        transaction
            .execute(
                "INSERT INTO knowledge_sources
                 (id,name,source_type,authorized_scope,authorization,status,last_error,created_at_ms,updated_at_ms)
                 VALUES (?1,'Owner 生成热词待审','local_path','managed-local://generated-hotword-review',
                         'owner explicit generation action','ready',NULL,?2,?2)
                 ON CONFLICT(id) DO UPDATE SET status='ready',last_error=NULL,updated_at_ms=excluded.updated_at_ms",
                params![source_id, now_ms],
            )
            .map_err(|error| format!("无法复用知识层待审来源：{error}"))?;
        transaction
            .execute(
                "INSERT INTO knowledge_documents
                 (id,source_id,relative_path,format,content_hash,byte_count,updated_at_ms)
                 VALUES (?1,?2,?3,'tsv',?4,?5,?6)",
                params![
                    &document_id,
                    source_id,
                    format!("generated-{now_ms}.tsv"),
                    hash(output.as_bytes()),
                    output.len() as i64,
                    now_ms
                ],
            )
            .map_err(|error| format!("无法记录热词生成批次：{error}"))?;
        transaction
            .execute(
                "INSERT INTO knowledge_chunks (id,document_id,ordinal,content_hash,content)
                 VALUES (?1,?2,0,?3,?4)",
                params![&chunk_id, &document_id, hash(output.as_bytes()), output],
            )
            .map_err(|error| format!("无法记录热词生成内容：{error}"))?;
        for (text, pinyin, definition) in &rows {
            let term_id = hash(format!("{source_id}:{text}").as_bytes());
            transaction
                .execute(
                    "INSERT INTO knowledge_terms
                     (id,source_id,chunk_id,text,pinyin,kind,relation,definition,candidate_layer,
                      confidence_millis,review_state,fixed,last_verified_ms)
                     VALUES (?1,?2,?3,?4,?5,'generated_hotword','generated',?6,'first',950,'pending',0,?7)
                     ON CONFLICT(source_id,text) DO UPDATE SET
                       chunk_id=excluded.chunk_id,
                       pinyin=CASE WHEN knowledge_terms.review_state='pending' THEN excluded.pinyin ELSE knowledge_terms.pinyin END,
                       definition=CASE WHEN knowledge_terms.review_state='pending' THEN excluded.definition ELSE knowledge_terms.definition END,
                       last_verified_ms=excluded.last_verified_ms",
                    params![term_id, source_id, &chunk_id, text, pinyin, definition, now_ms],
                )
                .map_err(|error| format!("无法写入热词审核队列：{error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("无法提交热词待审批次：{error}"))?;
        Ok(HotwordGenerationStageReport {
            staged_count: rows.len(),
            invalid_count,
            missing_pinyin_count,
        })
    }

    pub fn save_knowledge_source(
        &self,
        input: KnowledgeSourceInput,
    ) -> Result<KnowledgeSource, String> {
        let id = required("知识源 ID", &input.id)?;
        let name = required("知识源名称", &input.name)?;
        let scope = required("授权范围", &input.authorized_scope)?;
        let authorization = required("授权说明", &input.authorization)?;
        let project_id = if input.project_id.trim().is_empty() {
            id.clone()
        } else {
            required("项目 ID", &input.project_id)?
        };
        if input.updated_at_ms < 0 {
            return Err("更新时间无效。".into());
        }
        let status = match input.source_type.as_str() {
            "local_path" => {
                let path = Path::new(&scope);
                if !path.is_absolute() {
                    return Err("本地知识源必须使用绝对路径。".into());
                }
                if fs::symlink_metadata(path)
                    .map_err(|error| format!("无法检查授权路径：{error}"))?
                    .file_type()
                    .is_symlink()
                {
                    return Err("授权路径不能是符号链接。".into());
                }
                "authorized"
            }
            "notion" => "deferred",
            _ => return Err("知识源类型无效。".into()),
        };
        let connection = self.lock_connection()?;
        connection.execute("INSERT INTO knowledge_sources (id,name,source_type,authorized_scope,authorization,status,last_error,created_at_ms,updated_at_ms,project_id,retrieval_status,sensitive_enabled) VALUES (?1,?2,?3,?4,?5,?6,NULL,?7,?7,?8,'pending',?9) ON CONFLICT(id) DO UPDATE SET name=excluded.name,authorized_scope=excluded.authorized_scope,authorization=excluded.authorization,status=excluded.status,last_error=NULL,updated_at_ms=excluded.updated_at_ms,project_id=excluded.project_id,retrieval_status='pending',sensitive_enabled=excluded.sensitive_enabled",
            params![id, name, input.source_type, scope, authorization, status, input.updated_at_ms, project_id, if input.sensitive_enabled { 1 } else { 0 }]).map_err(|error| format!("无法保存知识源：{error}"))?;
        drop(connection);
        self.get_knowledge_source(&input.id)?
            .ok_or_else(|| "知识源保存后不可见。".into())
    }

    fn get_knowledge_source(&self, id: &str) -> Result<Option<KnowledgeSource>, String> {
        Ok(self
            .list_knowledge_sources()?
            .into_iter()
            .find(|source| source.id == id))
    }

    pub fn list_knowledge_sources(&self) -> Result<Vec<KnowledgeSource>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare("SELECT s.id,s.name,s.source_type,s.authorized_scope,s.authorization,s.status,s.project_id,s.retrieval_status,s.sensitive_enabled,s.last_error,s.created_at_ms,s.updated_at_ms,(SELECT COUNT(*) FROM knowledge_documents d WHERE d.source_id=s.id),(SELECT COUNT(*) FROM knowledge_chunks c JOIN knowledge_documents d ON d.id=c.document_id WHERE d.source_id=s.id),(SELECT COUNT(*) FROM knowledge_terms t WHERE t.source_id=s.id) FROM knowledge_sources s ORDER BY s.name,s.id").map_err(|error| format!("无法准备知识源列表：{error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(KnowledgeSource {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    source_type: row.get(2)?,
                    authorized_scope: row.get(3)?,
                    authorization: row.get(4)?,
                    status: row.get(5)?,
                    project_id: row.get(6)?,
                    retrieval_status: row.get(7)?,
                    sensitive_enabled: row.get::<_, i64>(8)? != 0,
                    last_error: row.get(9)?,
                    created_at_ms: row.get(10)?,
                    updated_at_ms: row.get(11)?,
                    document_count: row.get(12)?,
                    chunk_count: row.get(13)?,
                    term_count: row.get(14)?,
                })
            })
            .map_err(|error| format!("无法读取知识源：{error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析知识源：{error}"))
    }

    pub fn ingest_knowledge_source(&self, id: &str, now_ms: i64) -> Result<IngestReport, String> {
        let source = self
            .get_knowledge_source(id)?
            .ok_or_else(|| "知识源不存在。".to_string())?;
        if source.source_type == "notion" {
            return Err("Notion 摄取已按 V07-03 后置；当前未进行任何网络读取。".into());
        }
        let root = PathBuf::from(&source.authorized_scope);
        let (files, mut skipped) = collect_files(&root)?;
        let mut total = 0_u64;
        let mut documents = Vec::new();
        for path in files {
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| format!("无法检查文档：{error}"))?;
            if metadata.len() > MAX_FILE_BYTES {
                skipped += 1;
                continue;
            }
            total += metadata.len();
            if total > MAX_SOURCE_BYTES {
                return Err("知识源超过 64 MiB 本地摄取上限。".into());
            }
            let bytes = fs::read(&path).map_err(|error| format!("无法读取文档：{error}"))?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("文档不是有效 UTF-8：{}", path.display()))?
                .to_string();
            let relative = if root.is_file() {
                root.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string()
            };
            documents.push((relative, bytes, text));
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("无法开始知识摄取事务：{error}"))?;
        let reviewed = {
            let mut statement = transaction.prepare("SELECT text,pinyin,review_state,fixed FROM knowledge_terms WHERE source_id=?1 AND review_state!='pending'").map_err(|error| format!("无法准备审核保留清单：{error}"))?;
            let rows = statement
                .query_map(params![id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        (
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                        ),
                    ))
                })
                .map_err(|error| format!("无法读取审核保留清单：{error}"))?;
            rows.collect::<Result<BTreeMap<_, _>, _>>()
                .map_err(|error| format!("无法解析审核保留清单：{error}"))?
        };
        let mut unchanged = 0;
        let mut chunk_count = 0;
        let mut term_count = 0;
        let seen = documents
            .iter()
            .map(|(relative, _, _)| relative.clone())
            .collect::<BTreeSet<_>>();
        for (relative, bytes, text) in documents {
            let content_hash = hash(&bytes);
            let document_id = hash(format!("{id}:{relative}").as_bytes());
            let old: Option<String> = transaction
                .query_row(
                    "SELECT content_hash FROM knowledge_documents WHERE id=?1",
                    params![&document_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| format!("无法检查增量文档：{error}"))?;
            if old.as_deref() == Some(&content_hash) {
                unchanged += 1;
                continue;
            }
            preserve_supported_terms_before_document_delete(&transaction, id, &document_id)?;
            transaction
                .execute(
                    "DELETE FROM knowledge_documents WHERE id=?1",
                    params![&document_id],
                )
                .map_err(|error| format!("无法替换知识文档：{error}"))?;
            let format = Path::new(&relative)
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("text");
            transaction.execute("INSERT INTO knowledge_documents (id,source_id,relative_path,format,content_hash,byte_count,updated_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![&document_id,id,&relative,format,&content_hash,bytes.len() as i64,now_ms]).map_err(|error| format!("无法记录知识文档：{error}"))?;
            for (ordinal, content) in chunks(&text).into_iter().enumerate() {
                let chunk_id = hash(format!("{document_id}:{ordinal}").as_bytes());
                transaction.execute("INSERT INTO knowledge_chunks (id,document_id,ordinal,content_hash,content) VALUES (?1,?2,?3,?4,?5)", params![&chunk_id,&document_id,ordinal as i64,hash(content.as_bytes()),&content]).map_err(|error| format!("无法记录知识块：{error}"))?;
                chunk_count += 1;
                for (term, pinyin, kind, confidence) in extract_terms(&content) {
                    let term_id = hash(format!("{id}:{term}").as_bytes());
                    let (review_pinyin, review_state, fixed) = reviewed
                        .get(&term)
                        .cloned()
                        .unwrap_or_else(|| (pinyin, "pending".into(), 0));
                    transaction.execute("INSERT INTO knowledge_terms (id,source_id,chunk_id,text,pinyin,kind,relation,confidence_millis,review_state,fixed,last_verified_ms) VALUES (?1,?2,?3,?4,?5,?6,'',?7,?8,?9,?10) ON CONFLICT(source_id,text) DO UPDATE SET chunk_id=excluded.chunk_id,pinyin=CASE WHEN knowledge_terms.review_state='pending' THEN excluded.pinyin ELSE knowledge_terms.pinyin END,kind=excluded.kind,confidence_millis=excluded.confidence_millis,last_verified_ms=excluded.last_verified_ms",
                        params![term_id,id,&chunk_id,term,review_pinyin,kind,confidence,review_state,fixed,now_ms]).map_err(|error| format!("无法保存知识术语：{error}"))?;
                    term_count += 1;
                }
            }
        }
        let mut query = transaction
            .prepare(
                "SELECT id,relative_path FROM knowledge_documents
                 WHERE source_id=?1 ORDER BY relative_path,id",
            )
            .map_err(|error| format!("无法准备增量清单：{error}"))?;
        let stale = query
            .query_map(params![id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("无法读取增量清单：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析增量清单：{error}"))?;
        drop(query);
        for (document_id, relative) in stale {
            if !seen.contains(&relative) {
                preserve_supported_terms_before_document_delete(&transaction, id, &document_id)?;
                transaction
                    .execute(
                        "DELETE FROM knowledge_documents WHERE id=?1",
                        params![document_id],
                    )
                    .map_err(|error| format!("无法清理已删除文档：{error}"))?;
            }
        }
        transaction.execute("UPDATE knowledge_sources SET status='ready',retrieval_status='approved',last_error=NULL,updated_at_ms=?2 WHERE id=?1", params![id,now_ms]).map_err(|error| format!("无法更新知识源状态：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法提交知识摄取：{error}"))?;
        Ok(IngestReport {
            document_count: seen.len(),
            chunk_count,
            extracted_term_count: term_count,
            skipped_count: skipped,
            unchanged_count: unchanged,
        })
    }

    pub fn list_knowledge_terms(&self) -> Result<Vec<KnowledgeTerm>, String> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare("SELECT t.id,t.source_id,s.name,s.authorized_scope,t.text,t.pinyin,t.kind,t.relation,t.definition,t.candidate_layer,t.confidence_millis,t.review_state,t.fixed,t.last_verified_ms FROM knowledge_terms t JOIN knowledge_sources s ON s.id=t.source_id ORDER BY t.review_state,t.text").map_err(|error| format!("无法准备知识术语列表：{error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(KnowledgeTerm {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    source_name: row.get(2)?,
                    source_path: row.get(3)?,
                    text: row.get(4)?,
                    pinyin: row.get(5)?,
                    kind: row.get(6)?,
                    relation: row.get(7)?,
                    definition: row.get(8)?,
                    candidate_layer: row.get(9)?,
                    confidence_millis: row.get(10)?,
                    review_state: row.get(11)?,
                    fixed: row.get::<_, i64>(12)? != 0,
                    last_verified_ms: row.get(13)?,
                })
            })
            .map_err(|error| format!("无法读取知识术语：{error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析知识术语：{error}"))
    }

    #[cfg(test)]
    pub fn review_knowledge_term(
        &self,
        id: &str,
        state: &str,
        pinyin: &str,
        fixed: bool,
        now_ms: i64,
    ) -> Result<KnowledgeTerm, String> {
        let current = self
            .list_knowledge_terms()?
            .into_iter()
            .find(|term| term.id == id)
            .ok_or_else(|| "知识术语不存在。".to_string())?;
        self.review_knowledge_term_details(
            id,
            state,
            &current.text,
            pinyin,
            &current.definition,
            &current.candidate_layer,
            fixed,
            now_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn review_knowledge_term_details(
        &self,
        id: &str,
        state: &str,
        text: &str,
        pinyin: &str,
        definition: &str,
        candidate_layer: &str,
        fixed: bool,
        now_ms: i64,
    ) -> Result<KnowledgeTerm, String> {
        if !matches!(state, "pending" | "approved" | "rejected") {
            return Err("审核状态无效。".into());
        }
        if !matches!(candidate_layer, "first" | "secondary") {
            return Err("候选层无效。".into());
        }
        let text = required("术语", text)?;
        let pinyin = normalize_review_pinyin(pinyin)?;
        if state == "approved" && pinyin.is_empty() {
            return Err("批准候选前必须确认读音。".into());
        }
        let previous = self
            .list_knowledge_terms()?
            .into_iter()
            .find(|term| term.id == id)
            .ok_or_else(|| "知识术语不存在。".to_string())?;
        let connection = self.lock_connection()?;
        let changed = connection
            .execute(
                "UPDATE knowledge_terms
             SET review_state=?2,text=?3,pinyin=?4,definition=?5,candidate_layer=?6,
                 fixed=?7,last_verified_ms=?8
             WHERE id=?1",
                params![
                    id,
                    state,
                    text,
                    pinyin,
                    definition.trim(),
                    candidate_layer,
                    if fixed { 1 } else { 0 },
                    now_ms
                ],
            )
            .map_err(|error| format!("无法审核知识术语：{error}"))?;
        if changed == 0 {
            return Err("知识术语不存在。".into());
        }
        drop(connection);
        if previous.kind == "generated_hotword" {
            if let Err(error) = self.sync_approved_generated_hotwords(now_ms) {
                let connection = self.lock_connection()?;
                connection
                    .execute(
                        "UPDATE knowledge_terms
                     SET review_state=?2,text=?3,pinyin=?4,definition=?5,candidate_layer=?6,
                         fixed=?7,last_verified_ms=?8 WHERE id=?1",
                        params![
                            id,
                            previous.review_state,
                            previous.text,
                            previous.pinyin,
                            previous.definition,
                            previous.candidate_layer,
                            if previous.fixed { 1 } else { 0 },
                            previous.last_verified_ms
                        ],
                    )
                    .map_err(|rollback_error| {
                        format!("热词本地源同步失败：{error}；审核回滚也失败：{rollback_error}")
                    })?;
                return Err(format!("热词本地源同步失败，审核已回滚：{error}"));
            }
        }
        self.list_knowledge_terms()?
            .into_iter()
            .find(|term| term.id == id)
            .ok_or_else(|| "知识术语审核后不可见。".into())
    }

    pub fn set_knowledge_source_retrieval_policy(
        &self,
        id: &str,
        retrieval_status: &str,
        sensitive_enabled: bool,
        now_ms: i64,
    ) -> Result<KnowledgeSource, String> {
        if !matches!(retrieval_status, "pending" | "approved") {
            return Err("知识检索审核状态无效。".into());
        }
        let connection = self.lock_connection()?;
        let changed = connection
            .execute(
                "UPDATE knowledge_sources
                 SET retrieval_status=?2,sensitive_enabled=?3,updated_at_ms=?4
                 WHERE id=?1 AND status!='revoked' AND length(trim(authorized_scope))>0",
                params![
                    id,
                    retrieval_status,
                    if sensitive_enabled { 1 } else { 0 },
                    now_ms
                ],
            )
            .map_err(|error| format!("无法更新知识源检索策略：{error}"))?;
        drop(connection);
        if changed == 0 {
            return Err("知识源不存在、已撤权或没有授权范围。".into());
        }
        self.get_knowledge_source(id)?
            .ok_or_else(|| "知识源策略更新后不可见。".into())
    }

    pub fn search_knowledge(
        &self,
        query: &str,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<KnowledgeSearchHit>, String> {
        let project_id = required("项目 ID", project_id)?;
        if limit == 0 || limit > MAX_SEARCH_RESULTS {
            return Err(format!("检索条数必须在 1-{MAX_SEARCH_RESULTS} 之间。"));
        }
        let expression = fts_match_expression(query)?;
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT c.id,c.content,knowledge_chunks_fts.rank,
                        s.id,s.name,d.relative_path,c.ordinal
                 FROM knowledge_chunks_fts
                 JOIN knowledge_chunks c ON c.rowid=knowledge_chunks_fts.rowid
                 JOIN knowledge_documents d ON d.id=c.document_id
                 JOIN knowledge_sources s ON s.id=d.source_id
                 WHERE knowledge_chunks_fts MATCH ?1
                   AND s.project_id=?2
                   AND s.status='ready'
                   AND s.retrieval_status='approved'
                   AND s.sensitive_enabled=1
                   AND length(trim(s.authorized_scope))>0
                 ORDER BY knowledge_chunks_fts.rank ASC,c.id ASC
                 LIMIT ?3",
            )
            .map_err(|error| format!("无法准备本地知识检索：{error}"))?;
        let rows = statement
            .query_map(params![expression, project_id, limit as i64], |row| {
                Ok(KnowledgeSearchHit {
                    chunk_id: row.get(0)?,
                    content: row.get(1)?,
                    rank: row.get(2)?,
                    source_id: row.get(3)?,
                    source_name: row.get(4)?,
                    document_path: row.get(5)?,
                    ordinal: row.get(6)?,
                })
            })
            .map_err(|error| format!("无法执行本地知识检索：{error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法解析本地知识检索结果：{error}"))
    }

    pub fn approved_knowledge_terms_for_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<KnowledgeTerm>, String> {
        let project_id = required("项目 ID", project_id)?;
        let allowed_sources = self
            .list_knowledge_sources()?
            .into_iter()
            .filter(|source| {
                source.project_id == project_id
                    && source.status == "ready"
                    && source.retrieval_status == "approved"
                    && source.sensitive_enabled
                    && !source.authorized_scope.trim().is_empty()
            })
            .map(|source| source.id)
            .collect::<BTreeSet<_>>();
        Ok(self
            .list_knowledge_terms()?
            .into_iter()
            .filter(|term| {
                term.review_state == "approved" && allowed_sources.contains(&term.source_id)
            })
            .collect())
    }

    pub fn delete_knowledge_source(
        &self,
        id: &str,
        purge: bool,
        now_ms: i64,
    ) -> Result<bool, String> {
        let connection = self.lock_connection()?;
        let changed = if purge { connection.execute("DELETE FROM knowledge_sources WHERE id=?1", params![id]) }
        else { connection.execute("UPDATE knowledge_sources SET status='revoked',retrieval_status='pending',sensitive_enabled=0,authorized_scope='',authorization='revoked',updated_at_ms=?2 WHERE id=?1", params![id,now_ms]) }.map_err(|error| format!("无法删除知识源：{error}"))?;
        Ok(changed > 0)
    }

    pub fn approved_knowledge_snapshot_entries(&self) -> Result<Vec<LexiconSnapshotEntry>, String> {
        self.approved_knowledge_entries_for_layer("first")
    }

    /// Approved knowledge terms the owner routed to the second candidate row.
    ///
    /// The review UI has accepted `candidate_layer = "secondary"` since migration 011
    /// and `review_knowledge_term_details` validates it, but the only consumer filtered
    /// for `"first"` and the prediction path reads `hotword_items`, not
    /// `knowledge_terms`. So a term reviewed into the second row was accepted, stored,
    /// and then silently dropped at snapshot time — it appeared nowhere and reported no
    /// error. This is the missing consumer.
    pub fn approved_knowledge_secondary_entries(
        &self,
    ) -> Result<Vec<LexiconSnapshotEntry>, String> {
        self.approved_knowledge_entries_for_layer("secondary")
    }

    fn approved_knowledge_entries_for_layer(
        &self,
        layer: &'static str,
    ) -> Result<Vec<LexiconSnapshotEntry>, String> {
        Ok(self
            .list_knowledge_terms()?
            .into_iter()
            .filter(|term| {
                term.review_state == "approved"
                    && !term.pinyin.is_empty()
                    && term.text.chars().count() <= 32
                    && term.candidate_layer == layer
            })
            .map(|term| LexiconSnapshotEntry {
                id: format!("knowledge-{}", term.id),
                text: term.text,
                pinyin: term.pinyin,
                aliases: Vec::new(),
                professional_packs: Vec::new(),
                project_domains: Vec::new(),
                sources: vec![LexiconSnapshotSource {
                    source_kind: "knowledge".into(),
                    source: format!("{}:{}", term.source_name, term.source_path),
                    license: "private-user-data".into(),
                    confidence_millis: term.confidence_millis,
                }],
                // Second-row terms stay unfixed and unweighted-up: routing a term there
                // is the owner saying "useful, but must not compete for Top-1".
                base_weight: if layer == "secondary" {
                    105
                } else if term.fixed {
                    145
                } else {
                    105
                },
                fixed: layer == "first" && term.fixed,
                disabled: false,
                demoted: false,
                candidate_layer: layer.into(),
                updated_at_ms: term.last_verified_ms,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn indexed_chunk_counts(storage: &AppStorage) -> (usize, usize) {
        let connection = storage.lock_connection().unwrap();
        let chunks = connection
            .query_row("SELECT COUNT(*) FROM knowledge_chunks", [], |row| {
                row.get(0)
            })
            .unwrap();
        let fts = connection
            .query_row("SELECT COUNT(*) FROM knowledge_chunks_fts", [], |row| {
                row.get(0)
            })
            .unwrap();
        (chunks, fts)
    }

    #[test]
    fn local_pinyin_reuses_the_frozen_exact_reading_index() {
        assert_eq!(local_pinyin_for_text("拼音").as_deref(), Some("pin yin"));
        assert_eq!(local_pinyin_for_text("知识").as_deref(), Some("zhi shi"));
        assert_eq!(local_pinyin_for_text("不存在于本地索引的词"), None);
    }

    #[test]
    fn fts_search_closes_project_policy_sensitive_and_deletion_boundaries() {
        let root = std::env::temp_dir().join(format!("agent-knowledge-fts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project_a = root.join("project-a");
        let project_b = root.join("project-b");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();
        fs::write(
            project_a.join("guide.md"),
            "AlphaNeedle | alpha needle\nAlphaNeedle 只属于项目 A 的离线检索资料。\n",
        )
        .unwrap();
        fs::write(
            project_b.join("secret.md"),
            "BetaNeedle | beta needle\nBetaNeedle 只属于项目 B 的敏感资料。\n",
        )
        .unwrap();
        let storage = AppStorage::open(root.join("knowledge.sqlite3")).unwrap();
        for (id, name, path) in [
            ("source-a", "Project A guide", &project_a),
            ("source-b", "Project B secret", &project_b),
        ] {
            storage
                .save_knowledge_source(KnowledgeSourceInput {
                    id: id.into(),
                    name: name.into(),
                    source_type: "local_path".into(),
                    authorized_scope: path.to_string_lossy().into_owned(),
                    authorization: "test explicitly authorizes this fixture".into(),
                    project_id: id.into(),
                    sensitive_enabled: true,
                    updated_at_ms: 1,
                })
                .unwrap();
            storage.ingest_knowledge_source(id, 2).unwrap();
            let term = storage
                .list_knowledge_terms()
                .unwrap()
                .into_iter()
                .find(|term| term.source_id == id)
                .unwrap();
            storage
                .review_knowledge_term(&term.id, "approved", &term.pinyin, true, 3)
                .unwrap();
        }

        let hits = storage
            .search_knowledge("AlphaNeedle", "source-a", 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_name, "Project A guide");
        assert_eq!(hits[0].document_path, "guide.md");
        assert_eq!(hits[0].ordinal, 0);
        assert!(hits[0].content.contains("项目 A"));

        let cross_project = storage
            .search_knowledge("BetaNeedle", "source-a", 5)
            .unwrap();
        println!(
            "V07 project-isolation reverse raw output: {}",
            serde_json::to_string(&cross_project).unwrap()
        );
        assert!(cross_project.is_empty());

        storage
            .set_knowledge_source_retrieval_policy("source-a", "pending", true, 4)
            .unwrap();
        let pending = storage
            .search_knowledge("AlphaNeedle", "source-a", 5)
            .unwrap();
        println!(
            "V07 pending-source reverse raw output: {}",
            serde_json::to_string(&pending).unwrap()
        );
        assert!(pending.is_empty());

        storage
            .set_knowledge_source_retrieval_policy("source-a", "approved", false, 5)
            .unwrap();
        let sensitive_off = storage
            .search_knowledge("AlphaNeedle", "source-a", 5)
            .unwrap();
        println!(
            "V07 sensitive-off reverse raw output: {}",
            serde_json::to_string(&sensitive_off).unwrap()
        );
        assert!(sensitive_off.is_empty());
        storage
            .set_knowledge_source_retrieval_policy("source-a", "approved", true, 6)
            .unwrap();

        fs::remove_file(project_a.join("guide.md")).unwrap();
        storage.ingest_knowledge_source("source-a", 7).unwrap();
        let deleted = storage
            .search_knowledge("AlphaNeedle", "source-a", 5)
            .unwrap();
        println!(
            "V07 deleted-document reverse raw output: {}",
            serde_json::to_string(&deleted).unwrap()
        );
        assert!(deleted.is_empty());
        assert_eq!(indexed_chunk_counts(&storage), (1, 1));

        assert!(storage
            .delete_knowledge_source("source-b", true, 8)
            .unwrap());
        assert_eq!(indexed_chunk_counts(&storage), (0, 0));
        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generated_hotwords_reuse_review_then_sync_only_approved_rows_to_local_text() {
        let root =
            std::env::temp_dir().join(format!("agent-generated-hotwords-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let storage = AppStorage::open(root.join("db.sqlite3")).unwrap();
        let report = storage
            .stage_generated_hotword_candidates(
                "精确热词\tjing que re ci\t已审核精确匹配\n相关表达\txiang guan biao da\t低置信关联\n缺拼音\t\t必须补读音\n格式错误",
                10,
            )
            .unwrap();
        assert_eq!(
            report,
            HotwordGenerationStageReport {
                staged_count: 3,
                invalid_count: 1,
                missing_pinyin_count: 1,
            }
        );
        let terms = storage.list_knowledge_terms().unwrap();
        assert_eq!(terms.len(), 3);
        assert!(storage
            .approved_knowledge_snapshot_entries()
            .unwrap()
            .is_empty());
        assert!(storage.list_hotword_items().unwrap().is_empty());

        let exact = terms.iter().find(|term| term.text == "精确热词").unwrap();
        storage
            .review_knowledge_term_details(
                &exact.id,
                "approved",
                &exact.text,
                &exact.pinyin,
                &exact.definition,
                "first",
                true,
                11,
            )
            .unwrap();
        let related = terms.iter().find(|term| term.text == "相关表达").unwrap();
        storage
            .review_knowledge_term_details(
                &related.id,
                "approved",
                &related.text,
                &related.pinyin,
                &related.definition,
                "secondary",
                false,
                12,
            )
            .unwrap();
        let missing = terms.iter().find(|term| term.text == "缺拼音").unwrap();
        let missing_error = storage
            .review_knowledge_term_details(
                &missing.id,
                "approved",
                &missing.text,
                "",
                &missing.definition,
                "first",
                true,
                13,
            )
            .unwrap_err();
        assert!(missing_error.contains("读音"), "{missing_error}");

        let primary = storage.approved_knowledge_snapshot_entries().unwrap();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].text, "精确热词");
        assert_eq!(primary[0].candidate_layer, "first");
        let predictions = storage.list_prediction_entries(14).unwrap();
        assert_eq!(
            predictions
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["相关表达", "精确热词"]
        );
        assert!(predictions.iter().all(|entry| entry
            .sources
            .iter()
            .all(|source| source.source_kind == "prediction:local_text")));
        let source = storage
            .list_dynamic_sources()
            .unwrap()
            .into_iter()
            .find(|source| source.id == crate::hotword::SELF_AUTHORED_HOTWORD_SOURCE_ID)
            .unwrap();
        assert!(source.enabled);
        assert_eq!(source.ownership, "self-authored");
        assert_eq!(source.license, "self-authored");
        assert!(source.authorization.starts_with("self-authored:"));
        assert!(source.terms_url.starts_with("self-authored://"));
        assert!(source.endpoint.is_empty());
        assert!(
            storage
                .list_hotword_items()
                .unwrap()
                .iter()
                .all(|item| item.text != "缺拼音"),
            "pending missing-pinyin row must never enter local_text"
        );

        storage
            .review_knowledge_term_details(
                &related.id,
                "rejected",
                &related.text,
                &related.pinyin,
                &related.definition,
                "secondary",
                false,
                15,
            )
            .unwrap();
        assert_eq!(
            storage
                .list_prediction_entries(16)
                .unwrap()
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["精确热词"]
        );
        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_project_round_trip_is_local_deterministic_reviewable_and_deletable() {
        let root =
            std::env::temp_dir().join(format!("agent-knowledge-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join(".ssh")).unwrap();
        fs::File::create(root.join("README.md"))
            .unwrap()
            .write_all(b"AgentSnapshot | agent snapshot\nUses ReplayGate and local_only mode.\n")
            .unwrap();
        fs::write(root.join(".ssh/secret.txt"), "NeverExtract | never extract").unwrap();
        let db = root.join("knowledge.sqlite3");
        let storage = AppStorage::open(db).unwrap();
        storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "project".into(),
                name: "real project fixture".into(),
                source_type: "local_path".into(),
                authorized_scope: root.to_string_lossy().to_string(),
                authorization: "owner selected this directory".into(),
                project_id: "project".into(),
                sensitive_enabled: true,
                updated_at_ms: 1,
            })
            .unwrap();
        let first = storage.ingest_knowledge_source("project", 2).unwrap();
        assert_eq!(first.document_count, 1);
        assert!(first.skipped_count >= 1);
        let second = storage.ingest_knowledge_source("project", 3).unwrap();
        assert_eq!(second.unchanged_count, 1);
        let term = storage
            .list_knowledge_terms()
            .unwrap()
            .into_iter()
            .find(|term| term.text == "AgentSnapshot")
            .unwrap();
        assert_eq!(term.review_state, "pending");
        storage
            .review_knowledge_term(&term.id, "approved", "agent snapshot", true, 4)
            .unwrap();
        fs::write(
            root.join("README.md"),
            "AgentSnapshot | changed reading\nUses ReplayGate and local_only mode.\n",
        )
        .unwrap();
        storage.ingest_knowledge_source("project", 5).unwrap();
        let preserved = storage
            .list_knowledge_terms()
            .unwrap()
            .into_iter()
            .find(|term| term.text == "AgentSnapshot")
            .unwrap();
        assert_eq!(preserved.review_state, "approved");
        assert_eq!(preserved.pinyin, "agent snapshot");
        let snapshot = storage.approved_knowledge_snapshot_entries().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].candidate_layer, "first");
        assert!(snapshot[0].sources[0]
            .source
            .contains("real project fixture"));

        // Re-reviewing the same term into the second row must move it, not delete it.
        // Before 2026-07-25 the secondary side had no consumer at all, so this term
        // vanished from both projections and the owner got no error.
        storage
            .review_knowledge_term_details(
                &preserved.id,
                "approved",
                &preserved.text,
                &preserved.pinyin,
                &preserved.definition,
                "secondary",
                false,
                7,
            )
            .unwrap();
        assert!(
            storage
                .approved_knowledge_snapshot_entries()
                .unwrap()
                .is_empty(),
            "a second-row term must not stay in the first-layer projection"
        );
        let secondary = storage.approved_knowledge_secondary_entries().unwrap();
        assert_eq!(secondary.len(), 1, "second-row term must not be dropped");
        assert_eq!(secondary[0].text, "AgentSnapshot");
        assert_eq!(secondary[0].candidate_layer, "secondary");
        assert!(
            !secondary[0].fixed,
            "second row must not carry fixed weight"
        );

        assert!(storage.delete_knowledge_source("project", true, 6).unwrap());
        assert!(storage.list_knowledge_terms().unwrap().is_empty());
        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_symlinks_and_defers_notion_without_network() {
        let root =
            std::env::temp_dir().join(format!("agent-knowledge-boundary-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let storage = AppStorage::open(root.join("db.sqlite3")).unwrap();
        let notion = storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "notion".into(),
                name: "Selected page".into(),
                source_type: "notion".into(),
                authorized_scope: "page-id".into(),
                authorization: "owner selected page".into(),
                project_id: "notion".into(),
                sensitive_enabled: true,
                updated_at_ms: 1,
            })
            .unwrap();
        assert_eq!(notion.status, "deferred");
        assert!(storage
            .ingest_knowledge_source("notion", 2)
            .unwrap_err()
            .contains("后置"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("db.sqlite3"), root.join("linked.md")).unwrap();
            let report = collect_files(&root).unwrap();
            assert!(report.1 >= 1);
        }
        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    // Public exports omit internal taskbooks, so use an explicitly authorized synthetic document.
    #[test]
    fn synthetic_project_document_can_be_authorized_ingested_reviewed_and_removed() {
        let root =
            std::env::temp_dir().join(format!("agent-knowledge-synthetic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let taskbook = root.join("PROJECT-PLAN.md");
        let mut document = String::from(
            "# Synthetic project plan / 合成项目计划\n\n\
             ## Glossary / 项目术语\n\
             SnapshotProbe | snapshot probe\n\
             ReplayGate | replay gate\n\
             local_only | local only\n\n",
        );
        for section in 1..=12 {
            document.push_str(&format!(
                "### Stage {section} / 验证阶段\n\n\
                 本阶段使用合成输入验证授权、导入、审核与删除流程。\
                 SnapshotProbe records a local snapshot; ReplayGate checks local_only mode.\n\n\
                 ```rust\nlet mode = \"local_only\";\n```\n\n"
            ));
        }
        fs::write(&taskbook, document).unwrap();
        assert!(taskbook.is_file());
        let storage = AppStorage::open(root.join("db.sqlite3")).unwrap();
        storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "taskbook".into(),
                name: "synthetic project fixture".into(),
                source_type: "local_path".into(),
                authorized_scope: taskbook.to_string_lossy().to_string(),
                authorization: "test explicitly selects one synthetic project document".into(),
                project_id: "taskbook".into(),
                sensitive_enabled: true,
                updated_at_ms: 1,
            })
            .unwrap();
        let report = storage.ingest_knowledge_source("taskbook", 2).unwrap();
        assert_eq!(report.document_count, 1);
        assert!(report.chunk_count > 0);
        assert!(report.chunk_count > 1, "multi-section fixture must span chunks");
        let term = storage
            .list_knowledge_terms()
            .unwrap()
            .into_iter()
            // Select a known fixture term instead of an arbitrary internal-document term.
            .find(|term| term.text == "SnapshotProbe")
            .expect("synthetic project document yields its local rule term");
        assert_eq!(term.review_state, "pending");
        storage
            .review_knowledge_term(&term.id, "approved", "snapshot probe", false, 3)
            .unwrap();
        assert_eq!(
            storage.approved_knowledge_snapshot_entries().unwrap().len(),
            1
        );
        assert!(storage
            .delete_knowledge_source("taskbook", true, 4)
            .unwrap());
        assert!(storage.list_knowledge_terms().unwrap().is_empty());
        drop(storage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn deleting_one_document_preserves_reviewed_terms_supported_by_another_document() {
        let root =
            std::env::temp_dir().join(format!("agent-knowledge-shared-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.md"), "SharedProbe_x | shared probe\n").unwrap();
        fs::write(
            root.join("b.md"),
            "SharedProbe_x | shared probe\nOnlyInB_x | only in b\n",
        )
        .unwrap();
        let storage = AppStorage::open(root.join("knowledge.sqlite3")).unwrap();
        storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "project".into(),
                name: "shared-term fixture".into(),
                source_type: "local_path".into(),
                authorized_scope: root.to_string_lossy().to_string(),
                authorization: "test explicitly authorizes this fixture".into(),
                project_id: "project".into(),
                sensitive_enabled: true,
                updated_at_ms: 1,
            })
            .unwrap();
        storage.ingest_knowledge_source("project", 2).unwrap();
        for text in ["SharedProbe_x", "OnlyInB_x"] {
            let term = storage
                .list_knowledge_terms()
                .unwrap()
                .into_iter()
                .find(|term| term.text == text)
                .unwrap();
            storage
                .review_knowledge_term(&term.id, "approved", &term.pinyin, true, 3)
                .unwrap();
        }

        fs::remove_file(root.join("b.md")).unwrap();
        storage.ingest_knowledge_source("project", 4).unwrap();
        assert!(fs::read_to_string(root.join("a.md"))
            .unwrap()
            .contains("SharedProbe_x | shared probe"));
        let terms = storage.list_knowledge_terms().unwrap();
        let shared = terms
            .iter()
            .find(|term| term.text == "SharedProbe_x")
            .expect("a supported reviewed term must survive deleting b.md");
        assert_eq!(shared.review_state, "approved");
        assert!(shared.fixed);
        assert!(terms.iter().all(|term| term.text != "OnlyInB_x"));
        let snapshot = storage.approved_knowledge_snapshot_entries().unwrap();
        assert_eq!(
            snapshot
                .iter()
                .filter(|entry| entry.text == "SharedProbe_x")
                .count(),
            1
        );
        assert!(snapshot.iter().all(|entry| entry.text != "OnlyInB_x"));

        drop(storage);
        let _ = fs::remove_dir_all(root);
    }
}
