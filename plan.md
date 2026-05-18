# Kern Security Maintenance Plan

This plan records the verified parts of the second external report. It keeps
the report's speculative severity labels out of the active queue and tracks the
small set of changes that are useful for Kern as a personal, actively maintained
language project.

## Priority 1: Shell API Documentation

`library/std/proc/shell.kn` exposes `shell_capture`, `shell_status`, and
`shell_success` as shell-command helpers. Their implementations intentionally
execute through the host shell:

- Linux: `/bin/sh -lc` in `library/std/host/os/linux.kn`.
- macOS: `popen` in `library/std/proc/shell.kn`.
- Windows: `cmd.exe /d /c` in `library/std/host/os/windows.kn`.

This is dangerous for untrusted input, but it is not a remote vulnerability by
itself: the API is explicitly a shell API, and current in-tree uses are tests or
carefully quoted example commands.

Planned work:

- Add prominent documentation warnings to the public shell helpers.
- State that callers must not pass untrusted input unless they fully understand
  platform shell quoting.
- Plan a future argv-style process API that executes a program with arguments
  without invoking a shell.

## Priority 2: Craft Git Source Hardening

Craft already has release source policy checks for floating git references and
`http://` sources. The report missed this existing policy, but the fetch path
can still be made more explicit.

Planned work:

- Add explicit `-c http.sslVerify=true` to remote git clone/fetch calls.
- Extend insecure transport classification beyond `http://` where appropriate,
  especially `git://`.
- Keep local path git dependencies working.
- Add tests around policy classification and git command construction.

## Priority 3: Security Policy

Add a lightweight `SECURITY.md` so vulnerability reports have a private path and
do not default to public issues.

Planned work:

- Keep commitments realistic for a personal project.
- Support current `main` and the latest release only.
- Ask reporters to avoid public disclosure until a fix is available.
- Prefer GitHub private vulnerability reporting when available.

## Backlog

- Review deterministic compiler temporary object paths in
  `compiler/kernc_driver/src/compiler/link.rs` and decide whether unique temp
  names are worth the complexity.
- Consider configurable LSP resource limits for open documents and cached
  analysis state.
- Treat broad `unwrap` and `panic!` cleanup as ICE quality work driven by real
  crashes, not as a blanket security task.

## Not Planned

- Build tool sandboxing. Build scripts and tools execute with user privileges
  by design, like other mainstream build systems.
- Error path stripping. Absolute paths are useful compiler and build diagnostics
  and are not currently worth a global redaction mode.
- SDK code signing. Checksums already exist; release signing is a long-term
  distribution infrastructure topic.
