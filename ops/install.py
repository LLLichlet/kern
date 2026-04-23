from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from ops.common import (  # type: ignore
        HOST_TOOL_BINARIES,
        OpsError,
        detect_host_target,
        ensure,
        file_size,
        info,
        make_temp_dir,
        read_json,
        sha256_directory,
        sha256_file,
    )
else:
    from .common import (
        HOST_TOOL_BINARIES,
        OpsError,
        detect_host_target,
        ensure,
        file_size,
        info,
        make_temp_dir,
        read_json,
        sha256_directory,
        sha256_file,
    )


DEFAULT_GITHUB_REPO = "softfault/kern"
DEFAULT_VERSION = "v0.7.0"


@dataclass(frozen=True)
class InstallReleaseArgs:
    version: str | None
    target: str | None
    archive: str | None
    dest: str | None
    github_repo: str
    no_path: bool


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m ops install",
        description="Install a Kern SDK release archive.",
    )
    parser.add_argument("--version", help="release tag; defaults to latest GitHub release")
    parser.add_argument("--target", help="host target label; defaults to the current host target")
    parser.add_argument("--archive", help="install from a local SDK archive instead of GitHub")
    parser.add_argument("--dest", help="installation directory; defaults to ~/.kern or %%USERPROFILE%%\\.kern")
    parser.add_argument("--github-repo", default=DEFAULT_GITHUB_REPO, help="GitHub repository for release downloads")
    parser.add_argument("--no-path", action="store_true", help="do not mutate user PATH configuration")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    ns = parser.parse_args(argv)
    try:
        return install_release(
            InstallReleaseArgs(
                version=ns.version,
                target=ns.target,
                archive=ns.archive,
                dest=ns.dest,
                github_repo=ns.github_repo,
                no_path=ns.no_path,
            )
        )
    except OpsError as err:
        parser.exit(1, f"Error: {err}\n")


def install_release(args: InstallReleaseArgs) -> int:
    host = detect_host_target()
    target = args.target or host.archive_target
    ensure(
        target == host.archive_target,
        f"target `{target}` does not match the current host `{host.archive_target}`",
    )

    install_root = Path(args.dest).expanduser() if args.dest else _default_install_root(host.is_windows)
    install_root.mkdir(parents=True, exist_ok=True)
    install_bin = install_root / "bin"

    archive_path: Path
    version = args.version
    temp_root = make_temp_dir("kern-install-")
    try:
        if args.archive:
            archive_path = Path(args.archive).expanduser().resolve()
            ensure(archive_path.is_file(), f"archive `{archive_path}` does not exist")
            if version is None:
                version = _infer_version_from_archive_name(archive_path.name, target)
        else:
            version = version or fetch_latest_version(args.github_repo) or DEFAULT_VERSION
            archive_name = f"kern-{version}-{target}.{host.archive_extension}"
            archive_path = temp_root / archive_name
            download_release_archive(args.github_repo, version, archive_name, archive_path)

        ensure(version is not None, "failed to resolve release version")
        extract_root = temp_root / "extract"
        extract_root.mkdir(parents=True, exist_ok=True)
        sdk_root = extract_archive(archive_path, extract_root, host.is_windows)
        validate_sdk_root(sdk_root, target)

        copy_sdk_contents(sdk_root, install_root)

        info("=> Verifying installed tools...")
        for binary in HOST_TOOL_BINARIES:
            verify_binary(install_bin / f"{binary}{host.exe_suffix}", install_root, target)

        if not args.no_path:
            configure_path(install_bin, host.is_windows)

        info(f"Kern {version} toolchain installed successfully!")
        return 0
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)


def fetch_latest_version(github_repo: str) -> str | None:
    url = f"https://api.github.com/repos/{github_repo}/releases/latest"
    try:
        with urllib.request.urlopen(url) as response:
            payload = response.read().decode("utf-8")
    except urllib.error.URLError:
        return None
    marker = '"tag_name":'
    index = payload.find(marker)
    if index == -1:
        return None
    tail = payload[index + len(marker):]
    first_quote = tail.find('"')
    second_quote = tail.find('"', first_quote + 1)
    if first_quote == -1 or second_quote == -1:
        return None
    return tail[first_quote + 1:second_quote]


def download_release_archive(github_repo: str, version: str, archive_name: str, dest: Path) -> None:
    url = f"https://github.com/{github_repo}/releases/download/{version}/{archive_name}"
    info(f"=> Downloading Kern {version}...")
    try:
        with urllib.request.urlopen(url) as response, dest.open("wb") as output:
            shutil.copyfileobj(response, output)
    except urllib.error.URLError as err:
        raise OpsError(f"download failed for `{url}`: {err}") from err


def extract_archive(archive_path: Path, extract_root: Path, is_windows: bool) -> Path:
    info("=> Extracting toolchain...")
    if is_windows:
        with zipfile.ZipFile(archive_path) as archive:
            archive.extractall(extract_root)
    else:
        with tarfile.open(archive_path, "r:*") as archive:
            archive.extractall(extract_root)

    roots = [path for path in extract_root.iterdir() if path.is_dir()]
    ensure(len(roots) == 1, f"expected exactly one SDK root in `{archive_path}`")
    return roots[0]


def validate_sdk_root(sdk_root: Path, expected_target: str) -> None:
    manifest_path = sdk_root / "manifest" / "sdk.json"
    ensure(manifest_path.is_file(), f"SDK manifest `{manifest_path}` is missing")
    manifest = read_json(manifest_path)
    ensure(manifest.get("host_target") == expected_target, f"SDK host target mismatch in `{manifest_path}`")
    for binary in HOST_TOOL_BINARIES:
        ensure((sdk_root / "bin" / binary).exists() or (sdk_root / "bin" / f"{binary}.exe").exists(), f"SDK binary `{binary}` is missing from `{sdk_root}`")
    ensure((sdk_root / "lib" / "kern" / "craft" / "init.rn").is_file(), "SDK craft script modules are missing")
    ensure((sdk_root / "toolchain" / "host" / "bin").is_dir(), "SDK toolchain layout is incomplete")
    _validate_manifest_toolchain(sdk_root, manifest)


def _validate_manifest_toolchain(sdk_root: Path, manifest: dict[str, object]) -> None:
    toolchain = manifest.get("toolchain")
    ensure(isinstance(toolchain, dict), "SDK manifest is missing the `toolchain` section")
    bundled = bool(toolchain.get("bundled"))
    components = toolchain.get("components")
    ensure(isinstance(components, dict), "SDK manifest toolchain components are invalid")

    if not bundled:
        return

    required = ["clang", "lld"]
    host_target = str(manifest.get("host_target", ""))
    if host_target.endswith("windows-msvc"):
        required.extend(["llvm_lib"])

    for component in required:
        entry = components.get(component)
        ensure(isinstance(entry, dict), f"SDK manifest is missing bundled component `{component}`")

    for component, entry in components.items():
        ensure(isinstance(entry, dict), f"SDK manifest component `{component}` is invalid")
        _validate_component_record(sdk_root, component, entry)

    for component in required:
        entry = components[component]
        assert isinstance(entry, dict)
        _verify_bundled_toolchain_component_starts(sdk_root, component, entry, host_target)


def _validate_component_record(sdk_root: Path, component: str, entry: dict[str, object]) -> None:
    relative_path = entry.get("path")
    ensure(
        isinstance(relative_path, str) and relative_path,
        f"SDK manifest component `{component}` has no path",
    )
    kind = entry.get("kind", "file")
    ensure(isinstance(kind, str), f"SDK manifest component `{component}` has an invalid kind")
    target = sdk_root / relative_path

    if kind == "directory":
        ensure(target.is_dir(), f"SDK bundled component `{component}` is missing at `{target}`")
        expected_sha = entry.get("sha256")
        if isinstance(expected_sha, str) and expected_sha:
            actual_sha = sha256_directory(target)
            ensure(
                actual_sha == expected_sha,
                f"SDK bundled directory `{component}` checksum mismatch at `{target}`",
            )
        return

    ensure(target.is_file(), f"SDK bundled component `{component}` is missing at `{target}`")
    expected_size = entry.get("size")
    if isinstance(expected_size, int):
        ensure(
            file_size(target) == expected_size,
            f"SDK bundled component `{component}` size mismatch at `{target}`",
        )
    expected_sha = entry.get("sha256")
    if isinstance(expected_sha, str) and expected_sha:
        actual_sha = sha256_file(target)
        ensure(
            actual_sha == expected_sha,
            f"SDK bundled component `{component}` checksum mismatch at `{target}`",
        )


def _verify_bundled_toolchain_component_starts(
    sdk_root: Path,
    component: str,
    entry: dict[str, object],
    host_target: str,
) -> None:
    relative_path = entry.get("path")
    ensure(
        isinstance(relative_path, str) and relative_path,
        f"SDK manifest component `{component}` has no path",
    )
    target = sdk_root / relative_path
    ensure(target.is_file(), f"SDK bundled component `{component}` is missing at `{target}`")

    if component == "llvm_lib":
        temp_root = make_temp_dir("kern-llvm-lib-probe-")
        try:
            probe_output = temp_root / "empty.lib"
            completed = subprocess.run(
                [str(target), "/llvmlibempty", f"/out:{probe_output}"],
                check=False,
                text=True,
                capture_output=True,
            )
        finally:
            shutil.rmtree(temp_root, ignore_errors=True)
    else:
        completed = subprocess.run(
            [str(target), "--version"],
            check=False,
            text=True,
            capture_output=True,
        )

    if completed.returncode == 0:
        return

    output = (completed.stdout or "") + (completed.stderr or "")
    if host_target.endswith("linux-gnu"):
        output += (
            "\nThe bundled Linux runtime tool did not start. "
            "The SDK archive is missing a required shared-library dependency."
        )
    elif host_target.endswith("apple-darwin"):
        output += (
            "\nThe bundled macOS runtime tool did not start. "
            "The SDK archive likely has a broken dylib load command or missing bundled dylib."
        )
    else:
        output += (
            "\nThe bundled Windows runtime tool did not start. "
            "The SDK archive is missing a required runtime dependency."
        )
    raise OpsError(
        f"SDK bundled runtime component `{component}` failed to start at `{target}`:\n"
        f"{output.strip()}"
    )


def copy_sdk_contents(sdk_root: Path, install_root: Path) -> None:
    info(f"=> Installing SDK into {install_root}...")
    for child in sdk_root.iterdir():
        destination = install_root / child.name
        if destination.exists():
            if destination.is_dir():
                shutil.rmtree(destination)
            else:
                destination.unlink()
        if child.is_dir():
            shutil.copytree(child, destination)
        else:
            shutil.copy2(child, destination)


def verify_binary(binary_path: Path, install_root: Path, target: str) -> None:
    ensure(binary_path.is_file(), f"installed binary `{binary_path}` is missing")
    try:
        completed = subprocess.run(
            [str(binary_path), "--version"],
            check=False,
            text=True,
            capture_output=True,
        )
    except OSError as err:
        raise OpsError(f"failed to start `{binary_path}`: {err}") from err
    if completed.returncode == 0:
        output = completed.stdout.strip() or completed.stderr.strip()
        info(f"=> Verified {binary_path.name}: {output}")
        return

    message = completed.stdout + completed.stderr
    if target.endswith("linux-gnu"):
        message += (
            "\nThe host tool still failed after installation. "
            "This often means missing shared libraries or an older glibc baseline."
        )
    elif target.endswith("apple-darwin"):
        message += (
            "\nmacOS could not start the installed tool. "
            "Inspect local loader and security-policy behavior manually if needed."
        )
    else:
        message += (
            "\nOfficial Windows archives should be static-CRT. "
            "If startup still fails, inspect local security policy and archive provenance."
        )
    raise OpsError(f"failed to start `{binary_path}` after installation:\n{message.strip()}")


def configure_path(install_bin: Path, is_windows: bool) -> None:
    info("=> Configuring PATH...")
    if is_windows:
        _configure_windows_path(install_bin)
    else:
        _configure_unix_path(install_bin)


def _configure_unix_path(install_bin: Path) -> None:
    rc_file = _select_unix_rc_file()
    rc_file.touch(exist_ok=True)
    marker = str(install_bin)
    contents = rc_file.read_text(encoding="utf-8")
    if marker in contents:
        info(f"{install_bin} is already in your PATH.")
        return
    with rc_file.open("a", encoding="utf-8") as handle:
        handle.write("\n# Kern Programming Language\n")
        handle.write(f'export PATH="{install_bin}:$PATH"\n')
    info(f"Added {install_bin} to your PATH in {rc_file}.")


def _configure_windows_path(install_bin: Path) -> None:
    try:
        import winreg
    except ImportError as err:
        raise OpsError(f"failed to import Windows registry helpers: {err}") from err

    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Environment", 0, winreg.KEY_READ | winreg.KEY_SET_VALUE) as key:
        current, _ = winreg.QueryValueEx(key, "Path") if _value_exists(key, "Path") else ("", winreg.REG_EXPAND_SZ)
        if str(install_bin) in current:
            info(f"{install_bin} is already in your PATH.")
            return
        new_value = f"{current};{install_bin}" if current else str(install_bin)
        winreg.SetValueEx(key, "Path", 0, winreg.REG_EXPAND_SZ, new_value)
    info(f"Added {install_bin} to your user PATH.")


def _value_exists(key: object, name: str) -> bool:
    try:
        import winreg

        winreg.QueryValueEx(key, name)
        return True
    except OSError:
        return False


def _select_unix_rc_file() -> Path:
    shell_name = Path(os.environ.get("SHELL", "")).name
    home = Path.home()
    if shell_name == "zsh":
        return home / ".zshrc"
    if shell_name == "bash":
        return home / ".bashrc"
    return home / ".profile"


def _default_install_root(is_windows: bool) -> Path:
    if is_windows:
        return Path(os.environ["USERPROFILE"]) / ".kern"
    return Path.home() / ".kern"


def _infer_version_from_archive_name(name: str, target: str) -> str | None:
    prefix = "kern-"
    suffixes = (f"-{target}.tar.gz", f"-{target}.zip")
    for suffix in suffixes:
        if name.startswith(prefix) and name.endswith(suffix):
            return name[len(prefix):-len(suffix)]
    return None


if __name__ == "__main__":
    raise SystemExit(main())
