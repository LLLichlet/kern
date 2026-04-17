from __future__ import annotations

import os
import shutil
import tarfile
import zipfile
from glob import glob
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .common import (
    ArtifactRecord,
    HOST_TOOL_BINARIES,
    OFFICIAL_LIBRARY_LAYERS,
    BundledToolchain,
    HostTarget,
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

    for text_file in ("README.md", "LICENSE"):
        shutil.copy2(root / text_file, dist_dir / text_file)

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


def _copy_file(source: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, dest)


def _copy_selected_files(source: Path, dest: Path, *, predicate: Callable[[Path], bool]) -> None:
    for path in sorted(source.rglob("*")):
        if path.is_dir():
            continue
        relative = path.relative_to(source)
        if predicate(relative):
            target = dest / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)


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

    canonical_names = canonical_toolchain_component_names(host.archive_target)

    for component, source in bundled_toolchain.tools.items():
        target_name = canonical_names.get(component, source.name)
        target = bin_dir / target_name
        _copy_file(source, target)
        records[component] = ArtifactRecord(
            path=target.relative_to(dist_dir).as_posix(),
            kind="file",
            sha256=sha256_file(target),
            size=file_size(target),
        )

    if host.is_windows:
        for dll in sorted(bundled_toolchain.bindir.glob("*.dll")):
            _copy_file(dll, bin_dir / dll.name)

    def include_lib(relative: Path) -> bool:
        if not relative.parts:
            return False
        if relative.parts[0] in {"cmake", "pkgconfig"}:
            return False
        name = relative.name
        return (
            ".so." in name
            or name.endswith((".so", ".dylib", ".dll", ".lib", ".def", ".json"))
            or name == "LLVMgold.so"
        )

    _copy_selected_files(bundled_toolchain.libdir, lib_dir, predicate=include_lib)

    if bundled_toolchain.resource_dir is not None and bundled_toolchain.resource_dir.exists():
        resource_dest = lib_dir / "clang" / bundled_toolchain.resource_dir.name
        copy_directory_contents(bundled_toolchain.resource_dir, resource_dest)
        records["clang_resource_dir"] = ArtifactRecord(
            path=resource_dest.relative_to(dist_dir).as_posix(),
            kind="directory",
            sha256=sha256_directory(resource_dest),
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
                f"- Bundled executables: {', '.join(sorted(canonical_names.values()))}",
                "",
                "The SDK keeps user installs pointed at this bundled toolchain first.",
                "Host OS SDK/libc pieces may still remain platform responsibilities.",
                "",
            ]
        ),
        encoding="utf-8",
    )
    return records


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
