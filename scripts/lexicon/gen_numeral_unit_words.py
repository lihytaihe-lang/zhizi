"""Write the numeral x measure-word table (几十秒, 十几分钟, ...) for append_owner_words.py.

Usage: gen_numeral_unit_words.py OUT.tsv, then
  append_owner_words.py --owner-sourced OUT.tsv --pattern-generated
The importer's generic model-number rejection is preserved. Reviewed compounds
may use matches_word() to recognize a complete numeral + unit, with exact reading.
"""
import sys

PREFIX = {"几十": "ji shi", "几百": "ji bai", "几千": "ji qian", "几万": "ji wan",
          "十几": "shi ji", "二十几": "er shi ji", "三十几": "san shi ji", "十来": "shi lai"}
UNIT = {"秒": "miao", "毫秒": "hao miao", "分钟": "fen zhong", "小时": "xiao shi", "天": "tian",
        "周": "zhou", "个月": "ge yue", "年": "nian", "次": "ci", "遍": "bian", "倍": "bei",
        "万": "wan", "亿": "yi", "里": "li", "公里": "gong li", "米": "mi", "厘米": "li mi",
        "毫米": "hao mi", "斤": "jin", "公斤": "gong jin", "吨": "dun", "岁": "sui", "页": "ye",
        "行": "hang", "台": "tai", "本": "ben", "位": "wei", "件": "jian", "辆": "liang",
        "层": "ceng", "度": "du", "块钱": "kuai qian", "句": "ju", "张": "zhang", "只": "zhi",
        "名": "ming", "套": "tao", "部": "bu", "个": "ge", "条": "tiao", "人": "ren", "块": "kuai",
        "元": "yuan", "种": "zhong", "家": "jia", "处": "chu", "篇": "pian", "封": "feng",
        "行代码": "hang dai ma", "条日志": "tiao ri zhi", "个字": "ge zi", "个字符": "ge zi fu",
        "兆": "zhao", "毫安": "hao an", "瓦": "wa", "伏": "fu", "赫兹": "he zi", "帧": "zhen",
        "个点": "ge dian", "个人": "ge ren", "分": "fen", "点": "dian", "根": "gen", "份": "fen",
        "项": "xiang", "笔": "bi", "场": "chang", "局": "ju", "轮": "lun", "步": "bu", "号": "hao"}

def matches_word(text, reading):
    return any(text == p + u and reading == f"{pr} {ur}"
               for p, pr in PREFIX.items() for u, ur in UNIT.items())


if __name__ == "__main__":
    rows = ["text\treading\toccurrences\treview"]
    for p, pr in PREFIX.items():
        for u, ur in UNIT.items():
            rows.append(f"{p}{u}\t{pr} {ur}\t3\tauto")
    open(sys.argv[1], "w", encoding="utf-8").write("\n".join(rows) + "\n")
