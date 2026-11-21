from __future__ import annotations

import base64
import binascii
import hashlib
import hmac
import logging
import os
import re
import threading
from collections.abc import Sequence

import spacy
from presidio_analyzer import AnalyzerEngine, RecognizerRegistry, RecognizerResult
from presidio_analyzer.nlp_engine import SpacyNlpEngine
from presidio_anonymizer import AnonymizerEngine
from presidio_anonymizer.entities import OperatorConfig
from presidio_anonymizer.entities import RecognizerResult as AnonRecognizerResult

from pii.recognizers import (
    COMPOUND_SURNAMES,
    ENTITY_TO_CATEGORY,
    build_custom_recognizers,
)
from schemas import (
    MaskRuleLiteral,
    PiiAnalyzeRequest,
    PiiAnalyzeResponse,
    PiiAnonymizeRequest,
    PiiAnonymizeResponse,
    PiiCategory,
    PiiSpanModel,
)
from security import SidecarError

logger = logging.getLogger(__name__)

MASK_KEYS_ENV_VAR = "AXPOINT_MASK_KEYS"

DEFAULT_MASK_RULES: dict[PiiCategory, MaskRuleLiteral] = {
    "NONE": "NONE",
    "PHONE": "MASK_PHONE",
    "ID_CARD": "MASK_ID_CARD",
    "EMAIL": "MASK_EMAIL",
    "BANK_CARD": "MASK_BANK_CARD",
    "NAME": "MASK_NAME",
    "ADDRESS": "MASK_ADDRESS",
    "SECRET": "REDACT",
}

CATEGORY_PRIORITY: dict[PiiCategory, int] = {
    "SECRET": 7,
    "ID_CARD": 6,
    "BANK_CARD": 5,
    "PHONE": 4,
    "EMAIL": 3,
    "ADDRESS": 2,
    "NAME": 1,
    "NONE": 0,
}

_PHONE_STRIP_RE = re.compile(r"[\s\-()]")
_CARD_STRIP_RE = re.compile(r"[\s\-]")

# Sequential Chinese administrative hierarchy (province -> prefecture/city -> county/district).
_MASK_ADDR_PREFIX_RE = re.compile(
    r"^("
    r"(?:[^省市区县旗路街道巷弄号镇乡楼室]{1,15}?(?:自治区|特别行政区|省))?"
    r"(?:[^省市区县旗路街道巷弄号镇乡楼室]{1,15}?(?:自治州|盟|市))?"
    r"(?:[^省市区县旗路街道巷弄号镇乡楼室]{1,15}?(?:工业园区|新区|自治县|自治旗|区|县|旗))?"
    r")"
)

_ephemeral_mask_key_lock = threading.Lock()
_ephemeral_mask_key: bytes | None = None
_derived_key_cache_lock = threading.Lock()
_derived_key_cache: dict[tuple[str, str], bytes] = {}


def _normalize_halfwidth(text: str) -> str:
    """Convert fullwidth ASCII characters (FF01..FF5E) and ideographic space to halfwidth."""
    out: list[str] = []
    changed = False
    for ch in text:
        code = ord(ch)
        if 0xFF01 <= code <= 0xFF5E:
            out.append(chr(code - 0xFEE0))
            changed = True
        elif code == 0x3000:
            out.append(" ")
            changed = True
        else:
            out.append(ch)
    return "".join(out) if changed else text


def _decode_b64_key(b64_part: str) -> bytes:
    cleaned = b64_part.strip()
    padding = "=" * ((4 - len(cleaned) % 4) % 4)
    try:
        return base64.b64decode(cleaned + padding, validate=False)
    except binascii.Error as exc:
        raise SidecarError(500, "INTERNAL", f"invalid base64 mask key: {exc}") from exc


def _resolve_active_master_key(raw_keys_override: str | None = None) -> tuple[str, bytes]:
    """Parse active (highest version) 32-byte master key from AXPOINT_MASK_KEYS or ephemeral."""
    raw = (
        raw_keys_override
        if raw_keys_override is not None
        else os.getenv(MASK_KEYS_ENV_VAR, "")
    ).strip()

    if not raw:
        env_mode = os.getenv("AXPOINT_ENV", "development").strip().lower()
        if env_mode == "production":
            raise SidecarError(
                500,
                "INTERNAL",
                f"{MASK_KEYS_ENV_VAR} must be configured when AXPOINT_ENV=production",
            )
        global _ephemeral_mask_key
        with _ephemeral_mask_key_lock:
            if _ephemeral_mask_key is None:
                logger.warning("%s unset; using ephemeral 32-byte key", MASK_KEYS_ENV_VAR)
                _ephemeral_mask_key = os.urandom(32)
            return "__ephemeral__", _ephemeral_mask_key

    best_ver = -1
    best_key: bytes | None = None
    seen_versions: set[int] = set()
    for segment in raw.split(","):
        entry = segment.strip()
        if not entry or ":" not in entry:
            raise SidecarError(500, "INTERNAL", f"malformed {MASK_KEYS_ENV_VAR} entry")
        ver_str, b64_str = entry.split(":", 1)
        try:
            ver = int(ver_str.strip())
        except ValueError as exc:
            raise SidecarError(500, "INTERNAL", "invalid mask key version") from exc
        if ver <= 0 or ver in seen_versions:
            raise SidecarError(500, "INTERNAL", f"invalid or duplicate mask key version {ver}")
        seen_versions.add(ver)
        decoded = _decode_b64_key(b64_str)
        if len(decoded) != 32:
            raise SidecarError(
                500,
                "INTERNAL",
                f"mask key v{ver} decoded to {len(decoded)} bytes, expected 32",
            )
        if ver > best_ver:
            best_ver = ver
            best_key = decoded

    if best_key is None:
        raise SidecarError(500, "INTERNAL", f"{MASK_KEYS_ENV_VAR} contains no valid keys")
    return raw, best_key


def derive_tenant_mask_key(tenant_id: str, *, mask_keys_env: str | None = None) -> bytes:
    """Derive 32-byte k_tenant = HKDF-SHA256(ikm=active_key, salt=None, info=tenant_id)."""
    cache_tag, ikm = _resolve_active_master_key(mask_keys_env)
    cache_key = (cache_tag, tenant_id)
    with _derived_key_cache_lock:
        cached = _derived_key_cache.get(cache_key)
        if cached is not None:
            return cached

    # RFC 5869 §2.2 & §2.3 with SHA-256 (HashLen = 32, default salt = 32 zero bytes, L = 32).
    prk = hmac.new(b"\x00" * 32, ikm, hashlib.sha256).digest()
    k_tenant = hmac.new(prk, tenant_id.encode("utf-8") + b"\x01", hashlib.sha256).digest()

    with _derived_key_cache_lock:
        if len(_derived_key_cache) >= 512:
            _derived_key_cache.clear()
        _derived_key_cache[cache_key] = k_tenant
    return k_tenant


def apply_mask_rule(
    value: str | None,
    rule: MaskRuleLiteral,
    *,
    tenant_id: str = "default",
    mask_keys_env: str | None = None,
) -> str | None:
    """Apply deterministic Step 13 §13.7 MaskRule matching shared golden vectors 100%."""
    if value is None:
        return None
    if rule == "NONE":
        return value
    if rule == "REDACT":
        return "[REDACTED]"
    if value == "":
        return ""

    if rule == "HMAC_HASH":
        k_tenant = derive_tenant_mask_key(tenant_id, mask_keys_env=mask_keys_env)
        return hmac.new(k_tenant, value.encode("utf-8"), hashlib.sha256).hexdigest()[:16]

    s = _normalize_halfwidth(value).strip()
    if not s:
        return "****"

    if rule == "MASK_PHONE":
        prefix = ""
        rest = s
        if s.startswith("+86-"):
            prefix = "+86-"
            rest = s[4:]
        elif s.startswith("+86 "):
            prefix = "+86 "
            rest = s[4:]
        elif s.startswith("+86"):
            prefix = "+86"
            rest = s[3:]
        digits = _PHONE_STRIP_RE.sub("", rest)
        if digits.isdigit() and len(digits) >= 7:
            return f"{prefix}{digits[:3]}{'*' * (len(digits) - 7)}{digits[-4:]}"
        return "****"

    if rule == "MASK_ID_CARD":
        if len(s) >= 11 and s[:-1].isdigit() and (s[-1].isdigit() or s[-1] in {"X", "x"}):
            return f"{s[:6]}{'*' * (len(s) - 10)}{s[-4:]}"
        return "****"

    if rule == "MASK_EMAIL":
        at_idx = s.find("@")
        if 0 < at_idx < len(s) - 1:
            local = s[:at_idx]
            domain = s[at_idx + 1 :]
            if "." in domain and not domain.startswith(".") and not domain.endswith("."):
                masked_local = local[0] + ("*" * max(len(local) - 1, 1))
                return f"{masked_local}@{domain}"
        return "****"

    if rule == "MASK_BANK_CARD":
        digits = _CARD_STRIP_RE.sub("", s)
        if digits.isdigit() and len(digits) > 4:
            return f"{'*' * (len(digits) - 4)}{digits[-4:]}"
        return "****"

    if rule == "MASK_NAME":
        n = len(s)
        if n == 1:
            return "*"
        if n >= 3 and s[:2] in COMPOUND_SURNAMES:
            return f"{s[:2]}{'*' * (n - 2)}"
        return f"{s[0]}{'*' * (n - 1)}"

    if rule == "MASK_ADDRESS":
        m = _MASK_ADDR_PREFIX_RE.match(s)
        admin_prefix = m.group(1) if m else ""
        if admin_prefix:
            return f"{admin_prefix}****"
        return "****"

    return value


class OfflineSpacyNlpEngine(SpacyNlpEngine):
    """SpaCy NLP engine strictly forbidding runtime network downloads (Decision D2)."""

    def __init__(self) -> None:
        super().__init__(
            models=[
                {"lang_code": "zh", "model_name": "zh_core_web_sm"},
                {"lang_code": "en", "model_name": "en"},
            ]
        )
        self.using_fallback_blank = False

    def load(self) -> None:
        try:
            zh_nlp = spacy.load("zh_core_web_sm")
            self.using_fallback_blank = False
        except (OSError, ImportError):
            logger.info("zh_core_web_sm wheel not present; using spacy.blank('zh') fallback")
            zh_nlp = spacy.blank("zh")
            self.using_fallback_blank = True

        self.nlp = {  # type: ignore[assignment]
            "zh": zh_nlp,
            "en": spacy.blank("en"),
        }


def resolve_overlapping_spans(
    raw_results: Sequence[RecognizerResult],
    *,
    score_threshold: float,
) -> list[PiiSpanModel]:
    """Deduplicate and greedily resolve overlapping spans by score, length, and priority."""
    best_exact: dict[tuple[int, int, PiiCategory], float] = {}
    for res in raw_results:
        category = ENTITY_TO_CATEGORY.get(res.entity_type, "NONE")
        if category == "NONE":
            continue
        score = float(res.score)
        if score < score_threshold or res.start >= res.end or res.start < 0:
            continue
        key = (int(res.start), int(res.end), category)
        if score > best_exact.get(key, -1.0):
            best_exact[key] = min(score, 1.0)

    candidates = [
        PiiSpanModel(category=cat, start=start, end=end, score=round(sc, 4))
        for (start, end, cat), sc in best_exact.items()
    ]
    candidates.sort(
        key=lambda sp: (
            -round(sp.score, 6),
            -(sp.end - sp.start),
            -CATEGORY_PRIORITY.get(sp.category, 0),
            sp.start,
        )
    )

    accepted: list[PiiSpanModel] = []
    for cand in candidates:
        if any(cand.start < acc.end and cand.end > acc.start for acc in accepted):
            continue
        accepted.append(cand)

    accepted.sort(key=lambda sp: (sp.start, sp.end))
    return accepted


class PiiEngine:
    """Thread-safe Presidio AnalyzerEngine + AnonymizerEngine wrapper for AxGuard."""

    _instance: PiiEngine | None = None
    _instance_lock = threading.Lock()

    def __init__(self, nlp_engine: OfflineSpacyNlpEngine | None = None) -> None:
        self._analyze_lock = threading.Lock()
        self.nlp_engine = nlp_engine or OfflineSpacyNlpEngine()
        if not self.nlp_engine.is_loaded():
            self.nlp_engine.load()

        registry = RecognizerRegistry(supported_languages=["zh", "en"])
        for recognizer in build_custom_recognizers(languages=("zh", "en")):
            registry.add_recognizer(recognizer)

        self.analyzer = AnalyzerEngine(
            registry=registry,
            nlp_engine=self.nlp_engine,
            supported_languages=["zh", "en"],
        )
        self.anonymizer = AnonymizerEngine()  # type: ignore[no-untyped-call]

    @classmethod
    def get_instance(cls) -> PiiEngine:
        """Return lazy process-wide singleton PiiEngine."""
        if cls._instance is None:
            with cls._instance_lock:
                if cls._instance is None:
                    cls._instance = cls()
        return cls._instance

    @property
    def using_fallback_blank(self) -> bool:
        """Return True when running with spacy.blank('zh') offline fallback."""
        return self.nlp_engine.using_fallback_blank

    def analyze_text(
        self,
        text: str,
        *,
        language: str = "zh",
        score_threshold: float = 0.6,
    ) -> list[PiiSpanModel]:
        """Analyze a single string and return sorted non-overlapping Unicode codepoint spans."""
        if not text or not text.strip():
            return []

        norm_lang = "en" if language.lower().startswith("en") else "zh"
        # Pure ASCII text cannot contain CJK NER tokens; use lightweight 'en' blank pipeline.
        effective_lang = "en" if text.isascii() else norm_lang

        with self._analyze_lock:
            raw = self.analyzer.analyze(
                text=text,
                language=effective_lang,
                score_threshold=score_threshold,
            )
        return resolve_overlapping_spans(raw, score_threshold=score_threshold)

    def analyze_texts(self, req: PiiAnalyzeRequest) -> PiiAnalyzeResponse:
        """Analyze a batch of texts per Step 13 §13.8.1."""
        results = [
            self.analyze_text(
                txt,
                language=req.language,
                score_threshold=req.score_threshold,
            )
            for txt in req.texts
        ]
        return PiiAnalyzeResponse(results=results)

    def anonymize_text(
        self,
        text: str,
        *,
        rules: dict[PiiCategory, MaskRuleLiteral] | None = None,
        language: str = "zh",
        score_threshold: float = 0.6,
        tenant_id: str = "default",
        mask_keys_env: str | None = None,
    ) -> str:
        """Detect PII spans in `text` and replace them via Presidio AnonymizerEngine."""
        if not text:
            return text

        spans = self.analyze_text(text, language=language, score_threshold=score_threshold)
        if not spans:
            return text

        effective_rules: dict[PiiCategory, MaskRuleLiteral] = {
            **DEFAULT_MASK_RULES,
            **(rules or {}),
        }

        anon_results: list[AnonRecognizerResult] = []
        operators: dict[str, OperatorConfig] = {}

        for sp in spans:
            rule = effective_rules.get(sp.category, "NONE")
            if rule == "NONE":
                continue
            anon_results.append(
                AnonRecognizerResult(
                    entity_type=sp.category,
                    start=sp.start,
                    end=sp.end,
                    score=sp.score,
                )
            )
            if sp.category not in operators:

                def _make_op(
                    bound_rule: MaskRuleLiteral,
                ) -> OperatorConfig:
                    return OperatorConfig(
                        "custom",
                        {
                            "lambda": lambda raw_val: (
                                apply_mask_rule(
                                    raw_val,
                                    bound_rule,
                                    tenant_id=tenant_id,
                                    mask_keys_env=mask_keys_env,
                                )
                                or ""
                            )
                        },
                    )

                operators[sp.category] = _make_op(rule)

        if not anon_results:
            return text

        engine_res = self.anonymizer.anonymize(
            text=text,
            analyzer_results=anon_results,
            operators=operators,
        )
        return str(engine_res.text)

    def anonymize_texts(
        self,
        req: PiiAnonymizeRequest,
        tenant_id: str | None = None,
    ) -> PiiAnonymizeResponse:
        """Anonymize a batch of texts using tenant-scoped masking rules."""
        eff_tenant = tenant_id if tenant_id is not None else req.tenant_id
        masked = [
            self.anonymize_text(
                txt,
                rules=req.rules,
                language=req.language,
                score_threshold=req.score_threshold,
                tenant_id=eff_tenant,
            )
            for txt in req.texts
        ]
        return PiiAnonymizeResponse(texts=masked)


def get_pii_engine() -> PiiEngine:
    """Return the shared thread-safe PiiEngine singleton."""
    return PiiEngine.get_instance()


def analyze_texts(req: PiiAnalyzeRequest) -> PiiAnalyzeResponse:
    """Module-level helper delegating POST /pii/analyze to singleton PiiEngine."""
    return get_pii_engine().analyze_texts(req)


def anonymize_texts(
    req: PiiAnonymizeRequest,
    tenant_id: str = "default",
) -> PiiAnonymizeResponse:
    """Module-level helper delegating POST /pii/anonymize to singleton PiiEngine."""
    return get_pii_engine().anonymize_texts(req, tenant_id=tenant_id)


__all__ = [
    "CATEGORY_PRIORITY",
    "DEFAULT_MASK_RULES",
    "MASK_KEYS_ENV_VAR",
    "OfflineSpacyNlpEngine",
    "PiiEngine",
    "analyze_texts",
    "anonymize_texts",
    "apply_mask_rule",
    "derive_tenant_mask_key",
    "get_pii_engine",
    "resolve_overlapping_spans",
]
