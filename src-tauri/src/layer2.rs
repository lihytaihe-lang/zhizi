use crate::{
    knowledge::KnowledgeSearchHit,
    storage::{AppStorage, LexiconEntry},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const PROJECT_TERMS: &str =
    include_str!("../../native/resources/Rime/agent_project_terms.dict.yaml");

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum AppClass {
    Code,
    Prose,
    Browser,
    Unknown,
}

impl Default for AppClass {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionModeView {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub action_label: &'static str,
}

pub const ACTION_MODES: &[ActionModeView] = &[
    ActionModeView {
        id: "codex-execution",
        name: "Codex 执行",
        description: "把功能想法改写成边界清晰的代码实现任务。",
        action_label: "生成开发任务",
    },
    ActionModeView {
        id: "bug-fix",
        name: "Bug 修复",
        description: "把一句故障描述改写成可定位、可验证的排查任务。",
        action_label: "生成排查任务",
    },
    ActionModeView {
        id: "code-review",
        name: "代码审查",
        description: "让 AI 按风险优先级审查代码，而不是泛泛建议。",
        action_label: "生成审查任务",
    },
    ActionModeView {
        id: "chatgpt-discussion",
        name: "ChatGPT 讨论",
        description: "把方向或想法整理成高质量讨论问题。",
        action_label: "生成讨论提示",
    },
    ActionModeView {
        id: "product-planning",
        name: "产品规划",
        description: "把产品想法拆成定位、用户、MVP、路线图和风险。",
        action_label: "生成产品规划",
    },
    ActionModeView {
        id: "ui-design-prompt",
        name: "UI 设计提示词",
        description: "把页面想法转成可给图像或设计工具的 UI brief。",
        action_label: "生成设计提示",
    },
    ActionModeView {
        id: "rewrite",
        name: "改写",
        description: "保留原意，改善表达、结构与可读性。",
        action_label: "改写原文",
    },
    ActionModeView {
        id: "summarize",
        name: "总结",
        description: "提炼原文事实、结论、行动项与未决问题。",
        action_label: "总结原文",
    },
    ActionModeView {
        id: "translate",
        name: "翻译",
        description: "按原文语言自动中英互译，保留术语和格式。",
        action_label: "翻译原文",
    },
    ActionModeView {
        id: "hotword-candidates",
        // 与 src/domain/modes.ts 同批改名（2026-07-25）：产出的是个人词/术语，不是
        // 互联网热词热梗；后者需要在线词源，见 tasks/HOTWORD-SOURCE-SERVER.md。
        name: "个人词/术语生成",
        description: "围绕 owner 明确给出的主题生成带拼音和简短释义的个人词/术语，进审核队列。不是互联网热词热梗。",
        action_label: "生成待审词条",
    },
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TermView {
    pub text: String,
    pub pinyin: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextAssemblyRequest {
    pub raw_input: String,
    pub mode_id: String,
    pub mode_name: String,
    pub mode_description: String,
    pub user_rules: Vec<String>,
    pub local_prompt: String,
    #[serde(default)]
    pub compact: bool,
    #[serde(default)]
    pub app_class: AppClass,
    #[serde(default)]
    pub context_allowed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PayloadPreview {
    pub payload: String,
    pub character_count: usize,
    pub profile_entry_count: usize,
    pub project_term_count: usize,
    pub preference_count: usize,
    pub knowledge_chunk_count: usize,
}

fn enabled_rules_block(rules: &[String]) -> String {
    let rules = rules
        .iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty())
        .collect::<Vec<_>>();
    if rules.is_empty() {
        "无".into()
    } else {
        rules
            .into_iter()
            .map(|rule| format!("- {rule}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn render_local_prompt(
    mode_id: &str,
    raw_input: &str,
    user_rules: &[String],
    compact: bool,
) -> Result<String, String> {
    let raw = raw_input.trim();
    if raw.is_empty() {
        return Err("原文不能为空。".into());
    }
    let rules = user_rules
        .iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty())
        .map(|rule| rule.trim_end_matches(['。', '；', ';', ',', ' ']))
        .collect::<Vec<_>>();
    let compact_rules = if rules.is_empty() {
        String::new()
    } else {
        format!(
            "\n长期偏好：{}。",
            rules.iter().take(3).copied().collect::<Vec<_>>().join("；")
        )
    };
    let full_rules = if rules.is_empty() {
        String::new()
    } else {
        format!(
            "\n【用户长期偏好】\n{}\n",
            rules
                .iter()
                .enumerate()
                .map(|(index, rule)| format!("{}. {rule}", index + 1))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let output = if compact {
        match mode_id {
            "chatgpt-discussion" => format!("请围绕这个想法做一次高质量讨论：{raw}{compact_rules}\n\n请输出：\n1. 核心用户和最痛场景；\n2. MVP 应做/不做；\n3. 关键假设、风险和反例；\n4. 下一步最小验证行动。"),
            "codex-execution" => format!("请在当前项目完成：{raw}{compact_rules}\n执行边界：\n1. 先读相关文件并说明关键假设。\n2. 只做完成目标所需的最小改动。\n3. 不引入重型依赖，除非先说明理由。\n4. 完成后给出修改文件、运行方式和验证结果。\n\n验收：功能可本地运行并验证，不破坏现有功能。"),
            "product-planning" => format!("请把这个产品想法整理成可执行规划：{raw}{compact_rules}\n\n请输出定位、目标用户、核心场景、MVP 范围与不做项、风险、验证方式和下一步任务。"),
            "bug-fix" => format!("请排查并修复：{raw}{compact_rules}\n\n先说明最可能原因和相关文件，再修根因；完成后回复根因、修改点、验证结果和遗留风险。"),
            "code-review" => format!("请按风险优先审查当前代码：{raw}{compact_rules}\n\n只列真实问题，按严重程度排序；每条包含位置、原因、影响、建议和测试。"),
            "ui-design-prompt" => format!("请生成高保真 UI 设计 brief：{raw}{compact_rules}\n\n要求真实工具界面、桌面端优先、信息层级清晰、视觉克制专业。"),
            "rewrite" => format!("请改写以下原文，保留事实、语气和原意，改善表达、结构与可读性：\n\n{raw}{compact_rules}\n\n只输出改写后的正文，不补充原文没有的信息。"),
            "summarize" => format!("请总结以下原文：\n\n{raw}{compact_rules}\n\n按“核心结论、关键事实、行动项、未决问题”组织；没有的部分省略，不推测。"),
            "translate" => format!("请翻译以下原文：中文自动译为英文，其他语言自动译为中文；保留术语和格式：\n\n{raw}{compact_rules}\n\n只输出译文。"),
            "hotword-candidates" => format!("请围绕这个主题生成待审热词：{raw}{compact_rules}\n\n每行严格使用“词<TAB>拼音<TAB>简短释义”；拼音使用空格分隔的无声调小写音节。不要标题、序号、Markdown 或解释。\n\n本地手填模板：\n<词>\t<拼音>\t<简短释义>"),
            _ => return Err("不支持的动作模式。".into()),
        }
    } else {
        match mode_id {
            "chatgpt-discussion" => format!("我想讨论一个问题/方向，请帮我把它分析清楚。\n\n【我的初步想法】\n{raw}\n{full_rules}\n【希望你帮我分析】\n核心用户、最痛场景、MVP 边界、方案取舍、验证方式与下一步最小行动。"),
            "codex-execution" => format!("请在当前项目中完成以下任务。\n\n【任务目标】\n{raw}\n{full_rules}\n【执行边界】\n先读项目；只改必要文件；不引入重型依赖；保持现有风格。\n\n【验收标准】\n功能可本地验证，不破坏现有功能，说明修改文件、测试结果和遗留风险。"),
            "product-planning" => format!("请把下面这个产品想法整理成可执行的产品规划。\n\n【产品想法】\n{raw}\n{full_rules}\n【请输出】\n定位、目标用户、核心场景、MVP 与不做项、模块、路线图、风险和下一步任务。"),
            "bug-fix" => format!("请排查并修复以下问题。\n\n【问题描述】\n{raw}\n{full_rules}\n【执行要求】\n先定位根因，只修根因；完成后说明根因、修改点、验证方式和遗留风险。"),
            "code-review" => format!("请对当前代码进行审查。\n\n【审查目标】\n{raw}\n{full_rules}\n【输出要求】\n只列真实风险，按严重程度排序，包含文件位置、原因、影响、建议和测试。"),
            "ui-design-prompt" => format!("请生成一张高保真 UI 设计图/原型图。\n\n【页面或产品想法】\n{raw}\n{full_rules}\n【设计要求】\n真实工具界面、桌面端优先、信息层级清晰、视觉克制专业，包含核心流程与状态反馈。"),
            "rewrite" => format!("请改写下面的原文。\n\n【原文】\n{raw}\n{full_rules}\n【要求】\n保留事实、原意和语气；改善结构与可读性；不添加信息；只输出正文。"),
            "summarize" => format!("请总结下面的原文。\n\n【原文】\n{raw}\n{full_rules}\n【输出】\n核心结论、关键事实、行动项、未决问题；没有的部分省略，不推测。"),
            "translate" => format!("请翻译下面的原文。中文自动译为英文，其他语言自动译为中文。\n\n【原文】\n{raw}\n{full_rules}\n【要求】\n保留术语、代码、路径、数字和格式；只输出译文。"),
            "hotword-candidates" => format!("请围绕 owner 明确给出的主题生成一组可供人工审核的中文热词候选。\n\n【主题】\n{raw}\n{full_rules}\n【唯一输出格式】\n每行严格输出：词<TAB>拼音<TAB>简短释义\n\n【要求】\n拼音由本次生成一并给出，使用空格分隔的无声调小写音节；不抓取或引用任何站点；不输出标题、序号、Markdown 或解释。\n\n【无 Provider 时的本地手填模板】\n<词>\t<拼音>\t<简短释义>"),
            _ => return Err("不支持的动作模式。".into()),
        }
    };
    Ok(output)
}

fn safe_term(entry: LexiconEntry) -> TermView {
    TermView {
        text: entry.text,
        pinyin: entry.pinyin,
        aliases: entry.aliases,
    }
}

pub fn list_personal_terms(storage: &AppStorage) -> Result<Vec<TermView>, String> {
    let mut terms = storage
        .list_lexicon_entries()?
        .into_iter()
        .filter(|entry| !entry.disabled && !entry.demoted && entry.project_domains.is_empty())
        .map(safe_term)
        .collect::<Vec<_>>();
    terms.sort_by(|left, right| {
        left.text
            .cmp(&right.text)
            .then(left.pinyin.cmp(&right.pinyin))
    });
    Ok(terms)
}

fn static_project_terms() -> Vec<TermView> {
    PROJECT_TERMS
        .lines()
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with("---")
                && !line.contains(": ")
        })
        .filter_map(|line| {
            let mut columns = line.split('\t');
            let text = columns.next()?.trim();
            let pinyin = columns.next()?.trim();
            (!text.is_empty() && !pinyin.is_empty()).then(|| TermView {
                text: text.into(),
                pinyin: pinyin.into(),
                aliases: Vec::new(),
            })
        })
        .collect()
}

fn is_this_project(domain: &str) -> bool {
    matches!(
        domain.trim().to_ascii_lowercase().as_str(),
        "agent-input-method" | "agent_input_method" | "agentinputmethod" | "agent-input-layer"
    )
}

pub fn list_project_terms(storage: &AppStorage) -> Result<Vec<TermView>, String> {
    let runtime = storage.project_runtime_configuration()?;
    let Some(domain) = runtime.active_project_domain else {
        return Ok(Vec::new());
    };
    let mut terms = BTreeMap::<(String, String), TermView>::new();
    if is_this_project(&domain) {
        for term in static_project_terms() {
            terms.insert((term.text.clone(), term.pinyin.clone()), term);
        }
    }
    for entry in storage.list_lexicon_entries()? {
        let belongs_to_domain = entry
            .project_domains
            .iter()
            .any(|candidate| candidate == &domain);
        let belongs_to_pack = entry
            .professional_packs
            .iter()
            .any(|pack| runtime.active_professional_packs.contains(pack));
        if !entry.disabled && !entry.demoted && (belongs_to_domain || belongs_to_pack) {
            let term = safe_term(entry);
            terms.insert((term.text.clone(), term.pinyin.clone()), term);
        }
    }
    Ok(terms.into_values().collect())
}

fn raw_matches(raw: &str, term: &TermView) -> bool {
    let lowered = raw.to_lowercase();
    std::iter::once(term.text.as_str())
        .chain(term.aliases.iter().map(String::as_str))
        .any(|candidate| {
            !candidate.trim().is_empty() && lowered.contains(&candidate.to_lowercase())
        })
}

fn terms_block(terms: &[TermView]) -> String {
    if terms.is_empty() {
        "无（0 条）".into()
    } else {
        terms
            .iter()
            .map(|term| {
                if term.aliases.is_empty() {
                    format!("- {}（{}）", term.text, term.pinyin)
                } else {
                    format!(
                        "- {}（{}；别名：{}）",
                        term.text,
                        term.pinyin,
                        term.aliases.join("、")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn knowledge_block(hits: &[KnowledgeSearchHit]) -> String {
    if hits.is_empty() {
        "无（0 条）".into()
    } else {
        hits.iter()
            .map(|hit| {
                format!(
                    "- 来源：{} · {} · chunk #{}\n  {}",
                    hit.source_name,
                    hit.document_path,
                    hit.ordinal,
                    hit.content.replace('\n', "\n  ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn matched_knowledge_chunks(
    storage: &AppStorage,
    raw_input: &str,
    project_id: Option<&str>,
) -> Result<Vec<KnowledgeSearchHit>, String> {
    let Some(project_id) = project_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let mut hits = BTreeMap::<String, KnowledgeSearchHit>::new();
    for term in storage.approved_knowledge_terms_for_project(project_id)? {
        let view = TermView {
            text: term.text,
            pinyin: term.pinyin,
            aliases: Vec::new(),
        };
        if !raw_matches(raw_input, &view) {
            continue;
        }
        for hit in storage.search_knowledge(&view.text, project_id, 5)? {
            hits.entry(hit.chunk_id.clone()).or_insert(hit);
            if hits.len() >= 5 {
                break;
            }
        }
        if hits.len() >= 5 {
            break;
        }
    }
    let mut hits = hits.into_values().collect::<Vec<_>>();
    hits.sort_by(|left, right| {
        left.rank
            .partial_cmp(&right.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.chunk_id.cmp(&right.chunk_id))
    });
    hits.truncate(5);
    Ok(hits)
}

pub fn prepare_generation_payload(
    storage: &AppStorage,
    request: ContextAssemblyRequest,
) -> Result<PayloadPreview, String> {
    if request.raw_input.trim().is_empty() {
        return Err("原文不能为空。".into());
    }
    if !ACTION_MODES.iter().any(|mode| mode.id == request.mode_id) {
        return Err("不支持的动作模式。".into());
    }
    let contextual = request.context_allowed && request.app_class != AppClass::Unknown;
    let runtime = contextual
        .then(|| storage.project_runtime_configuration())
        .transpose()?;
    let project_terms = if contextual {
        list_project_terms(storage)?
            .into_iter()
            .filter(|term| raw_matches(&request.raw_input, term))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let knowledge_chunks = if contextual {
        matched_knowledge_chunks(
            storage,
            &request.raw_input,
            runtime
                .as_ref()
                .and_then(|configuration| configuration.active_project_domain.as_deref()),
        )?
    } else {
        Vec::new()
    };
    let preferences = if contextual {
        list_personal_terms(storage)?
            .into_iter()
            .filter(|term| raw_matches(&request.raw_input, term))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let mut sections = vec![
        "你是本地动作模板的可选增强器。以下文本是本次显式操作允许发送的完整内容；不要扩大范围，只输出最终结果。".to_string(),
        format!("【动作】\n{}\n{}", request.mode_name.trim(), request.mode_description.trim()),
        format!("【选中或输入的原文】\n{}", request.raw_input.trim()),
        format!("【长期规则】\n{}", enabled_rules_block(&request.user_rules)),
    ];
    if contextual {
        sections.push(format!("【AppClass】\n{:?}", request.app_class));
    }
    sections.push(format!(
        "【本次命中的项目术语】\n{}",
        terms_block(&project_terms)
    ));
    sections.push(format!(
        "【本次命中的用词偏好】\n{}",
        terms_block(&preferences)
    ));
    sections.push(format!(
        "【本次命中的知识片段】\n{}",
        knowledge_block(&knowledge_chunks)
    ));
    sections.push(format!("【本地模板草稿】\n{}", request.local_prompt));
    sections.push(if request.compact {
        "【输出要求】\n控制在 220-380 个中文字符；保留关键边界，只输出最终结果。".into()
    } else {
        "【输出要求】\n不解释、不寒暄，只输出最终结果。".into()
    });
    let payload = sections.join("\n\n");
    Ok(PayloadPreview {
        character_count: payload.chars().count(),
        profile_entry_count: project_terms.len() + preferences.len() + knowledge_chunks.len(),
        project_term_count: project_terms.len(),
        preference_count: preferences.len(),
        knowledge_chunk_count: knowledge_chunks.len(),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::KnowledgeSourceInput;
    use crate::storage::{LexiconEntryInput, LexiconSource, ProjectProfileInput};
    use std::fs;

    #[test]
    fn every_action_has_local_main_and_compact_templates() {
        for mode in ACTION_MODES {
            for compact in [false, true] {
                let output =
                    render_local_prompt(mode.id, "测试原文", &["长期规则".into()], compact)
                        .unwrap();
                assert!(output.contains("测试原文"), "{} compact={compact}", mode.id);
            }
        }
        assert_eq!(ACTION_MODES.len(), 10);
    }

    #[test]
    fn static_project_dictionary_parser_never_exposes_metadata_lines() {
        let terms = static_project_terms();
        assert!(terms.iter().any(|term| term.text == "AgentInputMethod"));
        assert!(!terms.iter().any(|term| term.text.contains("version")));
    }

    fn test_storage(name: &str) -> (AppStorage, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "agent-layer2-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("test.sqlite3");
        (AppStorage::open(path.clone()).unwrap(), path)
    }

    fn term(id: &str, text: &str, domains: Vec<&str>) -> LexiconEntryInput {
        LexiconEntryInput {
            id: id.into(),
            text: text.into(),
            pinyin: id.into(),
            aliases: vec![format!("{text}Alias")],
            professional_packs: Vec::new(),
            project_domains: domains.into_iter().map(str::to_string).collect(),
            sources: vec![LexiconSource {
                source_kind: "manual".into(),
                source: "owner-entry".into(),
                license: "private-user-data".into(),
                confidence_millis: 1000,
            }],
            base_weight: 500,
            fixed: true,
            disabled: false,
            demoted: false,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn assembler_matches_only_raw_terms_and_never_accepts_app_identifiers_or_history() {
        let (storage, path) = test_storage("assembler");
        storage
            .create_lexicon_entry(term("personal", "偏好叫法", Vec::new()))
            .unwrap();
        storage
            .create_lexicon_entry(term("project", "ProjectNova", vec!["custom-domain"]))
            .unwrap();
        storage
            .save_project_profile(ProjectProfileInput {
                id: "custom".into(),
                name: "Sensitive App Name".into(),
                project_domain: "custom-domain".into(),
                professional_packs: Vec::new(),
                agent_context: "forbidden preceding fragment from events.jsonl".into(),
                updated_at_ms: 1,
            })
            .unwrap();
        storage.activate_project_profile(Some("custom")).unwrap();
        let request = ContextAssemblyRequest {
            raw_input: "请把 ProjectNova 按偏好叫法改写".into(),
            mode_id: "rewrite".into(),
            mode_name: "改写".into(),
            mode_description: "改善表达".into(),
            user_rules: vec!["保持简洁".into()],
            local_prompt: "本地草稿".into(),
            compact: false,
            app_class: AppClass::Code,
            context_allowed: true,
        };
        let preview = prepare_generation_payload(&storage, request).unwrap();
        assert_eq!(preview.project_term_count, 1);
        assert_eq!(preview.preference_count, 1);
        assert_eq!(preview.profile_entry_count, 2);
        assert!(preview.payload.contains("ProjectNova"));
        assert!(preview.payload.contains("偏好叫法"));
        for forbidden in [
            "Sensitive App Name",
            "custom-domain",
            "forbidden preceding fragment",
            "events.jsonl",
            "bundleIdentifier",
            "bundle hash",
        ] {
            assert!(
                !preview.payload.contains(forbidden),
                "payload leaked {forbidden}"
            );
        }

        let no_match = prepare_generation_payload(
            &storage,
            ContextAssemblyRequest {
                raw_input: "完全无关的原文".into(),
                mode_id: "rewrite".into(),
                mode_name: "改写".into(),
                mode_description: "改善表达".into(),
                user_rules: Vec::new(),
                local_prompt: "本地草稿".into(),
                compact: false,
                app_class: AppClass::Code,
                context_allowed: true,
            },
        )
        .unwrap();
        assert_eq!(no_match.project_term_count, 0);
        assert_eq!(no_match.preference_count, 0);
        assert_eq!(no_match.knowledge_chunk_count, 0);

        let unknown = prepare_generation_payload(
            &storage,
            ContextAssemblyRequest {
                raw_input: "ProjectNova 偏好叫法".into(),
                mode_id: "rewrite".into(),
                mode_name: "改写".into(),
                mode_description: "改善表达".into(),
                user_rules: Vec::new(),
                local_prompt: "本地草稿".into(),
                compact: false,
                app_class: AppClass::Unknown,
                context_allowed: true,
            },
        )
        .unwrap();
        assert_eq!(unknown.profile_entry_count, 0);
        assert_eq!(unknown.knowledge_chunk_count, 0);
        assert!(!unknown.payload.contains("【AppClass】"));
        drop(storage);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn assembler_adds_only_raw_matched_project_knowledge_and_counts_visible_preview() {
        let (storage, path) = test_storage("knowledge-assembler");
        let source_root = path.parent().unwrap().join("project-docs");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(
            source_root.join("architecture.md"),
            "ProjectNova | project nova\nProjectNova 使用本地 FTS5 检索，不读取打字历史。\n",
        )
        .unwrap();
        storage
            .save_project_profile(ProjectProfileInput {
                id: "custom".into(),
                name: "Custom Project".into(),
                project_domain: "custom-domain".into(),
                professional_packs: Vec::new(),
                agent_context: "forbidden preceding fragment".into(),
                updated_at_ms: 1,
            })
            .unwrap();
        storage.activate_project_profile(Some("custom")).unwrap();
        storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "project-docs".into(),
                name: "Project docs".into(),
                source_type: "local_path".into(),
                authorized_scope: source_root.to_string_lossy().into_owned(),
                authorization: "test explicitly authorizes this fixture".into(),
                project_id: "custom-domain".into(),
                sensitive_enabled: true,
                updated_at_ms: 2,
            })
            .unwrap();
        storage.ingest_knowledge_source("project-docs", 3).unwrap();
        let term = storage
            .list_knowledge_terms()
            .unwrap()
            .into_iter()
            .find(|term| term.text == "ProjectNova")
            .unwrap();
        storage
            .review_knowledge_term(&term.id, "approved", "project nova", true, 4)
            .unwrap();

        let matched = prepare_generation_payload(
            &storage,
            ContextAssemblyRequest {
                raw_input: "请解释 ProjectNova 的检索边界".into(),
                mode_id: "summarize".into(),
                mode_name: "总结".into(),
                mode_description: "总结原文".into(),
                user_rules: Vec::new(),
                local_prompt: "本地草稿".into(),
                compact: false,
                app_class: AppClass::Code,
                context_allowed: true,
            },
        )
        .unwrap();
        assert_eq!(matched.knowledge_chunk_count, 1);
        assert_eq!(matched.profile_entry_count, 1);
        assert!(matched.payload.contains("【本次命中的知识片段】"));
        assert!(matched
            .payload
            .contains("Project docs · architecture.md · chunk #0"));
        assert!(matched.payload.contains("FTS5 检索"));
        assert_eq!(matched.character_count, matched.payload.chars().count());
        assert!(!matched.payload.contains("forbidden preceding fragment"));

        let unrelated = prepare_generation_payload(
            &storage,
            ContextAssemblyRequest {
                raw_input: "这段原文没有任何知识术语".into(),
                mode_id: "summarize".into(),
                mode_name: "总结".into(),
                mode_description: "总结原文".into(),
                user_rules: Vec::new(),
                local_prompt: "本地草稿".into(),
                compact: false,
                app_class: AppClass::Code,
                context_allowed: true,
            },
        )
        .unwrap();
        assert_eq!(unrelated.knowledge_chunk_count, 0);
        assert!(unrelated
            .payload
            .contains("【本次命中的知识片段】\n无（0 条）"));

        drop(storage);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
