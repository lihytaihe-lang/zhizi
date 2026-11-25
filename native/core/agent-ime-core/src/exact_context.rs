// Lexical collocations with a specific left context and an exact reading.
// Keep broad cues such as "新鲜" out: both 新鲜食物 and 新鲜事物 are ordinary.
const COLLOCATIONS: &[(&str, &str, &str)] = &[
    ("shiwu", "失误", "低级"),
    ("shiwu", "事务", "日常行政"),
    ("shiwu", "十五", "农历正月"),
    ("wan", "完", "请办"),
    ("wan", "完", "尽快办"),
    ("qian", "前", "活动开始"),
    ("qian", "前", "会议开始"),
    ("qian", "前", "比赛结束"),
    ("zhiding", "置顶", "将该公告"),
    ("zhiding", "置顶", "把本帖"),
    ("zhiding", "置顶", "把那条通知在群里"),
    ("zhiding", "置顶", "把这条通知在群聊里"),
    ("zhiding", "置顶", "把群公告在群里"),
    ("zhiding", "制定", "流程需要重新"),
    ("zhiding", "制定", "制度必须重新"),
    ("gaifa", "该法", "本案根据"),
    ("gaifa", "该法", "新规根据"),
    ("gaifa", "该法", "依照"),
    ("gaifa", "改法", "这种"),
    ("zhibiao", "指标", "关键绩效"),
    ("zhibiao", "治标", "这个偏方只能"),
    ("zhibiao", "治标", "头痛医头只能"),
    ("zhibiao", "制表", "在电脑上自动"),
    ("shishi", "史实", "史书记录的"),
    ("shishi", "史实", "古籍记载的"),
    ("shishi", "史实", "经史料证实的"),
    ("shishi", "失实", "报道内容明显"),
    ("shishi", "失实", "数据严重"),
    ("shishi", "失事", "客机途中"),
    ("shishi", "事实", "都清楚的客观"),
    ("shishi", "事实", "已核实的客观"),
    ("shishi", "事实", "无法否认的客观"),
    ("shitu", "仕途", "断送了自己的"),
    ("shitu", "视图", "切换到列表"),
    ("shitu", "视图", "切换到三维"),
    ("zhishi", "芝士", "马苏里拉"),
    ("zhishi", "芝士", "帕玛森"),
    ("zhishi", "芝士", "切达"),
    ("xingshi", "姓氏", "族谱上的"),
    ("xingshi", "姓氏", "家谱记载的"),
    ("xingshi", "行驶", "车辆应减速"),
    ("xingshi", "行驶", "车辆应当减速"),
];

struct ContextCue {
    reading: &'static str,
    word: &'static str,
    preceding_phrases: &'static [&'static str],
    endings: &'static [&'static str],
}

const CLAUSE_BOUNDARIES: [char; 12] = ['。', '！', '？', '；', '，', '、', '.', '!', '?', ';', ',', '\n'];

fn strip_any_suffix<'a>(text: &'a str, suffixes: &[&str]) -> Option<&'a str> {
    suffixes.iter().find_map(|suffix| text.strip_suffix(*suffix))
}

fn recent_clause(text: &str) -> &str {
    let clause = text.rsplit(|c| CLAUSE_BOUNDARIES.contains(&c) || matches!(c, ':' | '：' | '…'))
        .next().unwrap_or_default();
    match clause.char_indices().rev().nth(15) {
        Some((start, _)) => &clause[start..],
        None => clause,
    }
}

fn zhiding_context_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("AIM_EXPERIMENT_ZHIDING_CONTEXT")
        .map_or(cfg!(all(target_os = "macos", feature = "mac-fast-typing")), |value| value == "1"))
}

fn drafted_document(clause: &str) -> bool {
    const WRITTEN_RULES: &[&str] = &["细则", "条例", "规程", "办法", "制度", "章程", "规范", "标准", "规则", "条款", "守则"];
    const DRAFTING_BODIES: &[&str] = &[
        "双方", "各方", "双方代表", "各部门", "各单位", "主管部门", "相关部门", "工作组", "起草组", "联合工作组",
    ];
    // A written provision as the grammatical object is stronger than mentions of
    // a contract or manager elsewhere. A designated item may come from a list.
    if ["备选", "备用", "候选", "选定", "挑选", "筛选", "选出", "其中", "另行",
        "两套", "三套", "两份", "三份", "几套", "几份"]
        .iter().any(|cue| clause.contains(cue))
    {
        return false;
    }
    if let Some(before_joint) = strip_any_suffix(clause, &["共同", "联合", "一起", "协同"])
        && let Some((object, agents)) = before_joint.rsplit_once('由')
    {
        let object = strip_any_suffix(object, &["应当", "必须", "需要", "需", "应", "须", "将"]).unwrap_or(object);
        if DRAFTING_BODIES.contains(&agents) && strip_any_suffix(object, WRITTEN_RULES).is_some() {
            return true;
        }
    }
    let Some(before_revision) = clause.strip_suffix("重新") else { return false; };
    let Some(before_modal) = strip_any_suffix(before_revision,
        &["还需要", "需要", "必须", "应当", "还要", "还得", "要", "得", "应", "须"]) else { return false; };
    let Some(object) = strip_any_suffix(before_modal,
        &["今天", "明天", "本周", "下周", "本月", "下月", "年底前", "月底前", "近期", "现在", "随后", "尽快", "会后", "年内", "稍后"])
        else { return false; };
    strip_any_suffix(object, WRITTEN_RULES).is_some()
        || strip_any_suffix(object, &["预案", "计划", "纲要", "方案", "时间表"]).is_some()
}

fn selection_from_people(clause: &str) -> bool {
    let before_adverb = strip_any_suffix(clause, &["正式", "直接", "优先", "临时", "另行"]).unwrap_or(clause);
    let Some(before_pool_end) = strip_any_suffix(before_adverb, &["当中", "之中", "中"]) else { return false; };
    let Some((_, pool)) = before_pool_end.rsplit_once(['从', '在']) else { return false; };
    const PEOPLE: &[&str] = &["候选人", "候选者", "志愿者", "申请人", "代表", "委员", "评委", "组员", "成员", "专家"];
    let Some(modifier) = strip_any_suffix(pool, PEOPLE) else { return false; };
    // The source of the choice must be people. Exclude proposals and reported
    // recommendations even when they happen to contain a person's role.
    pool.chars().count() <= 16
        && !["起草", "拟定", "草案", "方案", "版本", "提交", "建议", "意见", "推荐", "和", "及"]
            .iter().any(|cue| pool.contains(cue))
        && (modifier.is_empty() || [
            "两名", "三名", "四名", "五名", "数名", "几名", "多名", "若干名", "几位", "两位", "三位",
            "候选", "报名", "入围", "在场", "现有", "所有", "评审",
        ].iter().any(|prefix| modifier.starts_with(prefix)))
}

fn space_first_zhiding_context(preceding: &str) -> Option<&'static str> {
    let clause = preceding.rsplit(CLAUSE_BOUNDARIES).next()?.trim_end();
    match (drafted_document(clause), selection_from_people(clause)) {
        (true, false) => Some("制定"),
        (false, true) => Some("指定"),
        _ => None,
    }
}

pub(crate) fn protects_drafted_head(raw: &str, preceding: &str) -> bool {
    raw == "zhiding" && zhiding_context_enabled() && space_first_zhiding_context(preceding) == Some("制定")
}

// Some collocations need both a grammatical ending and an earlier topic in
// the same sentence. A bare "事先" or "只能" would be too broad on its own.
const CUES: &[ContextCue] = &[
    ContextCue {
        reading: "xingshi", word: "行驶",
        preceding_phrases: &["高速路", "高速公路"],
        endings: &["注意减速", "应当减速", "应减速"],
    },
    ContextCue {
        reading: "zhiding", word: "指定",
        preceding_phrases: &["保存到", "上传到", "复制到", "移动到", "寄到", "转存到"],
        endings: &["事先", "预先", "所"],
    },
    ContextCue {
        reading: "dengji", word: "登记",
        preceding_phrases: &["访客", "来访", "入住", "到场"],
        endings: &["到前台", "到门岗", "办理入场", "办理实名"],
    },
    ContextCue {
        reading: "dengji", word: "等级",
        preceding_phrases: &["评估", "评定", "划分"],
        endings: &["风险", "故障", "灾情"],
    },
    ContextCue {
        reading: "zhibiao", word: "制表",
        preceding_phrases: &["统计", "报表", "汇总"],
        endings: &["用软件重新", "用电子表格重新"],
    },
    ContextCue {
        reading: "zhibiao", word: "治标",
        preceding_phrases: &["根除病因", "暂时缓解", "表面症状", "治本"],
        endings: &["只能", "仅能", "只会"],
    },
    ContextCue {
        reading: "shishi", word: "史实",
        preceding_phrases: &["史料", "史书", "文献", "古籍", "地方志"],
        endings: &["记载的", "证实的", "可查的", "佐证的"],
    },
    ContextCue {
        reading: "shishi", word: "失实",
        preceding_phrases: &["报道", "新闻", "通报", "关键数字"],
        endings: &["明显", "严重"],
    },
    ContextCue {
        reading: "shishi", word: "失事",
        preceding_phrases: &["黑匣子", "坠机", "沉船", "搜救"],
        endings: &["飞机", "客机", "货轮", "船只"],
    },
    ContextCue {
        reading: "zhishi", word: "芝士",
        preceding_phrases: &["披萨", "比萨", "焗饭"],
        endings: &["拉丝的", "刨碎的", "烤化的"],
    },
    ContextCue {
        reading: "zhishi", word: "制式",
        preceding_phrases: &["电视", "广播", "基站"],
        endings: &["视频信号", "传输信号", "无线通信"],
    },
];

// Beyond the first page, require precise context with evidence immediately
// adjacent to the typed word. Avoid crossing a clause for generic cues.
pub(crate) fn offpage_exact_collocation(preceding: &str) -> Option<(&'static str, &'static str)> {
    #[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
    if font_size_request(preceding) { return Some(("zaidaxie", "再大些")); }
    let clause = preceding.rsplit(['。', '！', '？', '；', '，', '、', '.', '!', '?', ';', ',', '\n'])
        .next().unwrap_or_default();
    if let Some(word) = short_phrase_after_context(clause) { return Some(word); }
    if ["家谱上标了同一个", "族谱上标了同一个", "家谱里登记了同一个", "族谱里登记了同一个",
        "家谱上都记着同一个", "族谱上都记着同一个"]
        .iter().any(|cue| clause.ends_with(cue))
        || preceding.ends_with("户籍资料显示，这个家族世代沿用同一个")
    {
        return Some(("xingshi", "姓氏"));
    }
    if ["举报内容明显", "投诉内容明显", "举报材料中的说法明显", "投诉材料的主要陈述严重"].iter().any(|cue| clause.ends_with(cue))
    {
        return Some(("shishi", "失实"));
    }
    let before = ["马苏里拉", "帕玛森", "切达"].iter()
        .find_map(|variety| clause.strip_suffix(variety))?;
    let cooking = ["出炉", "出锅", "趁热", "披萨", "比萨", "焗饭", "烤盘"];
    let actions = ["撒", "撒上", "撒了一层", "撒一层", "撒了些", "撒些", "铺上", "铺了一层"];
    (cooking.iter().any(|cue| before.contains(cue)) && actions.iter().any(|action| before.ends_with(action)))
        .then_some(("zhishi", "芝士"))
}

fn short_phrase_after_context(clause: &str) -> Option<(&'static str, &'static str)> {
    let nearby = recent_clause(clause);
    if strip_any_suffix(nearby, &["手续都", "材料都", "资料都", "人手都", "证件都"]).is_some() {
        return Some(("qile", "齐了"));
    }
    if ["生品", "生鲜", "生食", "新鲜食物"].iter().any(|cue| nearby.contains(cue))
        && strip_any_suffix(nearby, &["而不是", "区别于", "分开存放的"]).is_some()
    {
        return Some(("shupin", "熟品"));
    }
    if ["先办理", "收好行李", "收拾好行李", "等人到齐", "加满油"].iter().any(|cue| nearby.ends_with(cue))
        && !["处罚", "执法", "触发", "调试"].iter().any(|cue| nearby.contains(cue))
    {
        return Some(("zaichufa", "再出发"));
    }
    None
}

#[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
fn font_size_request(preceding: &str) -> bool {
    let clause = recent_clause(preceding).trim_end();
    strip_any_suffix(clause, &["可以", "能不能", "能否"]).is_some()
        && ["字体", "字号", "文字", "窗口的字", "屏幕上的字", "标题的字"]
            .iter().any(|word| clause.contains(word))
}

#[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
fn approval_context(preceding: &str) -> bool {
    let clause = recent_clause(preceding.trim_end().trim_end_matches(['，', ',']));
    ["方案", "提案", "审核", "审批", "报价", "议案", "评审"]
        .iter().any(|word| clause.contains(word))
        && (strip_any_suffix(clause, &["不用再改", "不需要修改", "已经一致", "没有异议", "没异议", "已通过", "已确认"]).is_some()
            || clause.contains("确认了") && !["没确认", "未确认", "不确认", "无法确认", "没通过", "修改", "调整", "拒绝"]
                .iter().any(|word| clause.contains(word)))
}

#[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
fn replacing_draft(preceding: &str) -> bool {
    let Some((before, current)) = preceding.rsplit_once(['，', ',']) else { return false; };
    if current.contains(CLAUSE_BOUNDARIES) || current.contains(['不', '没'])
        || strip_any_suffix(current.trim_end(), &["我打算", "我准备", "我会", "我们打算", "我们准备", "我们会"]).is_none()
    {
        return false;
    }
    let previous = before.rsplit(CLAUSE_BOUNDARIES).next().unwrap_or_default().trim_end();
    ["草稿", "稿子", "文档", "旧文件", "旧版本", "记录"]
        .iter().any(|word| previous.contains(word))
        && strip_any_suffix(previous, &["不继续改了", "不用了", "作废", "废弃了", "先撤掉", "停用"]).is_some()
}

/// Only these contextual phrases can use a proved native composition as an
/// exact collocation. The rest require a dictionary word as before.
pub(crate) fn accepts_composed_collocation(raw: &str) -> bool {
    cfg!(all(target_os = "macos", feature = "mac-fast-typing")) && matches!(raw, "zaidaxie" | "wozhijietongyi")
}

pub(crate) fn architectural_viewing_platform(raw: &str, preceding: &str) -> bool {
    if !cfg!(all(target_os = "macos", feature = "mac-fast-typing")) || raw != "guanlitai" { return false; }
    let clause = recent_clause(preceding).trim_end();
    ["古建筑", "礼仪建筑", "宫廷", "宫殿", "王宫", "城楼", "礼仪广场"].iter().any(|word| clause.contains(word))
        && !["软件", "应用", "登录", "后台", "服务器"].iter().any(|word| clause.contains(word))
        && strip_any_suffix(clause, &["宫廷的", "宫殿的", "建筑的", "建筑里的", "修了一座", "修了一个", "建了一座",
            "留有一处", "前设有", "旁设有"]).is_some()
}

pub(crate) fn overrides_cross_app_learning(raw: &str, preceding: &str, first: &str, preferred: &str) -> bool {
    if (first, preferred) == ("管理台", "观礼台") && architectural_viewing_platform(raw, preceding) {
        return true;
    }
    #[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
    if raw == "xinqi" && (first, preferred) == ("新奇", "新起") {
        return replacing_draft(preceding);
    }
    false
}

// The short-word fast path uses complete phrases or typed left-context
// evidence. Bare 上文 such as 新鲜, 基础, 根据, or 改成 is ambiguous.
fn short_exact_context(raw: &str, preceding: &str) -> Option<&'static str> {
    if raw == "zhiding" && zhiding_context_enabled() {
        let clause = recent_clause(preceding);
        let message = ["公告", "通知", "帖子", "这条消息", "那条消息"];
        if message.iter().any(|noun| clause.contains(noun))
            && strip_any_suffix(clause, &["在群聊里", "在群里", "在讨论组里", "在频道里"]).is_some()
        {
            return Some("置顶");
        }
        // Drafting evidence only protects an existing 制定 head in the mixer.
        // A bounded person pool can request the usual exact-page promotion.
        return space_first_zhiding_context(preceding).filter(|word| *word == "指定");
    }
    if !matches!(raw, "shiwu" | "zhishi" | "shishi" | "xingshi" | "jiya" | "dengji") { return None; }
    let clause = preceding.rsplit(['。', '！', '？', '；', '，', '、', '.', '!', '?', ';', ',', '\n'])
        .next().unwrap_or_default();
    match raw {
        "shiwu" => short_shiwu_context(clause),
        "dengji" => {
            let clause = recent_clause(preceding);
            let paperwork = ["手续", "证件", "来访", "访客", "入住", "入场", "报到"];
            let grading = ["风险", "故障", "危险", "能效", "权限", "噪音"];
            let reception = ["前台", "接待处", "服务台"].iter().any(|x| clause.contains(x))
                && ["访客", "来宾", "客人", "来访者"].iter().any(|x| clause.contains(x));
            let confirmed = ["确认了可以", "核实后可以", "确认后可以"].iter().any(|x| clause.ends_with(x));
            let flight = ["机场", "航班", "乘机", "登机口"].iter().any(|x| clause.contains(x));
            let register = paperwork.iter().any(|noun| clause.contains(noun))
                && strip_any_suffix(clause, &["办理", "办理入场", "办理入住", "先签到"]).is_some();
            let grade = grading.iter().any(|noun| clause.contains(noun))
                && strip_any_suffix(clause, &["评定", "重新核定", "评估风险", "评定故障"]).is_some();
            match ((register || reception && confirmed) && !flight, grade) {
                (true, false) => Some("登记"),
                (false, true) => Some("等级"),
                _ => None,
            }
        }
        "jiya" => {
            let work = ["办公室", "工单", "订单", "申请", "邮件", "案件"];
            let pending = ["处理今天", "清理昨天", "处理之前", "清理近期", "清理长期", "消化此前"];
            (work.iter().any(|topic| clause.contains(topic))
                && pending.iter().any(|end| clause.ends_with(end))).then_some("积压")
        }
        "zhishi" => {
            let educational = ["孩子", "学生", "这节课", "这门课", "教材", "练习题"];
            let foundations = ["这一部分基础", "这个单元的基础", "课本上的基础", "这一节的基础"];
            (educational.iter().any(|cue| clause.contains(cue))
                && foundations.iter().any(|cue| clause.ends_with(cue))).then_some("知识")
        }
        "shishi" => {
            let action = ["改成", "设置为", "切换为"].iter().find_map(|verb| clause.strip_suffix(verb))?;
            let (source, producer) = ["传感器数据", "监控画面", "定位信息"].iter()
                .find_map(|source| action.strip_suffix(source).map(|producer| (*source, producer)))?;
            let switches_feed = ["后台把", "监控系统把", "采集程序把"].iter()
                .any(|cue| producer.ends_with(cue));
            let rejects_interval = ["取消定时", "停掉定时", "停止定时"].iter().any(|cue| producer.contains(cue));
            (switches_feed && (source == "传感器数据" || rejects_interval)).then_some("实时")
        }
        "xingshi" => {
            ["表决权由本人依法", "否决权由董事会依法", "请求权由当事人依法"]
                .iter().any(|cue| clause.ends_with(cue)).then_some("行使")
        }
        _ => None,
    }
}

fn short_shiwu_context(clause: &str) -> Option<&'static str> {
    if ["展览馆", "博物馆", "科技馆", "科学展"].iter().any(|place| clause.contains(place))
        && ["从没见过的", "未曾见过的", "第一次见到的"].iter().any(|end| clause.ends_with(end))
        && !["美食", "饮食", "食品", "菜肴", "食材"].iter().any(|food| clause.contains(food))
    {
        return Some("事物");
    }
    let administrative = ["办理部门日常", "经办部门日常", "处理处室日常"];
    let other_topics = ["税务", "临床", "见习", "实习", "培训", "税法", "审计"];
    if administrative.iter().any(|cue| clause.ends_with(cue))
        && !other_topics.iter().any(|cue| clause.contains(cue))
    {
        return Some("事务");
    }
    #[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
    {
        let workplace = ["办公室", "行政", "处室", "部门"];
        let action = ["处理", "办理", "经办", "收尾", "解决"];
        let conflict = ["食物", "食材", "食品", "冰箱", "食堂", "菜品", "过期"];
        if strip_any_suffix(clause, &["积压的", "待办的", "日常", "尚未办完的"]).is_some()
            && workplace.iter().any(|word| clause.contains(word))
            && action.iter().any(|word| clause.contains(word))
            && !conflict.iter().chain(&other_topics).any(|word| clause.contains(word))
        {
            return Some("事务");
        }
    }

    let month = clause.strip_suffix('月')?;
    let (before, number) = month.rsplit_once("农历").or_else(|| month.rsplit_once("阴历"))?;
    let number = number.strip_prefix('闰').unwrap_or(number);
    const MONTHS: &[&str] = &["正", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二", "冬", "腊"];
    (MONTHS.contains(&number) && ["才到", "定在", "订在", "安排在"].iter().any(|cue| before.ends_with(cue)))
        .then_some("十五")
}

#[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
fn everyday_exact_collocation(raw: &str, preceding: &str) -> Option<&'static str> {
    if !matches!(raw, "qian" | "daochu" | "yaoshi" | "guomin" | "gaifa" | "baoyou" | "yibian"
        | "shizhuangbao" | "kuaichuan" | "goujianhao" | "guanlitai" | "yangli" | "dabiao"
        | "shencha" | "cuoci" | "changshiban" | "najige" | "yiban" | "chushen" | "tichu"
        | "shiyong" | "queshi" | "mingming") {
        return None;
    }
    let clause = recent_clause(preceding).trim_end();
    let contains = |words: &[&str]| words.iter().any(|word| clause.contains(word));
    let ends_with = |words: &[&str]| strip_any_suffix(clause, words).is_some();
    match raw {
        "qian" if ends_with(&["到期", "截止", "起飞", "开庭", "发车", "开场"]) => Some("前"),
        "daochu" if ends_with(&["批量", "一键", "选择", "分类", "按日期", "为表格", "从系统"])
            && contains(&["表格", "文件", "报表", "查询", "数据", "列表", "记录", "工具"]) => Some("导出"),
        "yaoshi" if ends_with(&["记得带上", "别忘带", "别忘了带", "忘了带"])
            || ends_with(&["拿一下", "带上", "取出", "掉了", "找一下"])
                && contains(&["门锁", "锁住", "开门", "车门", "上锁", "防盗门", "车库"]) => Some("钥匙"),
        "guomin" if ends_with(&["有点", "有些", "容易", "会", "有"])
            && contains(&["花生", "坚果", "牛奶", "虾蟹", "海鲜", "芒果", "花粉", "尘螨", "药物", "猫毛"]) => Some("过敏"),
        "gaifa" if ends_with(&["引用", "适用", "援引", "遵照"])
            && contains(&["法律", "法规", "条例", "法院", "法官", "司法", "条款"]) => Some("该法"),
        "baoyou" if ends_with(&["想法", "措施", "政策", "安排", "情况", "食品", "食物", "事物", "前景"])
            && contains(&["对于", "对这", "对那", "对某"]) => Some("抱有"),
        "yibian" if ends_with(&["核对", "复核", "校对", "通读", "重读"]) => Some("一遍"),
        "shizhuangbao" if contains(&["测试机", "试用", "试跑", "验证升级", "预发布"])
            && ends_with(&["发一个", "传一个", "出一个", "打一个", "做一个", "装一个", "先装"])
            => Some("试装包"),
        "kuaichuan" if contains(&["文件", "照片", "文档", "视频", "图片"])
            && ends_with(&["用局域网", "通过局域网", "走局域网", "经局域网", "用蓝牙", "用隔空投送"])
            => Some("快传"),
        "kuaichuan" if contains(&["救援", "巡逻", "渔船", "航道", "码头"])
            && ends_with(&["一艘", "两艘", "几艘"]) => Some("快船"),
        "goujianhao" if contains(&["查看", "核对", "查询", "显示", "记录", "读取", "确认"])
            && ends_with(&["软件的", "系统的", "应用的", "版本的", "安装包的", "更新包的"])
            => Some("构建号"),
        "guanlitai" if contains(&["打开", "进入", "登录", "访问", "切回"])
            && ends_with(&["知字", "知字的", "软件的", "后台的", "应用的", "服务端的", "服务器的"])
            => Some("管理台"),
        "guanlitai" => Some("观礼台"),
        "yangli" if ends_with(&["测试", "验收", "基准", "回归", "输入", "输出", "代码"])
            && contains(&["参考", "新增", "记录", "增加", "构造", "提供"]) => Some("样例"),
        "dabiao" if contains(&["图片", "样本", "文本", "语料", "数据集"])
            && ends_with(&["逐条", "逐张", "逐项", "人工", "分批"]) => Some("打标"),
        "shencha" if contains(&["原始数据", "原始日志", "来源", "证据"])
            && ends_with(&["继续", "进一步", "逐项", "再"]) => Some("深查"),
        "cuoci" if contains(&["输入法", "首选", "候选"])
            && ends_with(&["出了", "出现", "弹出", "写成了"]) => Some("错词"),
        "changshiban" if ends_with(&["可安装这个", "安装这个", "下载这个", "安装最新的", "下载最新的"])
            => Some("尝试版"),
        "changshiban" => Some("唱诗班"),
        "najige" if ends_with(&["没通过的是", "未通过的是", "不合格的是", "待处理的是"])
            => Some("哪几个"),
        "yiban" if ends_with(&["先改", "先修订", "先起草", "先迭代", "先更新"])
            => Some("一版"),
        "chushen" if contains(&["申请", "文件", "稿件", "材料", "资料"])
            && ends_with(&["提交", "送交", "送去", "进入", "通过", "待"]) => Some("初审"),
        "tichu" if contains(&["不合格", "不符合条件", "不符合要求", "重复项", "无效样本"])
            && ends_with(&["全部", "逐一", "一并", "统一"]) => Some("剔除"),
        "shiyong" if contains(&["付款前", "订阅前", "购买前"])
            && ends_with(&["免费", "先", "先行"]) => Some("试用"),
        "queshi" if contains(&["档案", "文件", "资料", "字段", "页面", "记录"])
            && ends_with(&["严重", "大量", "成批"]) => Some("缺失"),
        "mingming" if contains(&["给目录", "给文件", "给图层", "给文档", "给变量", "给项目"])
            && ends_with(&["重新", "统一", "自动", "批量"]) => Some("命名"),
        _ => None,
    }
}

#[cfg(feature = "mac-fast-typing")]
pub(crate) fn has_context_sensitive_abbreviation(raw: &str) -> bool {
    matches!(raw, "sdyx" | "mashangdf" | "tmyjqrl" | "wmxzqck" | "womenxzqck"
        | "womenxianzaiqck" | "wmyqxbl" | "womenyqxbl" | "womenyiqixbl")
}

#[cfg(feature = "mac-fast-typing")]
pub(crate) fn context_sensitive_abbreviation(raw: &str, preceding: &str) -> Option<(&'static str, bool)> {
    if !has_context_sensitive_abbreviation(raw) { return None; }
    // A trailing comma separates the typed word from an immediate request.
    // End-of-sentence punctuation must not revive a request from an older topic.
    let sentence = preceding.rsplit(['。', '！', '？', '；', '.', '!', '?', ';', '\n']).next()?;
    let clause = recent_clause(sentence.trim_end_matches(['，', ',', ' ', '\t'])).trim_end();
    match raw {
        "sdyx" => {
            let asks_to_wait = strip_any_suffix(clause,
                &["你先", "您先", "劳烦你", "劳烦您", "请您先", "请你先"]).is_some();
            let interruption = ["中断", "掉线", "故障", "停电", "设备", "延误"]
                .iter().any(|word| clause.contains(word));
            Some(("稍等一下", asks_to_wait && !interruption))
        }
        "mashangdf" => {
            let waits_for_a_reply = ["给个回应", "给我个回复", "等答复", "等回复", "收到请", "看到请"]
                .iter().any(|word| sentence.contains(word));
            let requests_an_action = strip_any_suffix(clause, &["请", "请你", "请您", "麻烦你", "麻烦您"]).is_some()
                || clause.ends_with("给个回应");
            Some(("马上答复", waits_for_a_reply && requests_an_action))
        }
        "tmyjqrl" => {
            let reports_arrivals = ["前台说", "客服回复", "接待处确认", "工作人员回复"]
                .iter().any(|word| clause.contains(word));
            let visitors = strip_any_suffix(clause, &["客人", "来宾", "来访人员", "来访者"]).is_some();
            let pending = ["还没", "没有", "不能", "待确认", "没联系上"]
                .iter().any(|word| sentence.contains(word));
            Some(("他们已经确认了", reports_arrivals && visitors && !pending))
        }
        "wmxzqck" | "womenxzqck" | "womenxianzaiqck" => {
            let parking = ["停车场", "车位", "车停在"].iter().any(|word| sentence.contains(word));
            let retrieve_car = ["取车", "去开车"].iter().any(|word| sentence.contains(word));
            let missing_key = sentence.contains("车钥匙")
                && ["找不到", "再找找", "丢了", "落在"].iter().any(|word| sentence.contains(word));
            let conflicting = ["已经找到", "钥匙找到了", "不用去", "不要去", "不去车库",
                "不取车", "窗口开", "开始受理", "办理手续"]
                .iter().any(|word| sentence.contains(word));
            Some(("我们现在去车库", (missing_key || parking && retrieve_car) && !conflicting))
        }
        "wmyqxbl" | "womenyqxbl" | "womenyiqixbl" => {
            let mentions = |words: &[&str]| words.iter().any(|word| sentence.contains(word));
            let counter = mentions(&["窗口", "柜台", "办事大厅"]);
            let paperwork = mentions(&["手续", "材料", "证件", "表单"]);
            let ready = mentions(&["带齐", "备齐", "齐了", "准备齐全", "都准备好"]);
            let conflicts = mentions(&["没带齐", "未带齐", "没备齐", "未备齐", "还缺", "不全",
                "关门", "打烊", "停止受理", "暂不受理", "不能办理", "已经收工", "刚交完班",
                "提案", "议案", "意见还没统一"]);
            Some(("我们一起先办理", counter && paperwork && ready && !conflicts))
        }
        _ => None,
    }
}

pub(crate) fn exact_collocation(raw: &str, preceding: &str) -> Option<&'static str> {
    #[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
    match raw {
        "zaidaxie" if font_size_request(preceding) => return Some("再大些"),
        "wozhijietongyi" if approval_context(preceding) => return Some("我直接同意"),
        "tongyi" if preceding.trim_end().strip_suffix("我直接").is_some_and(approval_context) => return Some("同意"),
        "xinqi" if replacing_draft(preceding) => return Some("新起"),
        _ => {}
    }
    if let Some(word) = preferred_word(COLLOCATIONS, raw, preceding)
        .or_else(|| document_or_component(raw, preceding))
        .or_else(|| match raw {
            "qile" | "shupin" | "zaichufa" => short_phrase_after_context(preceding)
                .filter(|(reading, _)| *reading == raw).map(|(_, word)| word),
            _ => None,
        })
        .or_else(|| short_exact_context(raw, preceding))
    {
        return Some(word);
    }
    #[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
    if let Some(word) = everyday_exact_collocation(raw, preceding) {
        return Some(word);
    }
    let preceding = if raw == "dengji" { recent_clause(preceding) } else { preceding };
    let matching_endings = CUES.iter().filter(|cue| cue.reading == raw)
        .flat_map(|cue| cue.endings.iter().map(move |ending| (cue, *ending)))
        .filter_map(|(cue, ending)| preceding.strip_suffix(ending).map(|before| (cue, before)));
    let mut matched = None;
    for (cue, before) in matching_endings {
        let clause = before.rsplit(['。', '！', '？', '；', '.', '!', '?', ';', '\n'])
            .next().unwrap_or_default();
        if !cue.preceding_phrases.iter().any(|phrase| clause.contains(phrase)) {
            continue;
        }
        if matched.is_some_and(|word| word != cue.word) {
            return None;
        }
        matched = Some(cue.word);
    }
    matched
}

// An adjacent action identifies the kind of object. Restrict evidence to the active clause.
fn document_or_component(raw: &str, preceding: &str) -> Option<&'static str> {
    if raw != "yuanjian" { return None; }
    let tail = recent_clause(preceding);
    const FILES: &[&str] = &["证照", "执照", "资料", "文件", "证件", "身份证", "合同", "协议", "档案", "签证"];
    const KEEP: &[&str] = &["保留", "留存", "保管", "核对", "核验", "查验", "归还"];
    const SHOW: &[&str] = &["出示", "携带", "带上"];
    const BOARDS: &[&str] = &["电路板", "线路板", "主板"];
    const FIX: [&str; 2] = ["焊接", "更换"];
    const BROKEN: [&str; 2] = ["损坏", "烧坏"];

    let keep_tail = tail.strip_suffix('好').unwrap_or(tail);
    let keep_end = KEEP.iter().any(|k| {
        keep_tail.ends_with(k) && FILES.iter().any(|f| near(keep_tail, f, k, 8))
    });
    let show_end = SHOW.iter().any(|v| {
        FILES.iter().any(|f| {
            tail.match_indices(v).any(|(i, a)| {
                starts_within(&tail[i + a.len()..], f, 4) && near_end(tail, f, 8)
            })
        })
    });
    let fix_end = FIX.iter().any(|v| near_end(tail, v, 6))
        && BOARDS.iter().any(|b| FIX.iter().any(|v| near(tail, b, v, 4)));
    let broken_end = BROKEN.iter().any(|a| {
        followed_by(tail, a, "的电子") && near_end(tail, "的电子", 6)
    });

    match (keep_end || show_end, fix_end || broken_end) {
        (true, false) => Some("原件"),
        (false, true) => Some("元件"),
        _ => None,
    }
}

fn near(s: &str, a: &str, b: &str, gap: usize) -> bool {
    for (i, x) in s.match_indices(a) {
        if starts_within(&s[i + x.len()..], b, gap) { return true; }
    }
    for (i, x) in s.match_indices(b) {
        if starts_within(&s[i + x.len()..], a, gap) { return true; }
    }
    false
}

fn starts_within(s: &str, needle: &str, gap: usize) -> bool {
    let mut dist = 0;
    for (k, _) in s.char_indices() {
        if dist > gap { break; }
        if s[k..].starts_with(needle) { return true; }
        dist += 1;
    }
    false
}

fn near_end(s: &str, needle: &str, n: usize) -> bool {
    let total = s.chars().count();
    s.match_indices(needle)
        .any(|(i, a)| total - s[..i + a.len()].chars().count() <= n)
}

fn followed_by(s: &str, head: &str, tail: &str) -> bool {
    s.match_indices(head).any(|(i, h)| s[i + h.len()..].starts_with(tail))
}

fn preferred_word<'a>(
    rules: &'a [(&'a str, &'a str, &'a str)],
    raw: &str,
    preceding: &str,
) -> Option<&'a str> {
    let mut best: Option<&'a str> = None;
    let mut best_len = 0usize;
    let mut ambiguous = false;
    for &(key, word, suffix) in rules {
        if raw != key || suffix.is_empty() || !preceding.ends_with(suffix) {
            continue;
        }
        let len = suffix.len();
        if len > best_len {
            best_len = len;
            best = Some(word);
            ambiguous = false;
        } else if len == best_len && best != Some(word) {
            ambiguous = true;
        }
    }
    if ambiguous { None } else { best }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(target_os = "macos", feature = "mac-fast-typing"))]
    #[test]
    fn everyday_collocations_require_the_active_topic_and_action() {
        for (raw, context, expected) in [
            ("gaifa", "新版条例开始施行后法院可以援引", Some("该法")),
            ("gaifa", "法院有新规定。上面这句现有的", None),
            ("shiwu", "行政部门正在处理尚未办完的", Some("事务")),
            ("shiwu", "行政办公室要清理冰箱里积压的", None),
            ("guomin", "对花粉她向来容易", Some("过敏")),
            ("guomin", "统计结果涉及众多", None),
            ("daochu", "请在报表里按日期分类", Some("导出")),
            ("daochu", "报表还没给我。这里都是人，到", None),
            ("yaoshi", "门锁关好后记得拿一下", Some("钥匙")),
            ("yaoshi", "这种安排还行，", None),
            ("baoyou", "他们对于新措施", Some("抱有")),
            ("baoyou", "祝福亲朋好友，祈祷神灵", None),
            ("yibian", "附件请再核对", Some("一遍")),
            ("yibian", "请把清单从头通读", Some("一遍")),
            ("yibian", "请把它放在", None),
            ("yibian", "请先核对。两份材料各放", None),
            ("shizhuangbao", "先在测试机发一个", Some("试装包")),
            ("shizhuangbao", "预发布版本先出一个", Some("试装包")),
            ("shizhuangbao", "测试机刚关。她买了一个新款", None),
            ("shizhuangbao", "她想买包试用一下", None),
            ("kuaichuan", "把文件用局域网", Some("快传")),
            ("kuaichuan", "这些图片通过局域网", Some("快传")),
            ("kuaichuan", "照片刚发完。这部小说的题材是", None),
            ("kuaichuan", "救援队出动一艘", Some("快船")),
            ("goujianhao", "查看当前软件的", Some("构建号")),
            ("goujianhao", "请核对更新包的", Some("构建号")),
            ("goujianhao", "请把安装包重新", None),
            ("goujianhao", "查看文档后。请看应用的", None),
            ("guanlitai", "打开知字的", Some("管理台")),
            ("guanlitai", "打开知字", Some("管理台")),
            ("guanlitai", "请登录服务端的", Some("管理台")),
            ("guanlitai", "", Some("观礼台")),
            ("guanlitai", "所建仿古建筑还原了宫廷的", Some("观礼台")),
            ("guanlitai", "打开门后。能看到宫廷的", Some("观礼台")),
            ("guanlitai", "打开仿古建筑里的", Some("观礼台")),
            ("yangli", "参考上次的测试", Some("样例")),
            ("yangli", "这个生日要按", None),
            ("dabiao", "请对这批图片逐张", Some("打标")),
            ("dabiao", "工厂排放已全部", None),
            ("shencha", "对原始数据继续", Some("深查")),
            ("shencha", "材料提交给部门", None),
            ("cuoci", "输入法首选出了", Some("错词")),
            ("cuoci", "投诉信里采用的", None),
            ("changshiban", "可以安装这个", Some("尝试版")),
            ("changshiban", "今天教堂的", Some("唱诗班")),
            ("changshiban", "", Some("唱诗班")),
            ("najige", "没通过的是", Some("哪几个")),
            ("najige", "刚从门口进来的", None),
            ("yiban", "我们先修订", Some("一版")),
            ("yiban", "先把表格填写", None),
            ("chushen", "材料已提交", Some("初审")),
            ("chushen", "他是一个农村", None),
            ("tichu", "所有不符合条件的样本全部", Some("剔除")),
            ("tichu", "各项建议已经书面", None),
            ("shiyong", "正式付款前可以免费", Some("试用")),
            ("shiyong", "厂商授予我们永久", None),
            ("queshi", "这份资料严重", Some("缺失")),
            ("queshi", "几份证据可以证明确实", None),
            ("mingming", "按新规则给目录重新", Some("命名")),
            ("mingming", "你昨天", None),
        ] {
            assert_eq!(exact_collocation(raw, context), expected, "{raw} {context}");
        }
    }

    #[cfg(feature = "mac-fast-typing")]
    #[test]
    fn abbreviation_distinguishes_an_active_request_from_a_competing_context() {
        for (raw, context, supported) in [
            ("sdyx", "我查一下库存，劳烦您", true),
            ("sdyx", "停电故障让设备也", false),
            ("mashangdf", "领导正在等回复，请", true),
            ("mashangdf", "领导正在等回复。价格预计", false),
            ("tmyjqrl", "前台说两位访客", false),
            ("tmyjqrl", "工作人员回复三位来访者", true),
            ("tmyjqrl", "还没联系上，前台说两位来宾", false),
            ("wmxzqck", "地下车位的车钥匙找不到", true),
            ("womenxzqck", "车停在地下车位，准备回去取车", true),
            ("womenxianzaiqck", "车钥匙已经找到，服务窗口开始受理", false),
            ("wmxzqck", "车停在地下车位，但不用去取车", false),
            ("wmxzqck", "旧车钥匙找不到。新材料已经齐了", false),
            ("wmyqxbl", "办事大厅刚开放，证件都备齐了", true),
            ("womenyiqixbl", "窗口说材料已经齐了，刚才又打烊了", false),
            ("wmyqxbl", "柜台说证件没带齐，还不能办理", false),
            ("womenyqxbl", "议案还没谈妥，窗口材料都齐了", false),
        ] {
            assert_eq!(context_sensitive_abbreviation(raw, context).map(|(_, matches)| matches),
                       Some(supported), "{raw} {context}");
        }
    }

    #[test]
    fn exact_offpage_does_not_treat_family_traditions_as_surnames() {
        assert_eq!(offpage_exact_collocation("户籍资料显示，这个家族世代沿用同一个"), Some(("xingshi", "姓氏")));
        assert_eq!(offpage_exact_collocation("祭祖用什么样的仪式，这个家族世代沿用同一个"), None);
        assert_eq!(offpage_exact_collocation("年会流程已经根深蒂固，这个家族世代沿用同一个"), None);
    }

    #[test]
    fn short_exact_context_requires_local_semantic_evidence() {
        for (raw, preceding, expected) in [
            ("shiwu", "处室文员负责经办部门日常", Some("事务")),
            ("shiwu", "等我处理完这些日常", None),
            ("shiwu", "那家小吃铺每天处理完这些日常", None),
            ("shiwu", "财务实习每天会处理完这些日常", None),
            ("shiwu", "大概还差两三天才到农历八月", Some("十五")),
            ("shiwu", "端午演出安排在阴历五月", Some("十五")),
            ("shiwu", "活动定在农历闰二月", Some("十五")),
            ("shiwu", "只是记录了农历八月", None),
            ("shiwu", "应该学着接受新鲜", None),
            ("zhishi", "学生还没掌握课本上的基础", Some("知识")),
            ("zhishi", "电视设备要保留基础", None),
            ("zhishi", "学生已经到齐，但项目的基础", None),
            ("shishi", "后台把传感器数据改成", Some("实时")),
            ("shishi", "取消定时设置后采集程序把定位信息切换为", Some("实时")),
            ("shishi", "上报不设固定频次，监控系统把定位信息设置为", None),
            ("shishi", "稿件中的推测应该改成", None),
            ("xingshi", "股东的表决权由本人依法", Some("行使")),
            ("xingshi", "董事会有权依法", None),
        ] {
            assert_eq!(short_exact_context(raw, preceding), expected, "{preceding}");
        }
    }

    #[test]
    fn only_exact_input_and_immediately_adjacent_text_match() {
        for (spelling, context, wanted) in [
            ("jiya", "订单增加大家在清理近期", Some("积压")),
            ("jiya", "维修工在处理今天", None),
            ("shiwu", "在科技馆发现了未曾见过的", Some("事物")),
            ("shiwu", "在饮食博物馆看见了从没见过的", None),
            ("wan", "麻烦您尽快办", Some("完")),
            ("wan", "事情已经办。", None),
            ("qian", "准备工作在会议开始", Some("前")),
            ("qian", "我还差一笔", None),
            ("dengji", "接待处这几位访客核实后可以", Some("登记")),
            ("dengji", "机场前台说客人的航班确认了可以", None),
            ("dengji", "来访手续已备好请办理", Some("登记")),
            ("dengji", "设备的风险需要重新核定", Some("等级")),
            ("yuanjian", "这份证照请妥善留存", Some("原件")),
            ("yuanjian", "更换主板上损坏的电子", Some("元件")),
            ("yuanjian", "昨天焊接。今天需要更换", None),
            ("yuan1jian", "这份证照请妥善留存", None),
            ("qile", "办证需要的材料都", Some("齐了")),
            ("qile", "这位同事今天", None),
            ("zaichufa", "出门前先把车加满油", Some("再出发")),
            ("zaichufa", "执法人员先办理", None),
            ("shupin", "这里卖的是生鲜而不是", Some("熟品")),
            ("shupin", "这次评选的是文章而不是", None),
        ] {
            assert_eq!(exact_collocation(spelling, context), wanted, "{context}");
        }
        assert_eq!(exact_collocation("shiwu", "犯了个低级"), Some("失误"));
        assert_eq!(exact_collocation("shiw", "犯了个低级"), None);
        assert_eq!(exact_collocation("shiwu", "并不低级。"), None);
        assert_eq!(exact_collocation("shiwu", "接受新鲜"), None);
        assert_eq!(exact_collocation("shiwu", ""), None);
        assert_eq!(exact_collocation("gaifa", "处理这类案件应依照"), Some("该法"));
        assert_eq!(exact_collocation("gaifa", "你先做这种"), Some("改法"));
        assert_eq!(exact_collocation("gaifa", "你先做这种，"), None);
        assert_eq!(exact_collocation("gaifa", "按流程根据"), None);
    }

    #[test]
    fn equal_longest_conflicts_abstain_until_more_specific_evidence() {
        let rules = [
            ("shiwu", "事物", "新鲜"),
            ("shiwu", "食物", "新鲜"),
            ("shiwu", "事物", "新鲜"),
            ("shiwu", "食物", "冰箱里的新鲜"),
            ("shiwu", "事物", "新鲜"),
        ];
        assert_eq!(preferred_word(&rules, "shiwu", "新鲜"), None);
        assert_eq!(preferred_word(&rules, "shiwu", "冰箱里的新鲜"), Some("食物"));
    }

    #[test]
    fn compound_clues_need_both_the_topic_and_the_ending_in_a_single_sentence() {
        assert_eq!(exact_collocation("zhiding", "请复制到财务预先"), Some("指定"));
        assert_eq!(exact_collocation("zhiding", "财务预先"), None);
        assert_eq!(exact_collocation("zhiding", "刚复制到硬盘。主任事先"), None);
        assert_eq!(exact_collocation("zhibiao", "这一做法是暂时缓解，通常只会"), Some("治标"));
        assert_eq!(exact_collocation("zhibiao", "说到治本。任务阶段只能"), None);
        assert_eq!(exact_collocation("zhishi", "焗饭上要铺碎屑，选一种能拉丝的"), Some("芝士"));
    }

    #[test]
    fn driving_and_rulemaking_cues_leave_other_homophones_open() {
        assert_eq!(exact_collocation("xingshi", "高速路两侧有雾，注意减速"), Some("行驶"));
        assert_eq!(exact_collocation("xingshi", "前方道路积水，车辆应当减速"), Some("行驶"));
        assert_eq!(exact_collocation("xingshi", "这里不走高速路。请注意减速"), None);
        assert_eq!(exact_collocation("xingshi", "董事会有权依法"), None);
        assert_eq!(exact_collocation("zhiding", "旧操作流程需要重新"), Some("制定"));
        assert_eq!(exact_collocation("zhiding", "现有考核制度必须重新"), Some("制定"));
        assert_eq!(exact_collocation("zhiding", "这个流程的负责人调走了，我们需要重新"), None);
    }

    #[test]
    fn narrow_cues_preserve_forms_without_both_signals() {
        assert_eq!(exact_collocation("zhiding", "请把那条通知在群里"), Some("置顶"));
        assert_eq!(exact_collocation("zhiding", "系统通知过我们，请在群里"), None);
        assert_eq!(exact_collocation("zhibiao", "每月统计要用软件重新"), Some("制表"));
        assert_eq!(exact_collocation("zhibiao", "季度考核请以客观"), None);
        assert_eq!(exact_collocation("shishi", "这是大家都清楚的客观"), Some("事实"));
        assert_eq!(exact_collocation("shishi", "请按批复客观"), None);
        assert_eq!(exact_collocation("shishi", "这些大家都清楚的，但新规要求客观"), None);
    }

    #[test]
    fn space_first_zhiding_requires_a_local_drafting_object_or_a_person_pool() {
        use super::space_first_zhiding_context as evidence;
        for context in [
            "合同附件还没写，验收细则应由双方共同",
            "负责人刚做完复盘，新的整改计划今天要重新",
            "例会的议事规则应由各单位共同",
            "评估守则会后还要重新",
            "风险预案月底前需要重新",
        ] {
            assert_eq!(evidence(context), Some("制定"), "{context}");
        }
        for context in [
            "监票员由组委会从三位委员之中正式",
            "报告人请从报名志愿者中临时",
            "主持人由理事长从几名代表当中直接",
            "奖项评委从入围专家中",
        ] {
            assert_eq!(evidence(context), Some("指定"), "{context}");
        }
        for context in [
            "规章制度写完后，院系联系人须由各部门共同",
            "这两套备用方案本周要重新",
            "培训办法从三份修改稿中正式",
            "起草组从三位委员提交的意见中正式",
            "项目报道提到了这份合同，麻烦把公告在群聊里",
            "校长先选了两位候选人，负责人明天要重新",
            "验收细则应由双方共同？",
        ] {
            assert_eq!(evidence(context), None, "{context}");
        }
    }
}
