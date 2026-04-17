from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import tempfile
from hashlib import sha256
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKSPACE_CARGO_TOML = REPO_ROOT / "Cargo.toml"
OFFICIAL_LIBRARY_LAYERS = ("base", "rt", "sys", "std")
HOST_TOOL_BINARIES = ("kernc", "craft", "kern-lsp")


class OpsError(RuntimeError):
    """Operations entrypoint failure."""


@dataclass(frozen=True)
class HostTarget:
    archive_target: str
    cargo_target: str | None
    exe_suffix: str
    archive_extension: str
    is_windows: bool


@dataclass(frozen=True)
class BundledToolchain:
    source_label: str
    prefix: Path
    bindir: Path
    libdir: Path
    includedir: Path
    version: str
    tools: dict[str, Path]
    resource_dir: Path | None
    sysroot_dir: Path | None


@dataclass(frozen=True)
class ArtifactRecord:
    path: str
    kind: str
    sha256: str | None
    size: int | None


def repo_root() -> Path:
    return REPO_ROOT


def info(message: str) -> None:
    print(message)


def ensure(condition: bool, message: str) -> None:
    if not condition:
        raise OpsError(message)


def require_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise OpsError(f"required tool `{name}` was not found in PATH")


def run(cmd: Iterable[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    command = list(cmd)
    info(f"=> Running: {' '.join(command)}")
    subprocess.run(command, cwd=cwd or repo_root(), env=env, check=True)


def run_capture(
    cmd: Iterable[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    command = list(cmd)
    info(f"=> Running: {' '.join(command)}")
    return subprocess.run(
        command,
        cwd=cwd or repo_root(),
        env=env,
        check=False,
        text=True,
        capture_output=True,
    )


def run_capture_checked(
    cmd: Iterable[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    completed = run_capture(cmd, cwd=cwd, env=env)
    if completed.returncode != 0:
        output = (completed.stdout or "") + (completed.stderr or "")
        raise OpsError(
            f"command failed with exit code {completed.returncode}: {' '.join(cmd)}\n{output.strip()}"
        )
    return completed


def load_workspace_version() -> str:
    source = WORKSPACE_CARGO_TOML.read_text(encoding="utf-8")
    in_workspace_package = False
    for raw_line in source.splitlines():
        line = raw_line.strip()
        if line.startswith("[") and line.endswith("]"):
            in_workspace_package = line == "[workspace.package]"
            continue
        if in_workspace_package and line.startswith("version = "):
            value = line.split("=", 1)[1].strip().strip('"')
            ensure(bool(value), "workspace version is empty")
            return value
    raise OpsError(f"failed to resolve workspace version from {WORKSPACE_CARGO_TOML}")


def detect_host_target() -> HostTarget:
    system = platform.system()
    machine = platform.machine().lower()

    if machine in {"x86_64", "amd64"}:
        arch = "x86_64"
    elif machine in {"aarch64", "arm64"}:
        arch = "aarch64"
    else:
        raise OpsError(f"unsupported architecture: {machine}")

    if system == "Linux":
        archive_target = f"{arch}-linux-gnu"
        return HostTarget(archive_target, None, "", "tar.gz", False)
    if system == "Darwin":
        archive_target = f"{arch}-apple-darwin"
        return HostTarget(archive_target, None, "", "tar.gz", False)
    if system == "Windows":
        ensure(
            arch == "x86_64",
            "Windows packaging currently only supports x86_64-windows-msvc",
        )
        archive_target = "x86_64-windows-msvc"
        return HostTarget(
            archive_target,
            "x86_64-pc-windows-msvc",
            ".exe",
            "zip",
            True,
        )
    raise OpsError(f"unsupported operating system: {system}")


def sdk_manifest(
    version: str,
    archive_target: str,
    *,
    bundled_toolchain: BundledToolchain | None,
    bundled_component_records: dict[str, ArtifactRecord] | None = None,
) -> dict[str, object]:
    toolchain_notes = [
        "The SDK prefers the bundled host toolchain when present.",
        "Ambient LLVM/PATH lookup remains a source-build fallback, not the primary install path.",
    ]
    components: dict[str, object] = {
        "clang": None,
        "clangxx": None,
        "lld": None,
        "llvm_ar": None,
        "llvm_config": None,
    }
    strategy = "system-fallback"
    bundled = False
    source: dict[str, object] | None = None
    layout = {
        "root": "toolchain",
        "host_root": "toolchain/host",
        "bin_dir": "toolchain/host/bin",
        "lib_dir": "toolchain/host/lib",
        "include_dir": "toolchain/host/include",
        "sysroot_dir": "toolchain/host/sysroot",
    }

    if bundled_toolchain is not None:
        bundled = True
        strategy = "bundled-first"
        source = {
            "label": bundled_toolchain.source_label,
            "version": bundled_toolchain.version,
        }
        components = {}
        if bundled_component_records is not None:
            components = {
                name: {
                    "path": record.path,
                    "kind": record.kind,
                    "sha256": record.sha256,
                    "size": record.size,
                }
                for name, record in bundled_component_records.items()
            }
        else:
            components = {
                name: {
                    "path": _bundled_component_path(bundled_toolchain, path),
                }
                for name, path in bundled_toolchain.tools.items()
            }
            if bundled_toolchain.resource_dir is not None:
                components["clang_resource_dir"] = {
                    "path": bundled_resource_dir_path(bundled_toolchain),
                }
            components["lib_dir"] = {
                "path": _bundled_component_path(bundled_toolchain, bundled_toolchain.libdir),
                "kind": "directory",
            }
            components["include_dir"] = {
                "path": _bundled_component_path(bundled_toolchain, bundled_toolchain.includedir),
                "kind": "directory",
            }
        layout = toolchain_layout_paths(bundled_toolchain)
        toolchain_notes = [
            "The SDK bundles the host LLVM/Clang toolchain used for release validation.",
            "The bundled toolchain preserves a relocatable LLVM development prefix for source builds.",
            "Host OS SDK/libc components may still be required by the platform linker/runtime.",
        ]

    return {
        "schema_version": 1,
        "sdk_version": version,
        "host_target": archive_target,
        "layout_version": 1,
        "binaries": list(HOST_TOOL_BINARIES),
        "libraries": list(OFFICIAL_LIBRARY_LAYERS),
        "toolchain": {
            "layout": layout,
            "bundled": bundled,
            "strategy": strategy,
            "resolver_order": [
                "explicit-toolchain-root",
                "sdk-relative-toolchain",
                "environment-overrides",
                "system-path",
            ],
            "source": source,
            "components": components,
            "notes": toolchain_notes,
        },
    }


def canonical_toolchain_component_names(archive_target: str) -> dict[str, str]:
    exe_suffix = ".exe" if archive_target.endswith("windows-msvc") else ""
    return {
        "clang": f"clang{exe_suffix}",
        "clangxx": f"clang++{exe_suffix}",
        "lld": (
            "lld-link.exe"
            if archive_target.endswith("windows-msvc")
            else "ld64.lld"
            if archive_target.endswith("apple-darwin")
            else "ld.lld"
        ),
        "llvm_ar": f"llvm-ar{exe_suffix}",
        "llvm_config": f"llvm-config{exe_suffix}",
        "llvm_lib": "llvm-lib.exe",
    }


def toolchain_layout_paths(bundled_toolchain: BundledToolchain) -> dict[str, str]:
    return {
        "root": "toolchain",
        "host_root": "toolchain/host",
        "bin_dir": _bundled_component_path(bundled_toolchain, bundled_toolchain.bindir),
        "lib_dir": _bundled_component_path(bundled_toolchain, bundled_toolchain.libdir),
        "include_dir": _bundled_component_path(bundled_toolchain, bundled_toolchain.includedir),
        "sysroot_dir": "toolchain/host/sysroot",
    }


def _bundled_component_path(bundled_toolchain: BundledToolchain, path: Path) -> str:
    try:
        relative = path.relative_to(bundled_toolchain.prefix)
    except ValueError as err:
        raise OpsError(
            f"toolchain path `{path}` does not live under prefix `{bundled_toolchain.prefix}`"
        ) from err
    return f"toolchain/host/{relative.as_posix()}"


def bundled_resource_dir_path(bundled_toolchain: BundledToolchain) -> str:
    ensure(bundled_toolchain.resource_dir is not None, "bundled toolchain has no clang resource dir")
    return f"toolchain/host/lib/clang/{bundled_toolchain.resource_dir.name}"


def toolchain_manifest(
    version: str,
    archive_target: str,
    *,
    bundled_toolchain: BundledToolchain,
    bundled_component_records: dict[str, ArtifactRecord] | None = None,
) -> dict[str, object]:
    if bundled_component_records is not None:
        components = {
            name: {
                "path": record.path,
                "kind": record.kind,
                "sha256": record.sha256,
                "size": record.size,
            }
            for name, record in bundled_component_records.items()
        }
    else:
        components = {
            name: {
                "path": _bundled_component_path(bundled_toolchain, path),
            }
            for name, path in bundled_toolchain.tools.items()
        }
        if bundled_toolchain.resource_dir is not None:
            components["clang_resource_dir"] = {
                "path": bundled_resource_dir_path(bundled_toolchain),
            }
        components["lib_dir"] = {
            "path": _bundled_component_path(bundled_toolchain, bundled_toolchain.libdir),
            "kind": "directory",
        }
        components["include_dir"] = {
            "path": _bundled_component_path(bundled_toolchain, bundled_toolchain.includedir),
            "kind": "directory",
        }

    return {
        "schema_version": 1,
        "toolchain_version": version,
        "host_target": archive_target,
        "layout_version": 1,
        "provider": "bundled-host-llvm",
        "layout": toolchain_layout_paths(bundled_toolchain),
        "source": {
            "label": bundled_toolchain.source_label,
            "version": bundled_toolchain.version,
        },
        "components": components,
        "notes": [
            "This archive contains the controlled host LLVM/Clang toolchain used by Kern packaging.",
            "It is intended for CI, release engineering, and SDK assembly.",
            "The archive preserves a relocatable LLVM development prefix for source builds.",
            "Host OS SDK/libc components may still remain platform responsibilities.",
        ],
    }


def write_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def read_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_file(path: Path) -> str:
    digest = sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def sha256_directory(path: Path) -> str:
    ensure(path.is_dir(), f"directory `{path}` does not exist")
    digest = sha256()
    for child in sorted(p for p in path.rglob("*") if p.is_file()):
        relative = child.relative_to(path).as_posix().encode("utf-8")
        digest.update(relative)
        digest.update(b"\0")
        digest.update(sha256_file(child).encode("ascii"))
        digest.update(b"\0")
    return digest.hexdigest()


def file_size(path: Path) -> int:
    return path.stat().st_size


def make_temp_dir(prefix: str) -> Path:
    preferred_root = os.environ.get("KERN_OPS_TEMP_ROOT") or os.environ.get("RUNNER_TEMP")
    if preferred_root:
        root = Path(preferred_root).expanduser()
        root.mkdir(parents=True, exist_ok=True)
        return Path(tempfile.mkdtemp(prefix=prefix, dir=root))
    return Path(tempfile.mkdtemp(prefix=prefix))


def find_llvm_sys_prefix() -> tuple[str, Path] | None:
    matches = [
        (key, Path(value))
        for key, value in os.environ.items()
        if key.startswith("LLVM_SYS_") and key.endswith("_PREFIX") and value
    ]
    matches.sort(key=lambda item: item[0])
    for key, path in matches:
        if path.is_dir():
            return key, path.resolve()
    return None


def find_kern_toolchain_root() -> tuple[str, Path] | None:
    value = os.environ.get("KERN_TOOLCHAIN_ROOT")
    if not value:
        return None
    path = Path(value).expanduser()
    if not path.is_dir():
        return None
    return "KERN_TOOLCHAIN_ROOT", path.resolve()


def _first_line(text: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            return stripped
    return ""


def _tool_output(cmd: Iterable[str]) -> str:
    completed = run_capture_checked(cmd)
    output = (completed.stdout or "").strip() or (completed.stderr or "").strip()
    ensure(bool(output), f"command produced no output: {' '.join(cmd)}")
    return output


def _tool_version_line(path: Path) -> str:
    completed = run_capture([str(path), "--version"])
    if completed.returncode != 0:
        return "<unavailable>"
    return _first_line((completed.stdout or "") + (completed.stderr or "")) or "<unavailable>"


def _version_major(version: str) -> str:
    return version.split(".", 1)[0]


def _tool_candidate_names(name: str, major: str, is_windows: bool) -> list[str]:
    suffix = ".exe" if is_windows else ""
    versioned = f"{name}-{major}{suffix}" if major else None
    plain = f"{name}{suffix}"
    if versioned is None:
        return [plain]
    return [plain, versioned]


def _resolve_homebrew_tool(
    *,
    formula_names: Iterable[str],
    name: str,
    major: str,
    required: bool,
) -> Path | None:
    brew = shutil.which("brew")
    if brew is None:
        if required:
            raise OpsError("failed to locate `brew` while resolving a Homebrew-managed LLVM tool")
        return None

    for formula in formula_names:
        completed = run_capture([brew, "--prefix", formula])
        if completed.returncode != 0:
            continue
        prefix = (completed.stdout or "").strip()
        if not prefix:
            continue
        candidate = _resolve_llvm_tool(
            name=name,
            major=major,
            bindir=Path(prefix).resolve() / "bin",
            is_windows=False,
            required=False,
            allow_path_lookup=False,
        )
        if candidate is not None:
            return candidate

    if required:
        raise OpsError(
            f"failed to resolve Homebrew-managed LLVM tool `{name}` from formulas: {', '.join(formula_names)}"
        )
    return None


def _resolve_llvm_tool(
    *,
    name: str,
    major: str,
    bindir: Path,
    is_windows: bool,
    required: bool,
    allow_path_lookup: bool = True,
) -> Path | None:
    for candidate in _tool_candidate_names(name, major, is_windows):
        direct = bindir / candidate
        if direct.is_file():
            return direct

    if allow_path_lookup:
        for candidate in _tool_candidate_names(name, major, is_windows):
            resolved = shutil.which(candidate)
            if resolved is not None:
                return Path(resolved)

    if required:
        raise OpsError(f"failed to resolve LLVM tool `{name}` for the current packaging environment")
    return None


def resolve_bundled_toolchain(
    host: HostTarget,
    *,
    explicit_prefix: str | None = None,
) -> BundledToolchain:
    source_label: str
    prefix: Path

    if explicit_prefix:
        prefix = Path(explicit_prefix).expanduser().resolve()
        source_label = "explicit-toolchain-prefix"
    else:
        env_prefix = find_kern_toolchain_root() or find_llvm_sys_prefix()
        if env_prefix is not None:
            source_label, prefix = env_prefix
        else:
            llvm_config = shutil.which("llvm-config-21") or shutil.which("llvm-config")
            ensure(llvm_config is not None, "failed to locate `llvm-config`; cannot bundle host LLVM toolchain")
            prefix = Path(_tool_output([llvm_config, "--prefix"])).resolve()
            source_label = "llvm-config"

    ensure(prefix.is_dir(), f"LLVM toolchain prefix `{prefix}` does not exist")

    llvm_config = _resolve_llvm_tool(
        name="llvm-config",
        major="21",
        bindir=prefix / "bin",
        is_windows=host.is_windows,
        required=False,
        allow_path_lookup=False,
    )
    ensure(llvm_config is not None, f"failed to resolve `llvm-config` within LLVM prefix `{prefix}`")

    version = _tool_output([str(llvm_config), "--version"])
    major = _version_major(version)
    bindir = Path(_tool_output([str(llvm_config), "--bindir"])).resolve()
    libdir = Path(_tool_output([str(llvm_config), "--libdir"])).resolve()
    includedir = Path(_tool_output([str(llvm_config), "--includedir"])).resolve()
    ensure(bindir.is_dir(), f"LLVM bindir `{bindir}` does not exist")
    ensure(libdir.is_dir(), f"LLVM libdir `{libdir}` does not exist")
    ensure(includedir.is_dir(), f"LLVM includedir `{includedir}` does not exist")

    tools: dict[str, Path] = {
        "llvm_config": llvm_config,
        "clang": _resolve_llvm_tool(
            name="clang",
            major=major,
            bindir=bindir,
            is_windows=host.is_windows,
            required=True,
            allow_path_lookup=False,
        ),
        "clangxx": _resolve_llvm_tool(
            name="clang++",
            major=major,
            bindir=bindir,
            is_windows=host.is_windows,
            required=True,
            allow_path_lookup=False,
        ),
        "llvm_ar": _resolve_llvm_tool(
            name="llvm-ar",
            major=major,
            bindir=bindir,
            is_windows=host.is_windows,
            required=True,
            allow_path_lookup=False,
        ),
    }

    if host.is_windows:
        lld = _resolve_llvm_tool(
            name="lld-link",
            major=major,
            bindir=bindir,
            is_windows=True,
            required=True,
            allow_path_lookup=False,
        )
        llvm_lib = _resolve_llvm_tool(
            name="llvm-lib",
            major=major,
            bindir=bindir,
            is_windows=True,
            required=True,
            allow_path_lookup=False,
        )
        assert lld is not None
        assert llvm_lib is not None
        tools["lld"] = lld
        tools["llvm_lib"] = llvm_lib
    elif host.archive_target.endswith("apple-darwin"):
        lld = _resolve_llvm_tool(
            name="ld64.lld",
            major=major,
            bindir=bindir,
            is_windows=False,
            required=False,
            allow_path_lookup=False,
        )
        if lld is None:
            lld = _resolve_homebrew_tool(
                formula_names=(f"lld@{major}", "lld"),
                name="ld64.lld",
                major=major,
                required=True,
            )
        assert lld is not None
        tools["lld"] = lld
    else:
        lld = _resolve_llvm_tool(
            name="ld.lld",
            major=major,
            bindir=bindir,
            is_windows=False,
            required=True,
            allow_path_lookup=False,
        )
        assert lld is not None
        tools["lld"] = lld

    resource_dir: Path | None = None
    try:
        resource_dir = Path(_tool_output([str(tools["clang"]), "--print-resource-dir"])).resolve()
    except OpsError:
        resource_dir = None

    sysroot_dir: Path | None = None
    if host.archive_target.endswith("apple-darwin"):
        sdkroot = os.environ.get("SDKROOT")
        if sdkroot:
            candidate = Path(sdkroot).expanduser()
            if candidate.exists():
                sysroot_dir = candidate.resolve()

    return BundledToolchain(
        source_label=source_label,
        prefix=prefix,
        bindir=bindir,
        libdir=libdir,
        includedir=includedir,
        version=version,
        tools=tools,
        resource_dir=resource_dir,
        sysroot_dir=sysroot_dir,
    )


def copy_directory_contents(source: Path, dest: Path) -> None:
    ensure(source.is_dir(), f"directory `{source}` does not exist")
    dest.mkdir(parents=True, exist_ok=True)
    for child in sorted(source.iterdir()):
        target = dest / child.name
        if child.is_dir():
            shutil.copytree(child, target, dirs_exist_ok=True)
        else:
            shutil.copy2(child, target)
