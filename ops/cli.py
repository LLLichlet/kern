from __future__ import annotations

import argparse

from .common import OpsError
from .ci import (
    KerncTestsArgs,
    PackagedToolchainInstallArgs,
    PackagedToolchainVerifyArgs,
    ToolchainArchiveVerifyArgs,
    ToolchainSpecArgs,
    assert_toolchain_health,
    install_packaged_toolchain_archive,
    print_toolchain_info,
    print_toolchain_spec,
    run_craft_policy_checks,
    run_kernc_tests,
    verify_packaged_toolchain_archive,
    verify_toolchain_archive,
)
from .install import InstallReleaseArgs, install_release
from .release import (
    ReleasePackageArgs,
    ReleaseChecksumsArgs,
    ReleaseToolchainPackageArgs,
    package_release,
    package_toolchain_release,
    write_release_checksums,
)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m ops",
        description="Cross-platform repository operations tooling.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    ci = subparsers.add_parser("ci", help="CI-oriented repository checks")
    ci_subparsers = ci.add_subparsers(dest="ci_command", required=True)

    kernc_tests = ci_subparsers.add_parser("kernc-tests", help="Run grouped kernc integration tests")
    kernc_tests.add_argument(
        "--mode",
        default="all",
        choices=("smoke", "hosted", "all"),
        help="test group selection",
    )
    ci_subparsers.add_parser("craft-policy", help="Run craft release policy fixtures")
    ci_subparsers.add_parser("toolchain-info", help="Print CI toolchain diagnostics")
    ci_subparsers.add_parser("toolchain-health", help="Fail if the current host toolchain is incomplete")
    toolchain_spec = ci_subparsers.add_parser(
        "toolchain-spec",
        help="Print the checked-in CI toolchain policy for a runner OS",
    )
    toolchain_spec.add_argument("--runner-os", required=True, help="Linux, macOS, or Windows")
    toolchain_spec.add_argument(
        "--mode",
        default="current",
        choices=("current", "bootstrap", "target"),
        help="policy mode to render",
    )
    toolchain_spec.add_argument("--host-target", help="archive target label when target-specific policy is needed")
    toolchain_spec.add_argument(
        "--format",
        default="text",
        choices=("text", "github-env"),
        help="render format",
    )
    toolchain_archive = ci_subparsers.add_parser(
        "verify-toolchain-archive",
        help="Verify an archive-based CI toolchain artifact against the checked-in policy",
    )
    toolchain_archive.add_argument("--runner-os", required=True, help="Linux, macOS, or Windows")
    toolchain_archive.add_argument(
        "--mode",
        default="current",
        choices=("current", "bootstrap", "target"),
        help="policy mode to validate against",
    )
    toolchain_archive.add_argument("--host-target", help="archive target label when target-specific policy is needed")
    toolchain_archive.add_argument("--archive-path", required=True, help="downloaded archive path")
    packaged_toolchain = ci_subparsers.add_parser(
        "verify-packaged-toolchain",
        help="Verify a packaged Kern toolchain archive layout and manifest",
    )
    packaged_toolchain.add_argument("--archive-path", required=True, help="packaged toolchain archive path")
    packaged_toolchain.add_argument(
        "--target",
        help="expected host target; defaults to the current host target",
    )
    install_packaged_toolchain = ci_subparsers.add_parser(
        "install-packaged-toolchain",
        help="Extract and validate a packaged Kern toolchain archive for local CI use",
    )
    install_packaged_toolchain.add_argument(
        "--archive-path",
        required=True,
        help="packaged toolchain archive path",
    )
    install_packaged_toolchain.add_argument(
        "--dest",
        required=True,
        help="destination directory for the extracted toolchain root",
    )
    install_packaged_toolchain.add_argument(
        "--target",
        help="expected host target; defaults to the current host target",
    )
    install_packaged_toolchain.add_argument(
        "--format",
        default="text",
        choices=("text", "github-env"),
        help="render format",
    )

    install = subparsers.add_parser("install", help="Install a released Kern SDK")
    install.add_argument("--version", help="release tag; defaults to latest GitHub release")
    install.add_argument("--target", help="host target label; defaults to the current host target")
    install.add_argument("--archive", help="install from a local SDK archive instead of GitHub")
    install.add_argument("--dest", help="installation directory; defaults to ~/.kern or %%USERPROFILE%%\\.kern")
    install.add_argument("--github-repo", default="kern-project/kern", help="GitHub repository for release downloads")
    install.add_argument("--no-path", action="store_true", help="do not mutate user PATH configuration")

    release = subparsers.add_parser("release", help="Release engineering commands")
    release_subparsers = release.add_subparsers(dest="release_command", required=True)

    package = release_subparsers.add_parser("package", help="Build and package a host-native SDK")
    package.add_argument("--version", help="archive version label; defaults to workspace version with `v` prefix")
    package.add_argument("--target", help="archive target label; defaults to the current host target")
    package.add_argument(
        "--skip-build",
        action="store_true",
        help="reuse existing release binaries instead of rebuilding",
    )
    package.add_argument(
        "--toolchain-prefix",
        help="LLVM toolchain prefix to bundle; defaults to LLVM_SYS_*_PREFIX or llvm-config",
    )

    package_toolchain = release_subparsers.add_parser(
        "package-toolchain",
        help="Package the controlled host LLVM toolchain as a standalone artifact",
    )
    package_toolchain.add_argument(
        "--version",
        help="artifact version label; defaults to llvm-<resolved toolchain version>",
    )
    package_toolchain.add_argument(
        "--target",
        help="archive target label; defaults to the current host target",
    )
    package_toolchain.add_argument(
        "--toolchain-prefix",
        help="LLVM toolchain prefix to package; defaults to LLVM_SYS_*_PREFIX or llvm-config",
    )
    checksums = release_subparsers.add_parser(
        "write-checksums",
        help="Generate sha256 sidecars and an optional manifest for release artifacts",
    )
    checksums.add_argument("paths", nargs="+", help="artifact paths or glob patterns relative to the repo root")
    checksums.add_argument("--manifest-path", help="optional manifest JSON output path")
    checksums.add_argument("--channel", default="release", help="logical artifact channel label")
    checksums.add_argument("--release-tag", help="release tag recorded in the manifest")

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    try:
        if args.command == "ci" and args.ci_command == "kernc-tests":
            return run_kernc_tests(KerncTestsArgs(mode=args.mode))
        if args.command == "ci" and args.ci_command == "craft-policy":
            return run_craft_policy_checks()
        if args.command == "ci" and args.ci_command == "toolchain-info":
            return print_toolchain_info()
        if args.command == "ci" and args.ci_command == "toolchain-health":
            return assert_toolchain_health()
        if args.command == "ci" and args.ci_command == "toolchain-spec":
            return print_toolchain_spec(
                ToolchainSpecArgs(
                    runner_os=args.runner_os,
                    mode=args.mode,
                    host_target=args.host_target,
                    format=args.format,
                )
            )
        if args.command == "ci" and args.ci_command == "verify-toolchain-archive":
            return verify_toolchain_archive(
                ToolchainArchiveVerifyArgs(
                    runner_os=args.runner_os,
                    mode=args.mode,
                    host_target=args.host_target,
                    archive_path=args.archive_path,
                )
            )
        if args.command == "ci" and args.ci_command == "verify-packaged-toolchain":
            return verify_packaged_toolchain_archive(
                PackagedToolchainVerifyArgs(
                    archive_path=args.archive_path,
                    target=args.target,
                )
            )
        if args.command == "ci" and args.ci_command == "install-packaged-toolchain":
            return install_packaged_toolchain_archive(
                PackagedToolchainInstallArgs(
                    archive_path=args.archive_path,
                    dest=args.dest,
                    target=args.target,
                    format=args.format,
                )
            )
        if args.command == "install":
            return install_release(
                InstallReleaseArgs(
                    version=args.version,
                    target=args.target,
                    archive=args.archive,
                    dest=args.dest,
                    github_repo=args.github_repo,
                    no_path=args.no_path,
                )
            )
        if args.command == "release" and args.release_command == "package":
            return package_release(
                ReleasePackageArgs(
                    version=args.version,
                    target=args.target,
                    skip_build=args.skip_build,
                    toolchain_prefix=args.toolchain_prefix,
                )
            )
        if args.command == "release" and args.release_command == "package-toolchain":
            return package_toolchain_release(
                ReleaseToolchainPackageArgs(
                    version=args.version,
                    target=args.target,
                    toolchain_prefix=args.toolchain_prefix,
                )
            )
        if args.command == "release" and args.release_command == "write-checksums":
            return write_release_checksums(
                ReleaseChecksumsArgs(
                    paths=tuple(args.paths),
                    manifest_path=args.manifest_path,
                    channel=args.channel,
                    release_tag=args.release_tag,
                )
            )
    except OpsError as err:
        parser.exit(1, f"Error: {err}\n")

    parser.exit(2, "Error: unsupported command\n")
