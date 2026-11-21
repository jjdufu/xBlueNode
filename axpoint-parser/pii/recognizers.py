from __future__ import annotations

import re
from datetime import date

from presidio_analyzer import EntityRecognizer, Pattern, PatternRecognizer, RecognizerResult
from presidio_analyzer.nlp_engine import NlpArtifacts

from schemas import PiiCategory

ID_CARD_WEIGHTS: tuple[int, ...] = (7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2)
ID_CARD_CHECK_CHARS = "10X98765432"

# Complete 81 traditional Chinese compound surnames (Step 13 §13.7 / §13.8.1).
COMPOUND_SURNAMES: frozenset[str] = frozenset(
    {
        "欧阳",
        "太史",
        "端木",
        "上官",
        "司马",
        "东方",
        "独孤",
        "南宫",
        "万俟",
        "闻人",
        "夏侯",
        "诸葛",
        "尉迟",
        "公羊",
        "赫连",
        "澹台",
        "皇甫",
        "宗政",
        "濮阳",
        "公冶",
        "太叔",
        "申屠",
        "公孙",
        "慕容",
        "仲孙",
        "钟离",
        "长孙",
        "宇文",
        "司徒",
        "鲜于",
        "司空",
        "闾丘",
        "子车",
        "亓官",
        "司寇",
        "巫马",
        "公西",
        "颛孙",
        "壤驷",
        "公良",
        "漆雕",
        "乐正",
        "宰父",
        "谷梁",
        "拓跋",
        "夹谷",
        "轩辕",
        "令狐",
        "段干",
        "百里",
        "呼延",
        "东郭",
        "南门",
        "羊舌",
        "微生",
        "公户",
        "公玉",
        "公仪",
        "梁丘",
        "公仲",
        "公上",
        "公门",
        "公山",
        "公坚",
        "左丘",
        "公伯",
        "西门",
        "公祖",
        "第五",
        "公乘",
        "贯丘",
        "公皙",
        "南荣",
        "东里",
        "东宫",
        "仲长",
        "子书",
        "子桑",
        "即墨",
        "达奚",
        "褚师",
    }
)

# Common single-character Chinese surnames (~180 high-frequency surnames).
SINGLE_SURNAMES: frozenset[str] = frozenset(
    "王李张刘陈杨赵黄周吴徐孙胡朱高林何郭马罗梁宋郑谢韩唐冯于董萧程曹袁邓许傅沈曾彭吕"
    "苏卢蒋蔡贾丁魏薛叶阎余潘杜戴夏钟汪田任姜范方石姚谭廖邹熊金陆郝孔白崔康毛邱秦江史"
    "顾侯邵孟龙万段漕钱汤尹黎易常武乔贺赖龚文肖闫付庞樊兰殷施陶洪翟安颜倪严牛温芦季俞"
    "章鲁葛伍韦申尤毕聂丛焦向柳邢路岳齐沿梅莫庄辛管祝左涂谷祁时舒耿牟卜詹关苗凌费纪靳"
    "盛童欧甄项曲游阳裴席卫查屈鲍位覃霍翁隋植甘景薄单包司柏宁柯阮桂闵解强柴华车冉房边"
)

ENTITY_TO_CATEGORY: dict[str, PiiCategory] = {
    "CN_PHONE": "PHONE",
    "PHONE": "PHONE",
    "PHONE_NUMBER": "PHONE",
    "CN_ID_CARD": "ID_CARD",
    "ID_CARD": "ID_CARD",
    "BANK_CARD": "BANK_CARD",
    "CREDIT_CARD": "BANK_CARD",
    "EMAIL": "EMAIL",
    "EMAIL_ADDRESS": "EMAIL",
    "CN_ADDRESS": "ADDRESS",
    "ADDRESS": "ADDRESS",
    "PERSON": "NAME",
    "NAME": "NAME",
    "NR": "NAME",
    "SECRET": "SECRET",
}

_CN_PHONE_RE = re.compile(r"(?<!\d)(?:\+?86[-\s]?)?(1[3-9]\d{9})(?!\d)")
_CN_PHONE_SEP_RE = re.compile(r"(?<!\d)(?:\+86[-\s]?)?(1[3-9]\d[-\s]\d{4}[-\s]\d{4})(?!\d)")

_CN_ID_CARD_PATTERN = (
    r"(?<![0-9A-Za-z])"
    r"[1-9]\d{5}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]"
    r"(?![0-9A-Za-z])"
)

_BANK_CARD_CONTIG_PATTERN = r"(?<!\d)[3-6]\d{15,18}(?!\d)"
_BANK_CARD_SEP_PATTERN = r"(?<!\d)[3-6]\d{3}(?:[-\s]\d{4}){3}(?:[-\s]?\d{1,3})?(?!\d)"

_EMAIL_PATTERN = (
    r"(?<![A-Za-z0-9._%+-])"
    r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"
    r"(?![A-Za-z0-9_-])"
)

# Known Chinese provincial-level regions for exact left-boundary anchoring in free text.
_PROVINCE_ANCHOR = (
    r"(?:北京市|天津市|上海市|重庆市|"
    r"(?:河北|山西|辽宁|吉林|黑龙江|江苏|浙江|安徽|福建|江西|山东|河南|湖北|湖南|广东|"
    r"海南|四川|贵州|云南|陕西|甘肃|青海|台湾)省|"
    r"(?:内蒙古|广西壮族|广西|西藏|宁夏回族|宁夏|新疆维吾尔|新疆)自治区|"
    r"(?:香港|澳门)特别行政区)"
)

_STREET_SEGMENT = (
    r"[\u4e00-\u9fffA-Za-z0-9\-]{1,16}?"
    r"(?:大道|大街|中路|南路|北路|东路|西路|路|街|巷|弄|胡同|条|"
    r"高科技园区|软件园|工业园区|工业园|科技园|产业园|园区|"
    r"大厦|广场|中心|花园|小区|新村|佳苑|苑|"
    r"号楼|号院|号|栋|幢|座|单元|层|楼|室|房)"
)

_CN_ADDRESS_PROV_RE = re.compile(
    rf"{_PROVINCE_ANCHOR}"
    r"(?:[\u4e00-\u9fff]{1,10}?(?:自治州|地区|盟|市))?"
    r"(?:[\u4e00-\u9fff]{1,10}?(?:自治县|自治旗|新区|工业园区|区|县|旗))?"
    r"(?:[\u4e00-\u9fff]{1,8}?(?:街道|镇|乡))?"
    rf"(?:{_STREET_SEGMENT}){{1,6}}"
)

_CN_ADDRESS_CITY_RE = re.compile(
    r"(?<![\u4e00-\u9fff])"
    r"(?:(?:住在|位于|寄往|发往|送至|前往|地址为|地址是|住址为|住址是|在|于|到))?"
    r"([\u4e00-\u9fff]{2,4}市"
    r"[\u4e00-\u9fff]{1,8}?(?:自治县|新区|工业园区|区|县|旗)"
    r"(?:[\u4e00-\u9fff]{1,8}?(?:街道|镇|乡))?"
    rf"(?:{_STREET_SEGMENT}){{1,6}})"
)

_ADDR_STREET_HINT_RE = re.compile(
    r"(?:大道|大街|路|街|巷|弄|胡同|号|室|栋|幢|单元|大厦|园区|软件园|工业园|科技园|小区|广场)"
)

# Structural/unit/geography characters that do not appear as given-name characters in 2-3 char names
_NON_NAME_GIVEN_CHARS: frozenset[str] = frozenset(
    "省市区县旗州盟镇乡村路街巷弄号栋幢室楼层座部司局院所厂店库仓站港馆校系组队科处办"
    "品件箱台个元角分秒度率额量价费税单据表图线体色级类款型版态合计总待已未无"
    "的了着过吗呢吧啊"
)

_COMPOUND_NON_PERSON_SUFFIX_RE = re.compile(
    r"(?:财富|明珠|航空|卫视|证券|集团|公司|银行|大学|学院|医院|广场|大厦|花园|软件园|"
    r"工业园|科技园|道路|大道|大街|路|街|门外|门内|挑一|公里)$"
)

_KNOWN_NON_PERSON_WORDS: frozenset[str] = frozenset(
    {
        "北京",
        "上海",
        "天津",
        "重庆",
        "苏州",
        "杭州",
        "南京",
        "广州",
        "深圳",
        "成都",
        "武汉",
        "西安",
        "郑州",
        "徐州",
        "常州",
        "温州",
        "金华",
        "宁波",
        "沈阳",
        "兰州",
        "柳州",
        "沧州",
        "湖州",
        "荆州",
        "泉州",
        "福州",
        "厦门",
        "青岛",
        "济南",
        "大连",
        "长春",
        "合肥",
        "南昌",
        "长沙",
        "贵阳",
        "昆明",
        "南宁",
        "海口",
        "太原",
        "银川",
        "西宁",
        "拉萨",
        "东莞",
        "佛山",
        "无锡",
        "珠海",
        "惠州",
        "中山",
        "嘉兴",
        "绍兴",
        "台州",
        "烟台",
        "潍坊",
        "洛阳",
        "襄阳",
        "宜昌",
        "株洲",
        "湘潭",
        "衡阳",
        "汕头",
        "湛江",
        "肇庆",
        "桂林",
        "三亚",
        "绵阳",
        "遵义",
        "咸阳",
        "宝鸡",
        "包头",
        "赤峰",
        "鞍山",
        "吉林",
        "大庆",
        "江苏",
        "浙江",
        "山东",
        "广东",
        "河南",
        "四川",
        "湖北",
        "湖南",
        "河北",
        "福建",
        "安徽",
        "辽宁",
        "陕西",
        "江西",
        "广西",
        "云南",
        "贵州",
        "山西",
        "甘肃",
        "海南",
        "宁夏",
        "青海",
        "西藏",
        "新疆",
        "香港",
        "澳门",
        "台湾",
        "中国",
        "中华",
        "经理",
        "主任",
        "总监",
        "主管",
        "专员",
        "客服",
        "服务",
        "代表",
        "客户",
        "员工",
        "用户",
        "人员",
        "姓名",
        "联系",
        "负责",
        "审批",
        "申请",
        "工程",
        "工具",
        "工作",
        "工资",
        "工业",
        "方案",
        "方式",
        "方法",
        "金融",
        "黄金",
        "林业",
        "黄河",
        "常规",
        "常用",
        "常量",
        "程序",
        "石材",
        "石油",
        "田野",
        "夏天",
        "历史",
        "顾问",
        "顾虑",
        "客观",
        "段落",
        "钱款",
        "容易",
        "武器",
        "乔木",
        "祝贺",
        "依赖",
        "文件",
        "文档",
        "文本",
        "文字",
        "文化",
        "文具",
        "文章",
        "文明",
        "安全",
        "安防",
        "安装",
        "安排",
        "安静",
        "牛奶",
        "温暖",
        "温和",
        "温泉",
        "章节",
        "章程",
        "规则",
        "规模",
        "规划",
        "规格",
        "规范",
        "规定",
        "聚焦",
        "向导",
        "路径",
        "对齐",
        "齐全",
        "边缘",
        "边界",
        "房屋",
        "房产",
        "车辆",
        "强弱",
        "强制",
        "强调",
        "解决",
        "解析",
        "解释",
        "解除",
        "包装",
        "包含",
        "包括",
        "包裹",
        "单独",
        "单纯",
        "景观",
        "景色",
        "植物",
        "查询",
        "查看",
        "卫生",
        "卫星",
        "出席",
        "阳光",
        "游戏",
        "游客",
        "曲折",
        "项目",
        "儿童",
        "童年",
        "盛大",
        "纪念",
        "纪律",
        "纪要",
        "凌晨",
        "关闭",
        "关联",
        "关系",
        "关键",
        "关注",
        "关于",
        "舒适",
        "山谷",
        "谷物",
        "涂料",
        "祝福",
        "管理",
        "管道",
        "管控",
        "辛苦",
        "庄园",
        "梅花",
        "沿海",
        "沿途",
    }
)

_PERSON_LEFT_CUE_RE = re.compile(
    r"(?:姓名|客户姓名|联系人|收件人|收货人|发件人|寄件人|持卡人|收款人|付款人|"
    r"经办人|审批人|负责人|申请人|户主|法人|客户|员工|用户|患者|"
    r"请联系|联系|致电|转交|交给|名叫|名为|叫做)"
    r"[:：\s为是]*([\u4e00-\u9fff]{2,4})"
)

_PERSON_RIGHT_CUE_RE = re.compile(
    r"(?<![\u4e00-\u9fff])([\u4e00-\u9fff]{2,4})"
    r"(?:先生|女士|同志|经理|主任|总监|老师|医生|工程师|"
    r"的(?:手机|电话|联系方式|身份证|证件|银行卡|卡号|邮箱|邮件|地址|住址|账户|账号|密钥|口令))"
)

_COMPOUND_IN_TEXT_RE = re.compile(
    r"(?:(?:" + "|".join(sorted(COMPOUND_SURNAMES)) + r")[\u4e00-\u9fff]{1,2})"
)

# Standalone or clause-delimited common Chinese names (e.g. "张三，...", "与李四、王五").
_CLAUSE_NAME_RE = re.compile(
    r"(?:^|(?<=[，。；：、！？\s（(【\[“\"'与和向给由找对跟同及]))"
    r"([\u4e00-\u9fff]{2,3})"
    r"(?=$|[，。；：、！？\s）)】\]”\"'的与和及])"
)

_CLASSIC_NAMES: frozenset[str] = frozenset(
    {"张三", "李四", "王五", "赵六", "孙七", "周八", "吴九", "郑十", "李四光", "王小明", "张小伟"}
)

_FOLLOWING_NON_PERSON_CHARS: frozenset[str] = frozenset(
    "路街道巷弄桥村镇乡区县市省园苑楼厦厂店司部局院"
)


def is_valid_gb11643_id_card(raw: str) -> bool:
    """Validate 18-digit Chinese resident ID card with GB 11643-1999 mod-11-2 checksum."""
    s = raw.strip().upper()
    if len(s) != 18 or not s[:17].isdigit() or s[0] == "0":
        return False
    try:
        year = int(s[6:10])
        month = int(s[10:12])
        day = int(s[12:14])
        if not (1900 <= year <= 2099):
            return False
        date(year, month, day)
    except ValueError:
        return False

    checksum_idx = sum((ord(s[i]) - 48) * ID_CARD_WEIGHTS[i] for i in range(17)) % 11
    return s[17] == ID_CARD_CHECK_CHARS[checksum_idx]


def is_valid_luhn_bank_card(raw: str) -> bool:
    """Validate 16..19 digit bank card sequence via prefix [3-6] and Luhn mod-10 checksum."""
    digits = re.sub(r"[-\s]", "", raw.strip())
    if not (16 <= len(digits) <= 19) or not digits.isdigit():
        return False
    if digits[0] not in {"3", "4", "5", "6"}:
        return False
    if len(digits) == 18 and is_valid_gb11643_id_card(digits):
        return False
    total = 0
    for idx, ch in enumerate(reversed(digits)):
        d = ord(ch) - 48
        if idx % 2 == 1:
            d *= 2
            if d > 9:
                d -= 9
        total += d
    return total % 10 == 0


def _is_plausible_cn_name(candidate: str, *, allow_single_surname: bool = True) -> bool:
    """Check if a 2..4 CJK token is a plausible Chinese compound- or single-surname name."""
    n = len(candidate)
    if n < 2 or n > 4:
        return False
    if not all("\u4e00" <= ch <= "\u9fff" for ch in candidate):
        return False
    if candidate in _KNOWN_NON_PERSON_WORDS:
        return False
    if candidate in _CLASSIC_NAMES:
        return True

    if n >= 3 and candidate[:2] in COMPOUND_SURNAMES:
        given = candidate[2:]
        if _COMPOUND_NON_PERSON_SUFFIX_RE.search(candidate):
            return False
        return not any(ch in _NON_NAME_GIVEN_CHARS for ch in given)

    if allow_single_surname and n in {2, 3} and candidate[0] in SINGLE_SURNAMES:
        given = candidate[1:]
        return not any(ch in _NON_NAME_GIVEN_CHARS for ch in given)

    return False


class CnPhoneRecognizer(EntityRecognizer):
    """Mainland China mobile phone recognizer (`CN_PHONE` -> `PHONE`)."""

    def __init__(self, supported_language: str = "zh") -> None:
        super().__init__(
            supported_entities=["PHONE"],
            name="CnPhoneRecognizer",
            supported_language=supported_language,
        )

    def load(self) -> None:
        return None

    def analyze(
        self,
        text: str,
        entities: list[str],
        nlp_artifacts: NlpArtifacts | None = None,
    ) -> list[RecognizerResult]:
        if entities and "PHONE" not in entities:
            return []
        results: list[RecognizerResult] = []
        seen_spans: set[tuple[int, int]] = set()

        for regex in (_CN_PHONE_RE, _CN_PHONE_SEP_RE):
            for match in regex.finditer(text):
                start, end = match.span(0)
                token = text[start:end]
                # If matched bare '86 ' without '+', trim to the 11-digit core phone token.
                if token.startswith("86") and not token.startswith("+"):
                    start, end = match.span(1)
                span_key = (start, end)
                if span_key in seen_spans:
                    continue
                seen_spans.add(span_key)
                results.append(
                    RecognizerResult(
                        entity_type="PHONE",
                        start=start,
                        end=end,
                        score=0.90,
                    )
                )
        return results


class CnIdCardRecognizer(PatternRecognizer):
    """18-digit Chinese ID card recognizer with GB 11643-1999 mod-11-2 checksum (`ID_CARD`)."""

    def __init__(self, supported_language: str = "zh") -> None:
        patterns = [
            Pattern(name="cn_id_card_18", regex=_CN_ID_CARD_PATTERN, score=0.96),
        ]
        super().__init__(
            supported_entity="ID_CARD",
            name="CnIdCardRecognizer",
            supported_language=supported_language,
            patterns=patterns,
        )

    def validate_result(self, pattern_text: str) -> bool | None:
        return is_valid_gb11643_id_card(pattern_text)

    def analyze(
        self,
        text: str,
        entities: list[str],
        nlp_artifacts: NlpArtifacts | None = None,
        regex_flags: int | None = None,
    ) -> list[RecognizerResult]:
        raw_results = super().analyze(text, entities, nlp_artifacts, regex_flags)
        validated: list[RecognizerResult] = []
        for res in raw_results:
            if res.score > 0.0 and is_valid_gb11643_id_card(text[res.start : res.end]):
                res.score = 0.96
                validated.append(res)
        return validated


class BankCardRecognizer(PatternRecognizer):
    """16..19 digit bank card recognizer with Luhn checksum (`BANK_CARD`)."""

    def __init__(self, supported_language: str = "zh") -> None:
        patterns = [
            Pattern(name="bank_card_16_19", regex=_BANK_CARD_CONTIG_PATTERN, score=0.92),
            Pattern(name="bank_card_grouped", regex=_BANK_CARD_SEP_PATTERN, score=0.92),
        ]
        super().__init__(
            supported_entity="BANK_CARD",
            name="BankCardRecognizer",
            supported_language=supported_language,
            patterns=patterns,
        )

    def validate_result(self, pattern_text: str) -> bool | None:
        return is_valid_luhn_bank_card(pattern_text)

    def analyze(
        self,
        text: str,
        entities: list[str],
        nlp_artifacts: NlpArtifacts | None = None,
        regex_flags: int | None = None,
    ) -> list[RecognizerResult]:
        raw_results = super().analyze(text, entities, nlp_artifacts, regex_flags)
        validated: list[RecognizerResult] = []
        for res in raw_results:
            snippet = text[res.start : res.end]
            if res.score > 0.0 and is_valid_luhn_bank_card(snippet):
                res.score = 0.92
                validated.append(res)
        return validated


class EmailAddressRecognizer(PatternRecognizer):
    """RFC-ish email address recognizer (`EMAIL`)."""

    def __init__(self, supported_language: str = "zh") -> None:
        patterns = [
            Pattern(name="rfc_email", regex=_EMAIL_PATTERN, score=0.92),
        ]
        super().__init__(
            supported_entity="EMAIL",
            name="EmailAddressRecognizer",
            supported_language=supported_language,
            patterns=patterns,
        )

    def validate_result(self, pattern_text: str) -> bool | None:
        s = pattern_text.strip()
        if len(s) > 254 or ".." in s:
            return False
        local, sep, domain = s.partition("@")
        if not sep or not local or not domain:
            return False
        return not (domain.startswith((".", "-")) or domain.endswith((".", "-")))

    def analyze(
        self,
        text: str,
        entities: list[str],
        nlp_artifacts: NlpArtifacts | None = None,
        regex_flags: int | None = None,
    ) -> list[RecognizerResult]:
        raw_results = super().analyze(text, entities, nlp_artifacts, regex_flags)
        out: list[RecognizerResult] = []
        for res in raw_results:
            if res.score > 0.0 and self.validate_result(text[res.start : res.end]):
                res.score = 0.92
                out.append(res)
        return out


class CnAddressRecognizer(EntityRecognizer):
    """Chinese administrative + street/building/number address recognizer (`ADDRESS`)."""

    def __init__(self, supported_language: str = "zh") -> None:
        super().__init__(
            supported_entities=["ADDRESS"],
            name="CnAddressRecognizer",
            supported_language=supported_language,
        )

    def load(self) -> None:
        return None

    def analyze(
        self,
        text: str,
        entities: list[str],
        nlp_artifacts: NlpArtifacts | None = None,
    ) -> list[RecognizerResult]:
        if entities and "ADDRESS" not in entities:
            return []

        candidates: list[RecognizerResult] = []
        for m in _CN_ADDRESS_PROV_RE.finditer(text):
            start, end = m.span(0)
            matched = text[start:end]
            if len(matched) >= 6 and _ADDR_STREET_HINT_RE.search(matched):
                candidates.append(
                    RecognizerResult(
                        entity_type="ADDRESS",
                        start=start,
                        end=end,
                        score=0.88,
                    )
                )

        for m in _CN_ADDRESS_CITY_RE.finditer(text):
            start, end = m.span(1)
            matched = text[start:end]
            if len(matched) < 6 or not _ADDR_STREET_HINT_RE.search(matched):
                continue
            # Skip if already covered by a province-anchored span.
            if any(c.start <= start and end <= c.end for c in candidates):
                continue
            candidates.append(
                RecognizerResult(
                    entity_type="ADDRESS",
                    start=start,
                    end=end,
                    score=0.86,
                )
            )

        return candidates


class CnPersonRecognizer(EntityRecognizer):
    """Chinese person name recognizer combining spaCy PERSON/NR entities and surname rules."""

    def __init__(self, supported_language: str = "zh") -> None:
        super().__init__(
            supported_entities=["NAME"],
            name="CnPersonRecognizer",
            supported_language=supported_language,
        )

    def load(self) -> None:
        return None

    def analyze(
        self,
        text: str,
        entities: list[str],
        nlp_artifacts: NlpArtifacts | None = None,
    ) -> list[RecognizerResult]:
        if entities and "NAME" not in entities:
            return []

        best_by_span: dict[tuple[int, int], float] = {}

        def _record(start: int, end: int, score: float) -> None:
            if start >= end:
                return
            if end < len(text) and text[end] in _FOLLOWING_NON_PERSON_CHARS:
                return
            key = (start, end)
            if score > best_by_span.get(key, 0.0):
                best_by_span[key] = score

        # 1. Inspect spaCy NER entities when zh_core_web_sm (or similar) is loaded.
        if nlp_artifacts is not None and getattr(nlp_artifacts, "entities", None):
            for ent in nlp_artifacts.entities:
                label = str(getattr(ent, "label_", ""))
                if label not in {"PERSON", "PER", "NR"}:
                    continue
                s_idx = int(getattr(ent, "start_char", 0))
                e_idx = int(getattr(ent, "end_char", 0))
                ent_str = text[s_idx:e_idx].strip()
                if len(ent_str) < 2 or ent_str in _KNOWN_NON_PERSON_WORDS:
                    continue
                if any(ch in _NON_NAME_GIVEN_CHARS for ch in ent_str[1:]):
                    continue
                has_surname = (len(ent_str) >= 3 and ent_str[:2] in COMPOUND_SURNAMES) or (
                    ent_str[0] in SINGLE_SURNAMES
                )
                score = 0.88 if has_surname else 0.72
                _record(s_idx, e_idx, score)

        # 2. Standalone cell value (e.g. column profiling or exact name input).
        stripped = text.strip()
        if 2 <= len(stripped) <= 4 and _is_plausible_cn_name(stripped, allow_single_surname=True):
            lead = len(text) - len(text.lstrip())
            _record(lead, lead + len(stripped), 0.86)

        # 3. Compound-surname names in free-form text (81 compound surnames + 1..2 given chars).
        for m in _COMPOUND_IN_TEXT_RE.finditer(text):
            s_idx, e_idx = m.span(0)
            # Do not match if immediately preceded or followed by extra CJK given chars > 4 total
            cand = text[s_idx:e_idx]
            if _is_plausible_cn_name(cand, allow_single_surname=False):
                _record(s_idx, e_idx, 0.88)

        # 4. Contextually cued single/compound surname names in free-form text.
        for m in _PERSON_LEFT_CUE_RE.finditer(text):
            s_idx, e_idx = m.span(1)
            raw_cand = text[s_idx:e_idx]
            # Trim trailing possessive/particle '的' if greedily captured in 2..4 window.
            for trim_len in (len(raw_cand), 3, 2):
                if trim_len < 2 or trim_len > len(raw_cand):
                    continue
                sub = raw_cand[:trim_len]
                if _is_plausible_cn_name(sub, allow_single_surname=True):
                    _record(s_idx, s_idx + trim_len, 0.87)
                    break

        for m in _PERSON_RIGHT_CUE_RE.finditer(text):
            s_idx, e_idx = m.span(1)
            raw_cand = text[s_idx:e_idx]
            # Try matching suffix 4, 3, or 2 chars in case leading verb was attached.
            for sub_len in (len(raw_cand), 3, 2):
                if sub_len < 2 or sub_len > len(raw_cand):
                    continue
                offset = len(raw_cand) - sub_len
                sub = raw_cand[offset:]
                if _is_plausible_cn_name(sub, allow_single_surname=True):
                    _record(s_idx + offset, e_idx, 0.87)
                    break

        # 5. Clause-delimited classic / surname names in free-form text (offline fallback).
        for m in _CLAUSE_NAME_RE.finditer(text):
            s_idx, e_idx = m.span(1)
            cand = text[s_idx:e_idx]
            if cand in _CLASSIC_NAMES or _is_plausible_cn_name(cand, allow_single_surname=True):
                _record(s_idx, e_idx, 0.82 if cand not in _CLASSIC_NAMES else 0.86)

        return [
            RecognizerResult(entity_type="NAME", start=s, end=e, score=sc)
            for (s, e), sc in sorted(best_by_span.items())
        ]


class SecretRecognizer(PatternRecognizer):
    """API keys, private key PEM headers, bearer tokens, and secret assignments (`SECRET`)."""

    def __init__(self, supported_language: str = "zh") -> None:
        patterns = [
            Pattern(
                name="pem_private_key",
                regex=r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----",
                score=0.98,
            ),
            Pattern(
                name="sk_api_key",
                regex=r"(?<![A-Za-z0-9])sk-[A-Za-z0-9_-]{20,}",
                score=0.98,
            ),
            Pattern(
                name="cloud_access_key",
                regex=r"(?<![A-Z0-9])AKIA[0-9A-Z]{16}(?![A-Z0-9])",
                score=0.98,
            ),
            Pattern(
                name="axpoint_pat",
                regex=r"(?<![A-Za-z0-9])axp_[A-Za-z0-9]{8}_[A-Za-z0-9]{32}(?![A-Za-z0-9])",
                score=0.98,
            ),
            Pattern(
                name="bearer_token",
                regex=r"(?<![A-Za-z0-9])Bearer\s+[A-Za-z0-9._\-]{24,}",
                score=0.96,
            ),
            Pattern(
                name="kv_secret_assignment",
                regex=(
                    r"(?i)\b(?:password|passwd|secret|token|api[_-]?key|private[_-]?key)"
                    r"\s*[:=]\s*[^\s,;\"'}\]]+"
                ),
                score=0.95,
            ),
        ]
        super().__init__(
            supported_entity="SECRET",
            name="SecretRecognizer",
            supported_language=supported_language,
            patterns=patterns,
        )


def build_custom_recognizers(languages: tuple[str, ...] = ("zh", "en")) -> list[EntityRecognizer]:
    """Instantiate the full AxGuard custom recognizer suite for all supported languages."""
    recognizers: list[EntityRecognizer] = []
    for lang in languages:
        recognizers.extend(
            [
                CnPhoneRecognizer(supported_language=lang),
                CnIdCardRecognizer(supported_language=lang),
                BankCardRecognizer(supported_language=lang),
                EmailAddressRecognizer(supported_language=lang),
                CnAddressRecognizer(supported_language=lang),
                CnPersonRecognizer(supported_language=lang),
                SecretRecognizer(supported_language=lang),
            ]
        )
    return recognizers


__all__ = [
    "BankCardRecognizer",
    "COMPOUND_SURNAMES",
    "CnAddressRecognizer",
    "CnIdCardRecognizer",
    "CnPersonRecognizer",
    "CnPhoneRecognizer",
    "ENTITY_TO_CATEGORY",
    "EmailAddressRecognizer",
    "ID_CARD_CHECK_CHARS",
    "ID_CARD_WEIGHTS",
    "SINGLE_SURNAMES",
    "SecretRecognizer",
    "build_custom_recognizers",
    "is_valid_gb11643_id_card",
    "is_valid_luhn_bank_card",
]
