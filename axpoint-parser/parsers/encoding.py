from __future__ import annotations

from charset_normalizer import from_bytes


def detect_and_decode(data: bytes) -> tuple[str, str, bool]:
    """Decode bytes into text, returning (decoded_text, encoding_used, used_fallback).

    Tries strict UTF-8 / UTF-8-SIG first, then GB18030 / charset-normalizer / Big5 / Latin-1.
    """
    if not data:
        return "", "utf-8", False

    if data.startswith(b"\xef\xbb\xbf"):
        try:
            return data.decode("utf-8-sig"), "utf-8-sig", False
        except UnicodeDecodeError:
            pass

    try:
        return data.decode("utf-8"), "utf-8", False
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
