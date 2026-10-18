import hashlib
import json
import os
import pathlib
import shutil
import tarfile

APP_NAME = "codex-proxy-rs"

def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

version = os.environ["VERSION"]
platforms = json.loads(pathlib.Path("release-platforms.json").read_text())["platforms"]
frontend_dir = pathlib.Path("dist/frontend")
binary_artifacts_dir = pathlib.Path("dist/binaries")
release_dir = pathlib.Path("dist/release")
package_dir = pathlib.Path("package")

shutil.rmtree(release_dir, ignore_errors=True)
release_dir.mkdir(parents=True)
config_asset = release_dir / "config.example.yaml"
compose_asset = release_dir / "compose.yaml"
shutil.copy2("deploy/config.example.yaml", config_asset)
compose = pathlib.Path("deploy/compose.yaml").read_text()
default_image = "${CPR_IMAGE:-ghcr.io/heykode/newcpr:latest}"
if compose.count(default_image) != 1:
    raise SystemExit("Expected exactly one default application image")
# Preserve Compose interpolation, anchors and comments.
pinned_image = "${CPR_IMAGE:-ghcr.io/heykode/newcpr:" + version.replace("+", "-") + "}"
compose_asset.write_text(compose.replace(default_image, pinned_image))
archives = [config_asset, compose_asset]

for platform in platforms:
    platform_name = f"{platform['os']}-{platform['arch']}"
    artifact_name = f"{APP_NAME}-{platform_name}"
    binary_src = binary_artifacts_dir / artifact_name / platform["binary"]
    # download-artifact flattens a single match even when a pattern is used.
    if not binary_src.is_file() and len(platforms) == 1:
        binary_src = binary_artifacts_dir / platform["binary"]
    if not binary_src.is_file():
        raise SystemExit(f"Missing binary artifact: {binary_src}")

    shutil.rmtree(package_dir, ignore_errors=True)
    package_root = package_dir / APP_NAME
    web_dist = package_root / "web" / "dist"
    web_dist.mkdir(parents=True)

    binary_dest = package_root / platform["binary"]
    shutil.copy2(binary_src, binary_dest)
    binary_dest.chmod(0o755)
    shutil.copytree(frontend_dir, web_dist, dirs_exist_ok=True)
    (package_root / "deploy").mkdir()
    shutil.copy2(config_asset, package_root / "deploy" / config_asset.name)

    archive_name = f"{APP_NAME}_{version}_{platform['os']}_{platform['arch']}.{platform['archive']}"
    archive_path = release_dir / archive_name
    if platform["archive"] == "tar.gz":
        with tarfile.open(archive_path, "w:gz") as archive:
            archive.add(package_root, arcname=APP_NAME)
    else:
        raise SystemExit(f"Unsupported archive format: {platform['archive']}")
    archives.append(archive_path)

checksum_path = release_dir / "checksums.txt"
with checksum_path.open("w", encoding="utf-8") as file:
    for archive in archives:
        file.write(f"{sha256(archive)}  {archive.name}\n")
