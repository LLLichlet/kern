from __future__ import annotations

import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

from .common import OpsError, read_json, repo_root, sha256_file


CI_TOOLCHAINS_MANIFEST = repo_root() / "manifest" / "ci-toolchains.json"


@dataclass(frozen=True)
class CiToolchainPolicy:
    runner_os: str
    mode: str
    host_target: str | None
    llvm_version: str
    llvm_major: int
    prefix_env: str
    provider_kind: str
    provider: str
    target_provider_kind: str
    target_provider: str
    required_tools: tuple[str, ...]
    raw: dict[str, object]


def normalize_runner_os(value: str) -> str:
    normalized = value.strip().lower()
    if normalized in {"linux"}:
        return "Linux"
    if normalized in {"macos", "macosx", "darwin"}:
        return "macOS"
    if normalized in {"windows", "win32"}:
        return "Windows"
    raise OpsError(f"unsupported runner OS `{value}`")


def load_ci_toolchains_manifest() -> dict[str, object]:
    payload = read_json(CI_TOOLCHAINS_MANIFEST)
    if payload.get("schema_version") != 1:
        raise OpsError(f"unsupported CI toolchain manifest schema in `{CI_TOOLCHAINS_MANIFEST}`")
    toolchains = payload.get("toolchains")
    if not isinstance(toolchains, dict):
        raise OpsError(f"invalid CI toolchain manifest: missing `toolchains` in `{CI_TOOLCHAINS_MANIFEST}`")
    return payload


def _normalize_policy_mode(mode: str) -> str:
    normalized = mode.strip().lower()
    if normalized in {"current", "bootstrap", "target"}:
        return normalized
    raise OpsError(f"unsupported CI toolchain mode `{mode}`")


def _resolve_policy_view(policy: dict[str, object], mode: str) -> dict[str, object]:
    if mode == "current":
        return dict(policy)

    effective = dict(policy)
    prefix = f"{mode}_"
    for key, value in policy.items():
        if not key.startswith(prefix):
            continue
        effective[key.removeprefix(prefix)] = value
    return effective


def _merge_host_target_overrides(
    policy: dict[str, object],
    *,
    host_target: str | None,
) -> dict[str, object]:
    effective = dict(policy)
    normalized_host_target = host_target.strip() if isinstance(host_target, str) else None
    if not normalized_host_target:
        return effective

    host_targets = policy.get("host_targets")
    if host_targets is None:
        return effective
    if not isinstance(host_targets, dict):
        raise OpsError(f"invalid host_targets in `{CI_TOOLCHAINS_MANIFEST}`")

    override = host_targets.get(normalized_host_target)
    if override is None:
        raise OpsError(
            f"missing CI toolchain host_target `{normalized_host_target}` in `{CI_TOOLCHAINS_MANIFEST}`"
        )
    if not isinstance(override, dict):
        raise OpsError(
            f"invalid host_target override `{normalized_host_target}` in `{CI_TOOLCHAINS_MANIFEST}`"
        )

    effective.update(override)
    return effective


def resolve_ci_toolchain_policy(
    runner_os: str,
    *,
    mode: str = "current",
    host_target: str | None = None,
) -> CiToolchainPolicy:
    manifest = load_ci_toolchains_manifest()
    toolchains = manifest["toolchains"]
    assert isinstance(toolchains, dict)
    normalized = normalize_runner_os(runner_os)
    normalized_mode = _normalize_policy_mode(mode)
    normalized_host_target = host_target.strip() if isinstance(host_target, str) else None
    if normalized_host_target == "":
        normalized_host_target = None
    policy = toolchains.get(normalized)
    if not isinstance(policy, dict):
        raise OpsError(f"missing CI toolchain policy for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    effective_policy = _resolve_policy_view(
        _merge_host_target_overrides(policy, host_target=normalized_host_target),
        normalized_mode,
    )

    llvm_version = effective_policy.get("llvm_version")
    llvm_major = effective_policy.get("llvm_major")
    prefix_env = effective_policy.get("prefix_env")
    provider_kind = effective_policy.get("provider_kind")
    provider = effective_policy.get("provider")
    target_provider_kind = effective_policy.get("target_provider_kind")
    target_provider = effective_policy.get("target_provider")
    required_tools = effective_policy.get("required_tools")

    if not isinstance(llvm_version, str) or not llvm_version:
        raise OpsError(f"invalid llvm_version for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(llvm_major, int) or llvm_major <= 0:
        raise OpsError(f"invalid llvm_major for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(prefix_env, str) or not prefix_env:
        raise OpsError(f"invalid prefix_env for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(provider_kind, str) or not provider_kind:
        raise OpsError(f"invalid provider_kind for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(provider, str) or not provider:
        raise OpsError(f"invalid provider for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(target_provider_kind, str) or not target_provider_kind:
        raise OpsError(f"invalid target_provider_kind for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(target_provider, str) or not target_provider:
        raise OpsError(f"invalid target_provider for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")
    if not isinstance(required_tools, list) or not all(
        isinstance(tool, str) and tool for tool in required_tools
    ):
        raise OpsError(f"invalid required_tools for `{normalized}` in `{CI_TOOLCHAINS_MANIFEST}`")

    return CiToolchainPolicy(
        runner_os=normalized,
        mode=normalized_mode,
        host_target=normalized_host_target,
        llvm_version=llvm_version,
        llvm_major=llvm_major,
        prefix_env=prefix_env,
        provider_kind=provider_kind,
        provider=provider,
        target_provider_kind=target_provider_kind,
        target_provider=target_provider,
        required_tools=tuple(required_tools),
        raw=effective_policy,
    )


def render_ci_toolchain_env(policy: CiToolchainPolicy) -> str:
    format_vars = {
        "llvm_version": policy.llvm_version,
        "host_target": policy.host_target or "",
    }
    lines = [
        f"KERN_CI_RUNNER_OS={policy.runner_os}",
        f"KERN_CI_MODE={policy.mode}",
        f"KERN_CI_HOST_TARGET={policy.host_target or ''}",
        f"KERN_CI_LLVM_VERSION={policy.llvm_version}",
        f"KERN_CI_LLVM_MAJOR={policy.llvm_major}",
        f"KERN_CI_LLVM_PREFIX_ENV={policy.prefix_env}",
        f"KERN_CI_PROVIDER_KIND={policy.provider_kind}",
        f"KERN_CI_TOOLCHAIN_PROVIDER={policy.provider}",
        f"KERN_CI_TARGET_PROVIDER_KIND={policy.target_provider_kind}",
        f"KERN_CI_TARGET_TOOLCHAIN_PROVIDER={policy.target_provider}",
        f"KERN_CI_REQUIRED_TOOLS={' '.join(policy.required_tools)}",
    ]

    if policy.provider_kind == "archive":
        archive_url = policy.raw.get("archive_url")
        archive_sha256 = policy.raw.get("archive_sha256")
        archive_root = policy.raw.get("archive_root")
        install_dir = policy.raw.get("install_dir")
        archive_prefix_subdir = policy.raw.get("archive_prefix_subdir", "")
        if not all(isinstance(value, str) and value for value in (archive_url, archive_root, install_dir)):
            raise OpsError(f"invalid archive-based CI toolchain policy for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")
        if not isinstance(archive_prefix_subdir, str):
            raise OpsError(
                f"invalid archive_prefix_subdir for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`"
            )
        lines.extend(
            [
                f"KERN_CI_ARCHIVE_URL={archive_url.format(**format_vars)}",
                f"KERN_CI_ARCHIVE_SHA256={archive_sha256 or ''}",
                f"KERN_CI_ARCHIVE_ROOT={archive_root.format(**format_vars)}",
                f"KERN_CI_INSTALL_DIR={install_dir}",
                f"KERN_CI_ARCHIVE_PREFIX_SUBDIR={archive_prefix_subdir}",
            ]
        )
        if policy.runner_os == "Windows":
            vcpkg_package = policy.raw.get("vcpkg_package", "")
            vcpkg_cache_key = policy.raw.get("vcpkg_cache_key", "")
            if not isinstance(vcpkg_package, str) or not isinstance(vcpkg_cache_key, str):
                raise OpsError(f"invalid Windows CI toolchain policy in `{CI_TOOLCHAINS_MANIFEST}`")
            lines.extend(
                [
                    f"KERN_CI_WINDOWS_VCPKG_PACKAGE={vcpkg_package}",
                    f"KERN_CI_WINDOWS_VCPKG_CACHE_KEY={vcpkg_cache_key}",
                ]
            )
    elif policy.runner_os == "Linux":
        packages = policy.raw.get("apt_packages")
        if not isinstance(packages, list) or not all(isinstance(item, str) and item for item in packages):
            raise OpsError(f"invalid apt_packages for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")
        lines.append(f"KERN_CI_APT_PACKAGES={' '.join(packages)}")
    elif policy.runner_os == "macOS":
        primary = policy.raw.get("primary_formula")
        fallback = policy.raw.get("fallback_formula")
        extras = policy.raw.get("extra_formulas")
        if not isinstance(primary, str) or not primary:
            raise OpsError(f"invalid primary_formula for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")
        if not isinstance(fallback, str) or not fallback:
            raise OpsError(f"invalid fallback_formula for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")
        if not isinstance(extras, list) or not all(isinstance(item, str) and item for item in extras):
            raise OpsError(f"invalid extra_formulas for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")
        lines.extend(
            [
                f"KERN_CI_BREW_PRIMARY_FORMULA={primary}",
                f"KERN_CI_BREW_FALLBACK_FORMULA={fallback}",
                f"KERN_CI_BREW_EXTRA_FORMULAS={' '.join(extras)}",
            ]
        )
    return "\n".join(lines) + "\n"


def verify_ci_toolchain_archive(
    runner_os: str,
    archive_path: str,
    *,
    mode: str = "current",
    host_target: str | None = None,
) -> str:
    policy = resolve_ci_toolchain_policy(runner_os, mode=mode, host_target=host_target)
    if policy.provider_kind != "archive":
        raise OpsError(
            f"runner OS `{policy.runner_os}` does not use an archive-based {policy.mode} provider in `{CI_TOOLCHAINS_MANIFEST}`"
        )

    archive = Path(archive_path).expanduser().resolve()
    if not archive.is_file():
        raise OpsError(f"toolchain archive `{archive}` does not exist")

    expected_sha256 = _resolve_expected_archive_sha256(policy)
    if expected_sha256 is None:
        return f"toolchain archive verification skipped for `{archive}`; no archive_sha256 is pinned yet"

    actual_sha256 = sha256_file(archive)
    if actual_sha256 != expected_sha256:
        raise OpsError(
            f"toolchain archive checksum mismatch for `{archive}`: expected {expected_sha256}, got {actual_sha256}"
        )
    return f"toolchain archive checksum verified for `{archive}`"


def _resolve_expected_archive_sha256(policy: CiToolchainPolicy) -> str | None:
    expected_sha256 = policy.raw.get("archive_sha256")
    if expected_sha256 not in (None, ""):
        if not isinstance(expected_sha256, str):
            raise OpsError(f"invalid archive_sha256 for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")
        return expected_sha256

    checksum_url = policy.raw.get("archive_sha256_url")
    if checksum_url in (None, ""):
        return None
    if not isinstance(checksum_url, str):
        raise OpsError(f"invalid archive_sha256_url for `{policy.runner_os}` in `{CI_TOOLCHAINS_MANIFEST}`")

    try:
        with urllib.request.urlopen(checksum_url) as response:
            payload = response.read().decode("utf-8")
    except urllib.error.URLError as err:
        raise OpsError(f"failed to download archive checksum from `{checksum_url}`: {err}") from err

    first_line = next((line.strip() for line in payload.splitlines() if line.strip()), "")
    if not first_line:
        raise OpsError(f"archive checksum file `{checksum_url}` is empty")

    checksum = first_line.split()[0]
    if len(checksum) != 64 or any(ch not in "0123456789abcdefABCDEF" for ch in checksum):
        raise OpsError(f"archive checksum file `{checksum_url}` does not start with a sha256 digest")
    return checksum.lower()
