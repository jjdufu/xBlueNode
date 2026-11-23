from __future__ import annotations

from charset_normalizer import from_bytes


def _looks_like_cjk_text(text: str) -> bool:
    non_ascii = [ch for ch in text if ord(ch) > 127]
    if not non_ascii:
        return False
    cjk_count = sum(
        1
        for ch in non_ascii
        if ("\u4e00" <= ch <= "\u9fff")
        or ("\u3000" <= ch <= "\u303f")
        or ("\uff00" <= ch <= "\uffef")
    )
    return (cjk_count / len(non_ascii)) >= 0.6


def detect_and_decode(data: bytes) -> tuple[str, str, bool]:
    """Decode bytes into text, returning (decoded_text, encoding_used, used_fallback).

    Tries strict UTF-8 / UTF-8-SIG / UTF-16 BOM first, then strict GBK/GB18030 CJK validation,
    charset-normalizer, Big5, and Latin-1.
    """
    if not data:
        return "", "utf-8", False

    if data.startswith(b"\xef\xbb\xbf"):
        try:
            return data.decode("utf-8-sig"), "utf-8-sig", False
        except UnicodeDecodeError:
            pass

    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        try:
            return data.decode("utf-16"), "utf-16", True
        except UnicodeDecodeError:
            pass

    try:
        return data.decode("utf-8"), "utf-8", False
    except UnicodeDecodeError:
        pass

    # Prefer strict GBK -> GB18030 when high-bit bytes form valid CJK text
    try:
        gbk_sample = data[:262_144].decode("gbk")
        if _looks_like_cjk_text(gbk_sample):
            return data.decode("gb18030"), "gb18030", True
    except UnicodeDecodeError:
        pass

    # Probe charset-normalizer on first 256 KB
    sample = data[:262_144]
    matches = from_bytes(sample)
    best = matches.best()
    if best is not None and best.encoding:
        enc = best.encoding.lower()
        # Upgrade gb2312 / gbk family to full gb18030 superset
        if enc in {"gb2312", "gbk", "gb18030", "cp936"}:
            try:
                return data.decode("gb18030"), "gb18030", True
            except UnicodeDecodeError:
                pass
        else:
            try:
                return data.decode(best.encoding), best.encoding.lower(), True
            except (LookupError, UnicodeDecodeError):
                pass

    for fallback_enc in ("utf-8-sig", "gb18030", "big5", "latin-1"):
        try:
            return data.decode(fallback_enc), fallback_enc, True
        except UnicodeDecodeError:
            continue

    return data.decode("utf-8", errors="replace"), "utf-8-replace", True
