#!/usr/bin/env python3
"""Build a macOS app bundle and register its telinha:// URL handler."""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if sys.platform != "darwin":
        parser.error("Execute este script no macOS.")
    command = ["cargo", "build", "--locked", "-p", "telinha"]
    if args.release:
        command.append("--release")
    subprocess.run(command, cwd=ROOT, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    bundle = ROOT / "dist" / "telinha.app"
    contents = bundle / "Contents"
    (contents / "MacOS").mkdir(parents=True, exist_ok=True)
    (contents / "Resources").mkdir(exist_ok=True)
    shutil.copy2(ROOT / "app" / "assets" / "telinha.icns", contents / "Resources" / "telinha.icns")
    shutil.copy2(target / ("release" if args.release else "debug") / "telinha",
                 contents / "MacOS" / "telinha")
    camera_helper = contents / "MacOS" / "telinha-camera"
    subprocess.run(["xcrun", "swiftc", "-O", "-swift-version", "5",
                    "-target", f"{os.uname().machine}-apple-macosx14.0",
                    str(ROOT / "app" / "native" / "CameraCapture.swift"),
                    "-o", str(camera_helper)], check=True)
    subprocess.run(["codesign", "--force", "--sign", "-", "--identifier",
                    "tech.telinha.camera", str(camera_helper)], check=True)
    with (contents / "Info.plist").open("wb") as output:
        plistlib.dump({
            "CFBundleName": "telinha", "CFBundleDisplayName": "telinha",
            "CFBundleExecutable": "telinha", "CFBundleIdentifier": "tech.telinha.app",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
            "CFBundleVersion": version, "LSMinimumSystemVersion": "14.0",
            "CFBundleIconFile": "telinha.icns",
            "NSHighResolutionCapable": True,
            "NSCameraUsageDescription": "A telinha usa a webcam que você escolher para mostrar seu vídeo aos amigos da sessão.",
            "NSCameraUseContinuityCameraDeviceType": True,
            "NSScreenCaptureUsageDescription": "A telinha compartilha a janela ou tela que você escolher com seus amigos.",
            "CFBundleURLTypes": [{"CFBundleURLName": "Sessão da telinha",
                                  "CFBundleURLSchemes": ["telinha"],
                                  "CFBundleTypeRole": "Viewer"}],
        }, output)
    subprocess.run(["codesign", "--force", "--sign", "-", "--identifier",
                    "tech.telinha.app", str(bundle)], check=True)
    register = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    subprocess.run([register, "-f", str(bundle)], check=True)
    print(bundle)
    print("Pacote local de desenvolvimento, com assinatura ad hoc; não notarizado.")


if __name__ == "__main__":
    main()
