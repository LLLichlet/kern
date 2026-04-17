from __future__ import annotations

import os
import shutil
import tarfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

from .common import (
    OpsError,
    detect_host_target,
    ensure,
    find_kern_toolchain_root,
    find_llvm_sys_prefix,
    file_size,
    info,
    load_workspace_version,
    make_temp_dir,
    read_json,
    repo_root,
    resolve_bundled_toolchain,
    run,
    run_capture,
    sha256_directory,
    sha256_file,
)
from .toolchains import (
    render_ci_toolchain_env,
    resolve_ci_toolchain_policy,
    verify_ci_toolchain_archive,
)

SMOKE_TESTS = (
    "anonymous_aggregates",
    "atomics",
    "regressions",
    "stdlib",
    "traits",
)

HOSTED_TESTS = (
    "collections",
    "filesystem",
)


@dataclass(frozen=True)
class KerncTestsArgs:
    mode: str


@dataclass(frozen=True)
class ToolchainSpecArgs:
    runner_os: str
    mode: str
    host_target: str | None
    format: str


@dataclass(frozen=True)
class ToolchainArchiveVerifyArgs:
    runner_os: str
    mode: str
    host_target: str | None
    archive_path: str


@dataclass(frozen=True)
class PackagedToolchainVerifyArgs:
    archive_path: str
    target: str | None


@dataclass(frozen=True)
class PackagedToolchainInstallArgs:
    archive_path: str
    dest: str
    target: str | None
    format: str


def run_kernc_tests(args: KerncTestsArgs) -> int:
    modes = {
        "smoke": (("smoke", SMOKE_TESTS),),
        "hosted": (("hosted", HOSTED_TESTS),),
        "all": (("smoke", SMOKE_TESTS), ("hosted", HOSTED_TESTS)),
    }
    if args.mode not in modes:
        raise OpsError(f"unknown kernc test mode `{args.mode}`")

    for label, tests in modes[args.mode]:
        info(f"Running {label} suite...")
        for test_name in tests:
            run(["cargo", "test", "-p", "kernc_cli", "--test", test_name])
    return 0


def run_craft_policy_checks() -> int:
    root = repo_root()
    current_kern_version = load_workspace_version()
    fixtures_root = root / "tools" / "craft" / "fixtures" / "release-policy"
    temp_root = make_temp_dir("craft-policy-")

    try:
        allowed = _prepare_fixture(fixtures_root / "allowed", temp_root, current_kern_version)
        allowed_exception = _prepare_fixture(
            fixtures_root / "allowed-exception", temp_root, current_kern_version
        )
        blocked = _prepare_fixture(fixtures_root / "blocked", temp_root, current_kern_version)

        info("Running craft release policy allow fixture...")
        run(["cargo", "run", "-p", "craft", "--", "check", "--project-path", str(allowed), "--profile", "release"])

        info("Running craft release policy allow-exception fixture...")
        run(
            [
                "cargo",
                "run",
                "-p",
                "craft",
                "--",
                "check",
                "--project-path",
                str(allowed_exception),
                "--profile",
                "release",
            ]
        )

        info("Running craft release policy block fixture...")
        result = run_capture(
            [
                "cargo",
                "run",
                "-p",
                "craft",
                "--",
                "check",
                "--project-path",
                str(blocked),
                "--profile",
                "release",
            ]
        )
        ensure(result.returncode != 0, f"craft release policy fixture unexpectedly passed: {blocked}")
        output = f"{result.stdout}{result.stderr}"
        ensure(
            "release source policy rejected" in output,
            "craft release policy fixture failed for an unexpected reason",
        )
        info("craft release policy fixtures passed")
        return 0
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)


def print_toolchain_info() -> int:
    host = detect_host_target()
    info(f"runner_target: {host.archive_target}")
    info(f"KERN_TOOLCHAIN_ROOT: {os.environ.get('KERN_TOOLCHAIN_ROOT', '<unset>')}")
    info(f"CC: {os.environ.get('CC', '<unset>')}")
    info(f"CXX: {os.environ.get('CXX', '<unset>')}")

    configured_root = find_kern_toolchain_root()
    if configured_root is None:
        info("configured_toolchain_root: <unset>")
    else:
        label, path = configured_root
        info(f"{label}: {path}")

    env_prefix = find_llvm_sys_prefix()
    if env_prefix is None:
        info("LLVM_SYS prefix: <unset>")
    else:
        label, path = env_prefix
        info(f"{label}: {path}")

    for name in (
        "cc",
        "clang",
        "clang++",
        "ld",
        "ld.lld",
        "ld64.lld",
        "lld-link",
        "llvm-lib",
        "llvm-config",
        "llvm-config-21",
    ):
        resolved = shutil.which(name)
        info(f"{name}: {resolved or '<missing>'}")
        if resolved is None:
            continue
        completed = run_capture([resolved, "--version"])
        first_line = next(
            (
                line.strip()
                for line in ((completed.stdout or "") + (completed.stderr or "")).splitlines()
                if line.strip()
            ),
            "<no version output>",
        )
        info(f"{name} --version: {first_line}")

    try:
        bundled = resolve_bundled_toolchain(host)
    except OpsError as err:
        info(f"resolved_toolchain: <unavailable> ({err})")
        return 0

    info(f"resolved_toolchain.prefix: {bundled.prefix}")
    info(f"resolved_toolchain.bindir: {bundled.bindir}")
    info(f"resolved_toolchain.libdir: {bundled.libdir}")
    info(f"resolved_toolchain.includedir: {bundled.includedir}")
    info(f"resolved_toolchain.version: {bundled.version}")
    for name, path in bundled.tools.items():
        info(f"resolved_toolchain.tool.{name}: {path}")
    if bundled.resource_dir is not None:
        info(f"resolved_toolchain.resource_dir: {bundled.resource_dir}")
    if bundled.sysroot_dir is not None:
        info(f"resolved_toolchain.sysroot_dir: {bundled.sysroot_dir}")
    return 0


def print_toolchain_spec(args: ToolchainSpecArgs) -> int:
    policy = resolve_ci_toolchain_policy(args.runner_os, mode=args.mode, host_target=args.host_target)
    if args.format == "github-env":
        print(render_ci_toolchain_env(policy), end="")
        return 0

    info(f"toolchain_policy.runner_os: {policy.runner_os}")
    info(f"toolchain_policy.mode: {policy.mode}")
    info(f"toolchain_policy.host_target: {policy.host_target or '<unset>'}")
    info(f"toolchain_policy.provider_kind: {policy.provider_kind}")
    info(f"toolchain_policy.provider: {policy.provider}")
    info(f"toolchain_policy.target_provider_kind: {policy.target_provider_kind}")
    info(f"toolchain_policy.target_provider: {policy.target_provider}")
    info(f"toolchain_policy.llvm_version: {policy.llvm_version}")
    info(f"toolchain_policy.llvm_major: {policy.llvm_major}")
    info(f"toolchain_policy.prefix_env: {policy.prefix_env}")
    info(f"toolchain_policy.required_tools: {' '.join(policy.required_tools)}")
    for key in sorted(policy.raw):
        if key in {
            "llvm_version",
            "llvm_major",
            "prefix_env",
            "provider_kind",
            "provider",
            "target_provider_kind",
            "target_provider",
            "required_tools",
        }:
            continue
        info(f"toolchain_policy.{key}: {policy.raw[key]}")
    return 0


def verify_toolchain_archive(args: ToolchainArchiveVerifyArgs) -> int:
    info(
        verify_ci_toolchain_archive(
            args.runner_os,
            args.archive_path,
            mode=args.mode,
            host_target=args.host_target,
        )
    )
    return 0


def verify_packaged_toolchain_archive(args: PackagedToolchainVerifyArgs) -> int:
    archive = Path(args.archive_path).expanduser().resolve()
    if not archive.is_file():
        raise OpsError(f"packaged toolchain archive `{archive}` does not exist")

    host = detect_host_target()
    expected_target = args.target or host.archive_target
    temp_root = make_temp_dir("kern-toolchain-verify-")
    try:
        extract_root = temp_root / "extract"
        extract_root.mkdir(parents=True, exist_ok=True)
        root = _extract_archive_for_validation(archive, extract_root)
        _validate_toolchain_root(root, expected_target)
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)

    info(f"packaged toolchain archive verified: {archive}")
    return 0


def install_packaged_toolchain_archive(args: PackagedToolchainInstallArgs) -> int:
    archive = Path(args.archive_path).expanduser().resolve()
    if not archive.is_file():
        raise OpsError(f"packaged toolchain archive `{archive}` does not exist")

    host = detect_host_target()
    expected_target = args.target or host.archive_target
    install_root = Path(args.dest).expanduser().resolve()

    temp_root = make_temp_dir("kern-toolchain-install-")
    try:
        extract_root = temp_root / "extract"
        extract_root.mkdir(parents=True, exist_ok=True)
        root = _extract_archive_for_validation(archive, extract_root)
        _validate_toolchain_root(root, expected_target)

        if install_root.exists():
            shutil.rmtree(install_root)
        shutil.copytree(root, install_root)
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)

    prefix = install_root / "toolchain" / "host"
    ensure(prefix.is_dir(), f"installed packaged toolchain root `{prefix}` is missing")
    prefix_env = resolve_ci_toolchain_policy(
        _runner_os_for_target(expected_target),
        host_target=expected_target,
    ).prefix_env

    if args.format == "github-env":
        print(f"KERN_CI_PACKAGED_TOOLCHAIN_ROOT={prefix}")
        print(f"KERN_TOOLCHAIN_ROOT={prefix}")
        print(f"{prefix_env}={prefix}")
    else:
        info(f"packaged_toolchain.install_root: {install_root}")
        info(f"packaged_toolchain.prefix: {prefix}")
    return 0


def assert_toolchain_health() -> int:
    host = detect_host_target()
    policy = resolve_ci_toolchain_policy(_runner_os_for_host(host))
    try:
        bundled = resolve_bundled_toolchain(host)
    except OpsError as err:
        if policy.runner_os == "macOS":
            raise OpsError(
                "release-grade host toolchain is incomplete for macOS; expected a controlled LLVM toolchain with `"
                + "`, `".join(policy.required_tools)
                + "`"
            ) from err
        if policy.runner_os == "Linux":
            raise OpsError(
                "release-grade host toolchain is incomplete for Linux; expected a controlled LLVM toolchain with `"
                + "`, `".join(policy.required_tools)
                + "`"
            ) from err
        raise OpsError(
            "release-grade host toolchain is incomplete for Windows; expected a controlled LLVM toolchain with `"
            + "`, `".join(policy.required_tools)
            + "`"
        ) from err

    required = [_component_key_from_tool(tool) for tool in policy.required_tools]

    info(f"toolchain_health.target: {host.archive_target}")
    info(f"toolchain_health.source: {bundled.source_label}")
    info(f"toolchain_health.version: {bundled.version}")

    missing = [name for name in required if name not in bundled.tools]
    ensure(
        not missing,
        f"resolved toolchain is missing required components for {host.archive_target}: {', '.join(missing)}",
    )

    for name in required:
        path = bundled.tools[name]
        ensure(path.is_file(), f"required tool `{name}` is missing at `{path}`")
        completed = run_capture([str(path), "--version"])
        ensure(
            completed.returncode == 0,
            f"required tool `{name}` did not answer `--version`: {(completed.stdout or '')}{(completed.stderr or '')}".strip(),
        )
        first_line = next(
            (
                line.strip()
                for line in ((completed.stdout or "") + (completed.stderr or "")).splitlines()
                if line.strip()
            ),
            "<no version output>",
        )
        info(f"toolchain_health.{name}: {path} :: {first_line}")

    ensure(bundled.libdir.is_dir(), f"toolchain libdir `{bundled.libdir}` is missing")
    ensure(bundled.includedir.is_dir(), f"toolchain includedir `{bundled.includedir}` is missing")
    ensure(
        bundled.resource_dir is None or bundled.resource_dir.is_dir(),
        f"clang resource dir `{bundled.resource_dir}` is missing",
    )
    info("toolchain_health.status: ok")
    return 0


def _extract_archive_for_validation(archive_path: Path, extract_root: Path) -> Path:
    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path) as archive:
            archive.extractall(extract_root)
    else:
        with tarfile.open(archive_path, "r:*") as archive:
            archive.extractall(extract_root)

    roots = [path for path in extract_root.iterdir() if path.is_dir()]
    ensure(len(roots) == 1, f"expected exactly one packaged toolchain root in `{archive_path}`")
    return roots[0]


def _validate_toolchain_root(toolchain_root: Path, expected_target: str) -> None:
    manifest_path = toolchain_root / "manifest" / "toolchain.json"
    ensure(manifest_path.is_file(), f"toolchain manifest `{manifest_path}` is missing")
    manifest = read_json(manifest_path)
    ensure(
        manifest.get("host_target") == expected_target,
        f"toolchain host target mismatch in `{manifest_path}`",
    )
    ensure((toolchain_root / "toolchain" / "host").is_dir(), "toolchain host layout is incomplete")

    components = manifest.get("components")
    ensure(isinstance(components, dict), "toolchain manifest components are invalid")

    required = ["clang", "clangxx", "lld", "llvm_ar", "llvm_config", "lib_dir", "include_dir"]
    if expected_target.endswith("windows-msvc"):
        required.append("llvm_lib")

    for component in required:
        entry = components.get(component)
        ensure(isinstance(entry, dict), f"toolchain manifest is missing component `{component}`")
        _validate_component_record(toolchain_root, component, entry)

    resource_entry = components.get("clang_resource_dir")
    if isinstance(resource_entry, dict):
        _validate_component_record(toolchain_root, "clang_resource_dir", resource_entry)


def _validate_component_record(root: Path, component: str, entry: dict[str, object]) -> None:
    relative_path = entry.get("path")
    ensure(
        isinstance(relative_path, str) and relative_path,
        f"toolchain manifest component `{component}` has no path",
    )
    kind = entry.get("kind", "file")
    ensure(isinstance(kind, str), f"toolchain manifest component `{component}` has an invalid kind")
    target = root / relative_path

    if kind == "directory":
        ensure(target.is_dir(), f"toolchain directory component `{component}` is missing at `{target}`")
        expected_sha = entry.get("sha256")
        if isinstance(expected_sha, str) and expected_sha:
            actual_sha = sha256_directory(target)
            ensure(
                actual_sha == expected_sha,
                f"toolchain directory `{component}` checksum mismatch at `{target}`",
            )
        return

    ensure(target.is_file(), f"toolchain component `{component}` is missing at `{target}`")
    expected_size = entry.get("size")
    if isinstance(expected_size, int):
        ensure(
            file_size(target) == expected_size,
            f"toolchain component `{component}` size mismatch at `{target}`",
        )
    expected_sha = entry.get("sha256")
    if isinstance(expected_sha, str) and expected_sha:
        actual_sha = sha256_file(target)
        ensure(
            actual_sha == expected_sha,
            f"toolchain component `{component}` checksum mismatch at `{target}`",
        )


def _prepare_fixture(source_dir: Path, temp_root: Path, current_kern_version: str) -> Path:
    destination = temp_root / source_dir.name
    shutil.copytree(source_dir, destination)
    manifest_path = destination / "Craft.toml"
    manifest_source = manifest_path.read_text(encoding="utf-8")
    updated = []
    for line in manifest_source.splitlines():
        if line.startswith('kern = "'):
            updated.append(f'kern = "{current_kern_version}"')
        else:
            updated.append(line)
    manifest_path.write_text("\n".join(updated) + "\n", encoding="utf-8")
    return destination


def _runner_os_for_host(host: object) -> str:
    archive_target = getattr(host, "archive_target")
    return _runner_os_for_target(archive_target)


def _runner_os_for_target(archive_target: str) -> str:
    if archive_target.endswith("linux-gnu"):
        return "Linux"
    if archive_target.endswith("apple-darwin"):
        return "macOS"
    return "Windows"


def _component_key_from_tool(tool: str) -> str:
    return {
        "clang": "clang",
        "clang++": "clangxx",
        "llvm-ar": "llvm_ar",
        "llvm-config": "llvm_config",
        "ld.lld": "lld",
        "ld64.lld": "lld",
        "lld-link": "lld",
        "llvm-lib": "llvm_lib",
    }[tool]
