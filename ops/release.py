from __future__ import annotations

import os
import shutil
import tarfile
import zipfile
from glob import glob
from dataclasses import dataclass
from pathlib import Path

from .common import (
    ArtifactRecord,
    HOST_TOOL_BINARIES,
    OFFICIAL_LIBRARY_LAYERS,
    BundledToolchain,
    HostTarget,
    bundled_resource_dir_path,
    canonical_toolchain_component_names,
    copy_directory_contents,
    detect_host_target,
    ensure,
    file_size,
    info,
    load_workspace_version,
    repo_root,
    require_tool,
    resolve_bundled_toolchain,
    run,
    run_capture,
    sha256_directory,
    sha256_file,
    sdk_manifest,
    toolchain_manifest,
    write_json,
)


@dataclass(frozen=True)
class ReleasePackageArgs:
    version: str | None
    target: str | None
    skip_build: bool
    toolchain_prefix: str | None


@dataclass(frozen=True)
class ReleaseToolchainPackageArgs:
    version: str | None
    target: str | None
    toolchain_prefix: str | None


@dataclass(frozen=True)
class ReleaseChecksumsArgs:
    paths: tuple[str, ...]
    manifest_path: str | None
    channel: str
    release_tag: str | None


def package_release(args: ReleasePackageArgs) -> int:
    root = repo_root()
    host = detect_host_target()
    version = args.version or f"v{load_workspace_version()}"
    archive_target = args.target or host.archive_target
    bundled_toolchain = resolve_bundled_toolchain(host, explicit_prefix=args.toolchain_prefix)

    ensure(
        archive_target == host.archive_target,
        (
            f"target label `{archive_target}` does not match the current host "
            f"`{host.archive_target}`; release packaging is host-native in this phase"
        ),
    )

    if host.is_windows:
        _package_windows(root, host, version, args.skip_build, bundled_toolchain)
    else:
        _package_unix(root, host, version, args.skip_build, bundled_toolchain)
    return 0


def package_toolchain_release(args: ReleaseToolchainPackageArgs) -> int:
    root = repo_root()
    host = detect_host_target()
    bundled_toolchain = resolve_bundled_toolchain(host, explicit_prefix=args.toolchain_prefix)
    version = args.version or f"llvm-{bundled_toolchain.version}"
    archive_target = args.target or host.archive_target

    ensure(
        archive_target == host.archive_target,
        (
            f"target label `{archive_target}` does not match the current host "
            f"`{host.archive_target}`; toolchain packaging is host-native in this phase"
        ),
    )

    if host.is_windows:
        _package_windows_toolchain(root, host, version, bundled_toolchain)
    else:
        _package_unix_toolchain(root, host, version, bundled_toolchain)
    return 0


def write_release_checksums(args: ReleaseChecksumsArgs) -> int:
    root = repo_root()
    resolved = _resolve_checksum_inputs(root, args.paths)
    ensure(resolved, "no release artifacts matched for checksum generation")

    records: list[dict[str, object]] = []
    for artifact in resolved:
        digest = sha256_file(artifact)
        sidecar = artifact.with_name(f"{artifact.name}.sha256")
        sidecar.write_text(f"{digest}  {artifact.name}\n", encoding="utf-8")
        records.append(
            {
                "name": artifact.name,
                "path": artifact.name,
                "sha256": digest,
                "size": file_size(artifact),
                "sha256_sidecar": sidecar.name,
            }
        )

    if args.manifest_path is not None:
        manifest_path = Path(args.manifest_path)
        if not manifest_path.is_absolute():
            manifest_path = root / manifest_path
        write_json(
            manifest_path,
            {
                "schema_version": 1,
                "channel": args.channel,
                "release_tag": args.release_tag,
                "assets": records,
            },
        )

    info(f"Generated checksums for {len(records)} release artifact(s)")
    return 0


def _package_unix(
    root: Path,
    host: HostTarget,
    version: str,
    skip_build: bool,
    bundled_toolchain: BundledToolchain,
) -> None:
    require_tool("cargo")
    require_tool("tar")
    if not skip_build:
        info("Building release binaries...")
        run(["cargo", "build", "--release", "-p", "kernc_cli", "--bin", "kernc"])
        run(["cargo", "build", "--release", "-p", "craft"])
        run(["cargo", "build", "--release", "-p", "kern-lsp"])

    dist_name = f"kern-{version}-{host.archive_target}"
    dist_dir = root / dist_name
    archive_path = root / f"{dist_name}.tar.gz"
    _prepare_dist_dir(root, dist_dir, host, None, version, bundled_toolchain)

    if archive_path.exists():
        archive_path.unlink()

    info(f"Packaging {dist_name}...")
    with tarfile.open(archive_path, "w:gz") as archive:
        archive.add(dist_dir, arcname=dist_name)

    info(f"Successfully packaged: {archive_path.name}")


def _package_windows(
    root: Path,
    host: HostTarget,
    version: str,
    skip_build: bool,
    bundled_toolchain: BundledToolchain,
) -> None:
    require_tool("cargo")
    cargo_target = host.cargo_target
    assert cargo_target is not None

    build_env = os.environ.copy()
    build_env["CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"] = "-C target-feature=+crt-static"
    build_args = ["cargo", "build", "--release", "--target", cargo_target]

    if not skip_build:
        info("Building release binaries...")
        run([*build_args, "-p", "kernc_cli", "--bin", "kernc"], env=build_env)
        run([*build_args, "-p", "craft"], env=build_env)
        run([*build_args, "-p", "kern-lsp"], env=build_env)

    dist_name = f"kern-{version}-{host.archive_target}"
    dist_dir = root / dist_name
    archive_path = root / f"{dist_name}.zip"
    _prepare_dist_dir(root, dist_dir, host, cargo_target, version, bundled_toolchain)

    if archive_path.exists():
        archive_path.unlink()

    info(f"Packaging {dist_name}...")
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(dist_dir.rglob("*")):
            if path.is_dir():
                continue
            archive.write(path, path.relative_to(root))

    info(f"Successfully packaged: {archive_path.name}")


def _package_unix_toolchain(
    root: Path,
    host: HostTarget,
    version: str,
    bundled_toolchain: BundledToolchain,
) -> None:
    require_tool("tar")

    dist_name = f"kern-toolchain-{version}-{host.archive_target}"
    dist_dir = root / dist_name
    archive_path = root / f"{dist_name}.tar.gz"
    _prepare_toolchain_dist_dir(root, dist_dir, host, version, bundled_toolchain)

    if archive_path.exists():
        archive_path.unlink()

    info(f"Packaging {dist_name}...")
    with tarfile.open(archive_path, "w:gz") as archive:
        archive.add(dist_dir, arcname=dist_name)

    info(f"Successfully packaged: {archive_path.name}")


def _package_windows_toolchain(
    root: Path,
    host: HostTarget,
    version: str,
    bundled_toolchain: BundledToolchain,
) -> None:
    dist_name = f"kern-toolchain-{version}-{host.archive_target}"
    dist_dir = root / dist_name
    archive_path = root / f"{dist_name}.zip"
    _prepare_toolchain_dist_dir(root, dist_dir, host, version, bundled_toolchain)

    if archive_path.exists():
        archive_path.unlink()

    info(f"Packaging {dist_name}...")
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(dist_dir.rglob("*")):
            if path.is_dir():
                continue
            archive.write(path, path.relative_to(root))

    info(f"Successfully packaged: {archive_path.name}")


def _prepare_dist_dir(
    root: Path,
    dist_dir: Path,
    host: HostTarget,
    cargo_target: str | None,
    version: str,
    bundled_toolchain: BundledToolchain,
) -> None:
    if dist_dir.exists():
        shutil.rmtree(dist_dir)

    (dist_dir / "bin").mkdir(parents=True)
    (dist_dir / "lib" / "kern").mkdir(parents=True)
    (dist_dir / "manifest").mkdir(parents=True)
    (dist_dir / "toolchain" / "host" / "bin").mkdir(parents=True)
    (dist_dir / "toolchain" / "host" / "lib").mkdir(parents=True)
    (dist_dir / "toolchain" / "host" / "sysroot").mkdir(parents=True)

    binary_dir = root / "target" / "release"
    if cargo_target is not None:
        binary_dir = root / "target" / cargo_target / "release"

    for binary in HOST_TOOL_BINARIES:
        source = binary_dir / f"{binary}{host.exe_suffix}"
        ensure(source.is_file(), f"expected release binary `{source}`")
        shutil.copy2(source, dist_dir / "bin" / source.name)

    for layer in OFFICIAL_LIBRARY_LAYERS:
        source = root / "library" / layer
        ensure(source.is_dir(), f"expected library layer `{source}`")
        shutil.copytree(source, dist_dir / "lib" / "kern" / layer)

    craft_sdk = root / "tools" / "craft" / "sdk"
    ensure((craft_sdk / "init.rn").is_file(), f"expected craft SDK `{craft_sdk}`")
    shutil.copytree(craft_sdk, dist_dir / "lib" / "kern" / "craft")

    for text_file in ("README.md", "LICENSE"):
        shutil.copy2(root / text_file, dist_dir / text_file)

    if host.archive_target.endswith(("linux-gnu", "windows-msvc", "apple-darwin")):
        bundled_component_records = _bundle_sdk_runtime_toolchain(
            dist_dir,
            host,
            bundled_toolchain,
        )
    else:
        bundled_component_records = _bundle_host_toolchain(dist_dir, host, bundled_toolchain)

    write_json(
        dist_dir / "manifest" / "sdk.json",
        sdk_manifest(
            version,
            host.archive_target,
            bundled_toolchain=bundled_toolchain,
            bundled_component_records=bundled_component_records,
        ),
    )


def _prepare_toolchain_dist_dir(
    root: Path,
    dist_dir: Path,
    host: HostTarget,
    version: str,
    bundled_toolchain: BundledToolchain,
) -> None:
    if dist_dir.exists():
        shutil.rmtree(dist_dir)

    (dist_dir / "manifest").mkdir(parents=True)
    (dist_dir / "toolchain" / "host" / "bin").mkdir(parents=True)
    (dist_dir / "toolchain" / "host" / "lib").mkdir(parents=True)
    (dist_dir / "toolchain" / "host" / "sysroot").mkdir(parents=True)

    shutil.copy2(root / "LICENSE", dist_dir / "LICENSE")

    bundled_component_records = _bundle_host_toolchain(dist_dir, host, bundled_toolchain)

    write_json(
        dist_dir / "manifest" / "toolchain.json",
        toolchain_manifest(
            version,
            host.archive_target,
            bundled_toolchain=bundled_toolchain,
            bundled_component_records=bundled_component_records,
        ),
    )

def _bundle_host_toolchain(
    dist_dir: Path,
    host: HostTarget,
    bundled_toolchain: BundledToolchain,
) -> dict[str, ArtifactRecord]:
    host_root = dist_dir / "toolchain" / "host"
    bin_dir = host_root / "bin"
    lib_dir = host_root / "lib"
    sysroot_dir = host_root / "sysroot"
    records: dict[str, ArtifactRecord] = {}

    info(
        "Bundling host LLVM toolchain "
        f"{bundled_toolchain.version} from {bundled_toolchain.source_label}: {bundled_toolchain.prefix}"
    )

    bindir_rel = bundled_toolchain.bindir.relative_to(bundled_toolchain.prefix)
    libdir_rel = bundled_toolchain.libdir.relative_to(bundled_toolchain.prefix)
    includedir_rel = bundled_toolchain.includedir.relative_to(bundled_toolchain.prefix)
    canonical_names = canonical_toolchain_component_names(host.archive_target)

    copied_bin_dir = host_root / bindir_rel
    copied_lib_dir = host_root / libdir_rel
    copied_include_dir = host_root / includedir_rel

    copy_directory_contents(bundled_toolchain.bindir, copied_bin_dir)
    copy_directory_contents(bundled_toolchain.libdir, copied_lib_dir)
    copy_directory_contents(bundled_toolchain.includedir, copied_include_dir)

    records["bin_dir"] = ArtifactRecord(
        path=copied_bin_dir.relative_to(dist_dir).as_posix(),
        kind="directory",
        sha256=None,
        size=None,
    )
    records["lib_dir"] = ArtifactRecord(
        path=copied_lib_dir.relative_to(dist_dir).as_posix(),
        kind="directory",
        sha256=None,
        size=None,
    )
    records["include_dir"] = ArtifactRecord(
        path=copied_include_dir.relative_to(dist_dir).as_posix(),
        kind="directory",
        sha256=None,
        size=None,
    )

    extra_runtime_libs: set[Path] = set()
    for component, source in bundled_toolchain.tools.items():
        try:
            target = host_root / source.relative_to(bundled_toolchain.prefix)
        except ValueError:
            target = bin_dir / canonical_names.get(component, source.name)
            if not target.exists():
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)
        ensure(target.is_file(), f"bundled toolchain component `{component}` is missing at `{target}`")
        records[component] = ArtifactRecord(
            path=target.relative_to(dist_dir).as_posix(),
            kind="file",
            sha256=sha256_file(target),
            size=file_size(target),
        )

    if host.archive_target.endswith("apple-darwin"):
        extra_runtime_lib_dirs = _external_runtime_libdirs_for_bundled_tools(
            bundled_toolchain.tools.values(),
            bundled_prefix=bundled_toolchain.prefix,
        )
        for extra_lib_dir in sorted(extra_runtime_lib_dirs):
            for dylib in sorted(extra_lib_dir.glob("*.dylib")):
                destination = lib_dir / dylib.name
                if not destination.exists():
                    shutil.copy2(dylib, destination)

        extra_runtime_libs = _macos_collect_external_runtime_libs(
            roots=sorted((host_root / "bin").glob("*")) + sorted(lib_dir.glob("*.dylib")),
            bundled_prefix=bundled_toolchain.prefix,
        )
        for dylib in sorted(extra_runtime_libs):
            destination = lib_dir / dylib.name
            if not destination.exists():
                shutil.copy2(dylib, destination)
    else:
        extra_runtime_lib_dirs = _external_runtime_libdirs_for_bundled_tools(
            bundled_toolchain.tools.values(),
            bundled_prefix=bundled_toolchain.prefix,
        )
        for extra_lib_dir in sorted(extra_runtime_lib_dirs):
            copy_directory_contents(extra_lib_dir, lib_dir)

    if host.archive_target.endswith("apple-darwin"):
        _rewrite_macos_toolchain_load_commands(
            host_root,
            original_libdirs={
                bundled_toolchain.libdir,
                *extra_runtime_lib_dirs,
                *(path.parent for path in extra_runtime_libs),
            },
        )

    if bundled_toolchain.resource_dir is not None and bundled_toolchain.resource_dir.exists():
        resource_dest = dist_dir / bundled_resource_dir_path(bundled_toolchain)
        if not resource_dest.exists():
            copy_directory_contents(bundled_toolchain.resource_dir, resource_dest)
        records["clang_resource_dir"] = ArtifactRecord(
            path=resource_dest.relative_to(dist_dir).as_posix(),
            kind="directory",
            sha256=None,
            size=None,
        )

    if bundled_toolchain.sysroot_dir is not None:
        (sysroot_dir / "README.txt").write_text(
            (
                "The packaging environment exposed a host SDK path, but Kern does not "
                "redistribute platform sysroots from that location.\n"
                f"Observed host SDK path: {bundled_toolchain.sysroot_dir}\n"
            ),
            encoding="utf-8",
        )
    elif not any(sysroot_dir.iterdir()):
        (sysroot_dir / ".empty").write_text(
            "Host OS sysroot contents are not bundled in this SDK.\n",
            encoding="utf-8",
        )

    (dist_dir / "toolchain" / "README.md").write_text(
        "\n".join(
            [
                "# Bundled Host Toolchain",
                "",
                "This SDK bundles the host LLVM/Clang toolchain used for release validation.",
                "",
                f"- Source: {bundled_toolchain.source_label}",
                f"- Version: {bundled_toolchain.version}",
                f"- Bundled bindir: {copied_bin_dir.relative_to(dist_dir).as_posix()}",
                f"- Bundled libdir: {copied_lib_dir.relative_to(dist_dir).as_posix()}",
                f"- Bundled includedir: {copied_include_dir.relative_to(dist_dir).as_posix()}",
                "",
                "The SDK keeps user installs pointed at this bundled toolchain first.",
                "The packaged toolchain preserves a relocatable LLVM development prefix for source builds.",
                "Host OS SDK/libc pieces may still remain platform responsibilities.",
                "",
            ]
        ),
        encoding="utf-8",
    )

    for component, record in list(records.items()):
        if record.kind != "file":
            continue
        target = dist_dir / record.path
        ensure(target.is_file(), f"bundled toolchain component `{component}` is missing at `{target}`")
        records[component] = ArtifactRecord(
            path=record.path,
            kind=record.kind,
            sha256=sha256_file(target),
            size=file_size(target),
        )

    return records


def _bundle_sdk_runtime_toolchain(
    dist_dir: Path,
    host: HostTarget,
    bundled_toolchain: BundledToolchain,
) -> dict[str, ArtifactRecord]:
    host_root = dist_dir / "toolchain" / "host"
    bin_dir = host_root / "bin"
    lib_dir = host_root / "lib"
    records: dict[str, ArtifactRecord] = {}

    info(
        "Bundling runtime host toolchain subset "
        f"{bundled_toolchain.version} from {bundled_toolchain.source_label}: {bundled_toolchain.prefix}"
    )

    runtime_tools = _sdk_runtime_tool_paths(host, bundled_toolchain)
    copied_tools: dict[str, Path] = {}
    for component, source in runtime_tools.items():
        destination = bin_dir / source.name
        shutil.copy2(source, destination)
        copied_tools[component] = destination
        records[component] = ArtifactRecord(
            path=destination.relative_to(dist_dir).as_posix(),
            kind="file",
            sha256=sha256_file(destination),
            size=file_size(destination),
        )

    runtime_libs: set[Path] = set()
    if host.archive_target.endswith("linux-gnu"):
        runtime_libs = _linux_collect_bundled_runtime_libs(
            roots=list(runtime_tools.values()),
            bundled_prefix=bundled_toolchain.prefix,
        )
    elif host.archive_target.endswith("apple-darwin"):
        runtime_libs = _macos_collect_runtime_libs(roots=list(runtime_tools.values()))
    for library in sorted(runtime_libs):
        destination = lib_dir / library.name
        if not destination.exists():
            shutil.copy2(library, destination)

    if host.archive_target.endswith("apple-darwin") and any(lib_dir.iterdir()):
        _rewrite_macos_toolchain_load_commands(
            host_root,
            original_libdirs={
                bundled_toolchain.libdir,
                *(path.parent.resolve() for path in runtime_libs),
            },
        )

    for component, path in copied_tools.items():
        _verify_sdk_runtime_tool_starts(host, component, path)

    if any(lib_dir.iterdir()):
        records["runtime_lib_dir"] = ArtifactRecord(
            path=lib_dir.relative_to(dist_dir).as_posix(),
            kind="directory",
            sha256=sha256_directory(lib_dir),
            size=None,
        )

    (dist_dir / "toolchain" / "README.md").write_text(
        "\n".join(
            [
                "# Bundled Host Toolchain",
                "",
                "This SDK bundles the minimal host LLVM/Clang runtime needed by installed Kern tools.",
                "",
                f"- Source: {bundled_toolchain.source_label}",
                f"- Version: {bundled_toolchain.version}",
                f"- Bundled runtime tools: {', '.join(sorted(path.name for path in runtime_tools.values()))}",
                "",
                "This is intentionally smaller than the standalone toolchain artifact.",
                "End-user SDKs omit the Clang resource dir because Kern only uses Clang as a linker driver here.",
                "Headers, llvm-config, and the full LLVM development prefix are not part of the end-user SDK.",
                "Clone the repository and configure the host environment directly for source builds.",
                "",
            ]
        ),
        encoding="utf-8",
    )

    return records


def _verify_sdk_runtime_tool_starts(host: HostTarget, component: str, path: Path) -> None:
    if component == "llvm_lib":
        probe_output = path.parent / "__kern_llvm_lib_probe.lib"
        try:
            completed = run_capture(
                [str(path), "/llvmlibempty", f"/out:{probe_output}"],
                cwd=path.parent,
            )
        finally:
            if probe_output.exists():
                probe_output.unlink()
    else:
        completed = run_capture([str(path), "--version"], cwd=path.parent)

    ensure(
        completed.returncode == 0,
        (
            f"bundled runtime tool `{path}` failed to start while packaging; "
            "the SDK runtime subset is missing a required dependency"
        ),
    )


def _sdk_runtime_tool_paths(
    host: HostTarget,
    bundled_toolchain: BundledToolchain,
) -> dict[str, Path]:
    if host.archive_target.endswith("windows-msvc"):
        component_names = ("clang", "lld", "llvm_lib")
    else:
        component_names = ("clang", "lld")

    runtime_tools: dict[str, Path] = {}
    for component in component_names:
        tool = bundled_toolchain.tools.get(component)
        ensure(tool is not None, f"bundled toolchain is missing runtime component `{component}`")
        runtime_tools[component] = tool
    return runtime_tools


def _linux_collect_bundled_runtime_libs(
    *,
    roots: list[Path],
    bundled_prefix: Path,
) -> set[Path]:
    require_tool("ldd")

    queued = [path.resolve() for path in roots if path.is_file()]
    visited: set[Path] = set()
    bundled_libs: set[Path] = set()
    bundled_prefix = bundled_prefix.resolve()

    while queued:
        current = queued.pop()
        if current in visited:
            continue
        visited.add(current)

        for dependency in _linux_load_dependencies(current):
            if not _is_linux_bundled_runtime_lib(dependency, bundled_prefix):
                continue
            if dependency in bundled_libs:
                continue
            bundled_libs.add(dependency)
            queued.append(dependency)

    return bundled_libs


def _is_linux_bundled_runtime_lib(dependency: Path, bundled_prefix: Path) -> bool:
    if dependency.is_relative_to(bundled_prefix):
        return True
    return dependency.name.startswith(("libLLVM", "libclang", "libLTO"))


def _linux_load_dependencies(path: Path) -> list[Path]:
    completed = run_capture(["ldd", str(path)])
    ensure(completed.returncode == 0, f"failed to inspect ELF dependencies for `{path}`")

    dependencies: list[Path] = []
    for line in (completed.stdout or "").splitlines():
        stripped = line.strip()
        if (
            not stripped
            or "statically linked" in stripped
            or "not a dynamic executable" in stripped
            or stripped.startswith("linux-vdso")
        ):
            continue

        candidate = ""
        if "=>" in stripped:
            _, rhs = stripped.split("=>", 1)
            candidate = rhs.strip().split(" ", 1)[0]
        else:
            candidate = stripped.split(" ", 1)[0]

        if not candidate.startswith("/"):
            continue

        dependency = Path(candidate)
        if dependency.is_file():
            dependencies.append(dependency.resolve())

    return dependencies


def _external_tool_runtime_libdirs(tool_path: Path) -> list[Path]:
    prefix = tool_path.parent.parent
    candidates = [prefix / "lib", prefix / "lib64"]
    return [candidate for candidate in candidates if candidate.is_dir()]


def _external_runtime_libdirs_for_bundled_tools(
    tool_paths: object,
    *,
    bundled_prefix: Path,
) -> set[Path]:
    extra_runtime_lib_dirs: set[Path] = set()
    for tool_path in tool_paths:
        path = Path(tool_path)
        if path.is_relative_to(bundled_prefix):
            continue
        for candidate in _external_tool_runtime_libdirs(path):
            extra_runtime_lib_dirs.add(candidate)
    return extra_runtime_lib_dirs


def _macos_collect_external_runtime_libs(
    *,
    roots: list[Path],
    bundled_prefix: Path,
) -> set[Path]:
    require_tool("otool")

    bundled_prefix = bundled_prefix.resolve()
    queued = [path.resolve() for path in roots if path.is_file()]
    visited: set[Path] = set()
    external_libs: set[Path] = set()

    while queued:
        current = queued.pop()
        if current in visited:
            continue
        visited.add(current)

        for dependency in _macos_load_dependencies(current):
            for dependency_path in _macos_dependency_candidates(current, dependency):
                if not dependency_path.is_file():
                    continue
                resolved = dependency_path.resolve()
                if resolved.is_relative_to(bundled_prefix):
                    queued.append(resolved)
                    continue
                if _is_macos_system_library(resolved):
                    continue
                # Preserve both the original load-command path and the fully
                # resolved file. Homebrew commonly records dylib dependencies
                # through `/usr/local/opt/...` symlinks while the real payload
                # lives under `/usr/local/Cellar/...`. Packaging only the resolved
                # filename misses compatibility aliases like `libzstd.1.dylib`,
                # which later prevents load-command rewriting from matching the
                # original dependency spelling during verification.
                external_libs.add(dependency_path)
                external_libs.add(resolved)
                queued.append(resolved)

    return external_libs


def _macos_collect_runtime_libs(*, roots: list[Path]) -> set[Path]:
    require_tool("otool")

    queued = [path.resolve() for path in roots if path.is_file()]
    visited: set[Path] = set()
    bundled_libs: set[Path] = set()

    while queued:
        current = queued.pop()
        if current in visited:
            continue
        visited.add(current)

        for dependency in _macos_load_dependencies(current):
            for dependency_path in _macos_dependency_candidates(current, dependency):
                if not dependency_path.is_file():
                    continue
                if _is_macos_system_library(dependency_path):
                    continue

                resolved = dependency_path.resolve()
                if _is_macos_system_library(resolved):
                    continue

                bundled_libs.add(dependency_path)
                bundled_libs.add(resolved)
                queued.append(resolved)

    return bundled_libs


def _is_macos_system_library(path: Path) -> bool:
    raw = str(path)
    return raw.startswith("/usr/lib/") or raw.startswith("/System/Library/")


def _rewrite_macos_toolchain_load_commands(
    host_root: Path,
    *,
    original_libdirs: set[Path],
) -> None:
    require_tool("otool")
    require_tool("install_name_tool")

    lib_dir = (host_root / "lib").resolve()
    original_libdirs = {path.resolve() for path in original_libdirs}
    targets = sorted((host_root / "bin").glob("*")) + sorted(lib_dir.glob("*.dylib"))
    modified: list[Path] = []

    for target in targets:
        if not target.is_file():
            continue

        for dependency in _macos_load_dependencies(target):
            dependency_path = Path(dependency)
            if not dependency_path.is_absolute():
                continue
            dependency_resolved = dependency_path.resolve(strict=False)
            if not any(
                dependency_path.is_relative_to(libdir)
                or dependency_resolved.is_relative_to(libdir)
                for libdir in original_libdirs
            ):
                continue

            replacement = _macos_local_dylib_reference(target, dependency_path.name, lib_dir=lib_dir)
            run(
                [
                    "install_name_tool",
                    "-change",
                    dependency,
                    replacement,
                    str(target),
                ]
            )
            modified.append(target)

        if target.parent == lib_dir and target.suffix == ".dylib":
            dylib_id = _macos_local_dylib_reference(target, target.name, lib_dir=lib_dir)
            run(["install_name_tool", "-id", dylib_id, str(target)])
            modified.append(target)

    if shutil.which("codesign") is not None:
        for target in sorted(set(modified)):
            run(["codesign", "--force", "--sign", "-", str(target)])


def _macos_load_dependencies(path: Path) -> list[str]:
    completed = run_capture(["otool", "-L", str(path)])
    ensure(completed.returncode == 0, f"failed to inspect Mach-O load commands for `{path}`")
    dependencies: list[str] = []
    for line in (completed.stdout or "").splitlines()[1:]:
        stripped = line.strip()
        if not stripped:
            continue
        dependency, _, _ = stripped.partition(" (compatibility version")
        dependencies.append(dependency.strip())
    return dependencies


def _macos_dependency_candidates(path: Path, dependency: str) -> list[Path]:
    if dependency.startswith("/"):
        return [Path(dependency)]

    if dependency.startswith("@loader_path/"):
        return [path.parent / dependency.removeprefix("@loader_path/")]

    if dependency.startswith("@executable_path/"):
        return [path.parent / dependency.removeprefix("@executable_path/")]

    if dependency.startswith("@rpath/"):
        suffix = dependency.removeprefix("@rpath/")
        return [rpath / suffix for rpath in _macos_load_rpaths(path)]

    return []


def _macos_load_rpaths(path: Path) -> list[Path]:
    completed = run_capture(["otool", "-l", str(path)])
    ensure(completed.returncode == 0, f"failed to inspect Mach-O rpaths for `{path}`")

    rpaths: list[Path] = []
    expect_path = False
    for line in (completed.stdout or "").splitlines():
        stripped = line.strip()
        if stripped == "cmd LC_RPATH":
            expect_path = True
            continue
        if not expect_path or not stripped.startswith("path "):
            continue

        raw_path = stripped.split(" (offset", 1)[0].removeprefix("path ").strip()
        expect_path = False
        if raw_path.startswith("@loader_path/"):
            rpaths.append((path.parent / raw_path.removeprefix("@loader_path/")).resolve())
        elif raw_path.startswith("@executable_path/"):
            rpaths.append((path.parent / raw_path.removeprefix("@executable_path/")).resolve())
        elif raw_path.startswith("/"):
            rpaths.append(Path(raw_path).resolve())

    return rpaths


def _macos_local_dylib_reference(path: Path, dylib_name: str, *, lib_dir: Path) -> str:
    if path.parent.resolve() == lib_dir:
        return f"@loader_path/{dylib_name}"
    return f"@loader_path/../lib/{dylib_name}"


def _resolve_checksum_inputs(root: Path, patterns: tuple[str, ...]) -> list[Path]:
    matched: list[Path] = []
    seen: set[Path] = set()
    for pattern in patterns:
        expanded = glob(pattern, root_dir=root, recursive=False)
        if expanded:
            for entry in sorted(expanded):
                candidate = (root / entry).resolve()
                if candidate.is_file() and candidate not in seen:
                    matched.append(candidate)
                    seen.add(candidate)
            continue

        candidate = Path(pattern)
        if not candidate.is_absolute():
            candidate = (root / candidate).resolve()
        if candidate.is_file() and candidate not in seen:
            matched.append(candidate)
            seen.add(candidate)
    return matched
