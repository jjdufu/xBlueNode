from __future__ import annotations

import shutil
import subprocess
import tempfile
from pathlib import Path

from security import SidecarError


def has_soffice() -> bool:
    """Return True if LibreOffice headless binary is available on PATH."""
    return shutil.which("soffice") is not None


def to_ooxml(data: bytes, src_suffix: str, target_ext: str) -> bytes:
    """Convert legacy Office document (.xls/.ppt/.doc) to OOXML via headless LibreOffice."""
    soffice = shutil.which("soffice")
    if not soffice:
        raise SidecarError(
            status_code=422,
            error_code="UNSUPPORTED_EARLY_OFFICE_FORMAT",
            message=(
                f"legacy Office format {src_suffix!r} requires LibreOffice (soffice) "
                "which is not installed in this image"
            ),
        )

    clean_ext = target_ext.lstrip(".")
    with tempfile.TemporaryDirectory() as tmp_str:
        tmp_dir = Path(tmp_str)
        src_file = (
            tmp_dir / f"input{src_suffix if src_suffix.startswith('.') else '.' + src_suffix}"
        )
        src_file.write_bytes(data)
        out_dir = tmp_dir / "out"
        out_dir.mkdir(parents=True, exist_ok=True)
        profile_uri = (tmp_dir / "lo_profile").as_uri()

        proc = subprocess.run(
            [
                soffice,
                f"-env:UserInstallation={profile_uri}",
                "--headless",
                "--convert-to",
                clean_ext,
                "--outdir",
                str(out_dir),
                str(src_file),
            ],
            capture_output=True,
            timeout=120,
            check=False,
        )
        produced = out_dir / f"input.{clean_ext}"
        if proc.returncode != 0 or not produced.is_file():
            stderr = proc.stderr.decode("utf-8", errors="replace").strip()
            raise SidecarError(
                status_code=422,
                error_code="PARSE_FAILED",
                message=f"LibreOffice conversion failed: {stderr or 'no output produced'}",
            )
        return produced.read_bytes()


def recalculate_xlsx(data: bytes) -> bytes | None:
    """Recalculate uncached Excel formulas via LibreOffice if available; None otherwise."""
    if not has_soffice():
        return None
    try:
        return to_ooxml(data, ".xlsx", "xlsx")
    except SidecarError:
        return None
