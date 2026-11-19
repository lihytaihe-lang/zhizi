"""中文提示文案工具：输入法学习设置窗口的错误说明与容量状态格式化。"""

_ERROR_PATTERNS = (
    (("undo expired",), "撤销已超过30分钟，请刷新列表。"),
    (("no such file", "not found"), "所选记录或备份已变化，请刷新后重试。"),
    (("another learning manager", "would block"), "学习数据正在保存，请稍后重试。"),
    (("capacity", "storage full"), "合并将超过学习容量，现有学习记录未改动。"),
    (("permission", "private journal"), "学习目录权限检查未通过，请检查当前账户的访问权限。"),
    (("invalid", "too large", "backup"), "备份格式或大小未通过校验，现有学习记录未改动。"),
)

_FALLBACK_MESSAGE = "操作未完成，请刷新后重试。"


def describe_error(detail: str) -> str:
    """按既定顺序匹配并返回中文提示，不回显原始 detail。"""
    text = detail.lower() if isinstance(detail, str) else ""
    for keywords, message in _ERROR_PATTERNS:
        if any(keyword in text for keyword in keywords):
            return message
    return _FALLBACK_MESSAGE


def _nonnegative_int(status: dict, key: str) -> int:
    value = status.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        return 0
    return value


def format_capacity(status: dict) -> str:
    """把容量状态字典格式化为中文说明，缺失或非法字段按 0 显示。"""
    if not isinstance(status, dict):
        status = {}
    entries = _nonnegative_int(status, "entries")
    choices = _nonnegative_int(status, "choices")
    estimated_bytes = _nonnegative_int(status, "estimatedBytes")
    max_bytes = _nonnegative_int(status, "maxBytes")
    max_entries = _nonnegative_int(status, "maxEntries")
    max_age_days = _nonnegative_int(status, "maxAgeDays")
    return (
        f"学习词 {entries} / {max_entries}，选择记录 {choices} 条；"
        f"预计占用 {estimated_bytes:,} 字节，"
        f"容量上限 {max_bytes:,} 字节，"
        f"学习记录保留 {max_age_days} 天。"
    )