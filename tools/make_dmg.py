#!/usr/bin/env python3
"""把 release 二进制打成 macOS .app，再压成 DMG。

    python3 tools/make_dmg.py \
        --binary target/aarch64-apple-darwin/release/tickerstrip \
        --arch arm64 \
        --version 0.1.0 \
        --out dist/tickerstrip-0.1.0-arm64.dmg

DMG 里放一个 .app + 指向 /Applications 的软链，用户拖进去即可安装。
签名用 ad-hoc（`codesign -s -`）：本机自用足够；要对外分发想避免 Gatekeeper
提示，需要 Developer ID 证书 + 公证（见 README）。
"""

import argparse
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

APP_NAME = "tickerstrip"
DISPLAY_NAME = "摸鱼盯盘"
BUNDLE_ID = "com.diaoff.tickerstrip"
MIN_MACOS = "11.0"


def run(cmd: list[str], **kw) -> None:
    subprocess.run(cmd, check=True, **kw)


def build_app(binary: Path, arch: str, version: str, root: Path, icns: Path) -> Path:
    app = root / f"{APP_NAME}.app"
    macos = app / "Contents" / "MacOS"
    res = app / "Contents" / "Resources"
    macos.mkdir(parents=True)
    res.mkdir(parents=True)

    shutil.copy2(binary, macos / APP_NAME)
    shutil.copy2(icns, res / f"{APP_NAME}.icns")

    info = {
        "CFBundleName": APP_NAME,
        "CFBundleDisplayName": DISPLAY_NAME,
        "CFBundleExecutable": APP_NAME,
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleIconFile": f"{APP_NAME}.icns",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": MIN_MACOS,
        # 纯菜单栏程序：不出现在 Dock
        "LSUIElement": True,
        "NSHumanReadableCopyright": "MIT License",
        "BuildArchitecture": arch,
    }
    (app / "Contents" / "Info.plist").write_bytes(plistlib.dumps(info))

    # ad-hoc 签名（先签可执行文件再签整个 bundle）
    run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(macos / APP_NAME)])
    run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(app)])
    return app


def build_dmg(app: Path, version: str, arch: str, out: Path, vol_name: str) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    out.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp) / "stage"
        stage.mkdir()
        run(["cp", "-R", str(app), str(stage / app.name)])
        (stage / "Applications").symlink_to("/Applications")

        run(
            [
                "hdiutil", "create",
                "-volname", vol_name,
                "-srcfolder", str(stage),
                "-fs", "HFS+",
                "-format", "UDZO",
                "-imagekey", "zlib-level=9",
                "-quiet",
                str(out),
            ]
        )
    size = out.stat().st_size
    print(f"  {out}  ({size / 1024 / 1024:.1f} MB)")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True, type=Path)
    ap.add_argument("--arch", required=True, choices=["arm64", "x86_64"])
    ap.add_argument("--version", required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--icns", type=Path, default=Path("assets/tickerstrip.icns"))
    args = ap.parse_args()

    if not args.binary.exists():
        sys.exit(f"找不到二进制：{args.binary}")
    if not args.icns.exists():
        sys.exit(f"找不到图标：{args.icns}（先跑 make icon）")

    with tempfile.TemporaryDirectory() as tmp:
        app = build_app(args.binary, args.arch, args.version, Path(tmp), args.icns)
        vol = f"{DISPLAY_NAME} {args.version} {args.arch}"
        build_dmg(app, args.version, args.arch, args.out, vol)


if __name__ == "__main__":
    main()
