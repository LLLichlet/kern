# limine-smoke

`limine-smoke` is a minimal freestanding Kern package that exercises the new
`craft` resource pipeline against a real Limine binary release tag.

The resource is currently pinned to Limine `v11.4.0-binary`.

Current scope:

- a freestanding `_start` kernel that prints one boot line and then halts forever
- a package-local linker script wired through `build.kn`
- a staged `iso-root/` tree built entirely from ordinary copy primitives
- a `build-dependency` host tool that turns that tree into `limine-smoke.iso`
- a minimal debugcon/serial message so QEMU boot verification has a concrete success signal
- the linked kernel still exists as the primary craft binary output as well

Fetch and build it from the repository root:

```sh
cargo run -q -p craft -- fetch --project-path examples/limine-smoke
cargo run -q -p craft -- build --project-path examples/limine-smoke
```

Build outputs currently land in three places:

- primary binary:
  - `.craft/build/dev/target/out/limine-smoke-0.1.0/bin/kernel`
- staged ISO root:
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/iso-root/kernel`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/iso-root/boot/limine.conf`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/iso-root/boot/limine-bios.sys`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/iso-root/boot/limine-bios-cd.bin`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/iso-root/boot/limine-uefi-cd.bin`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/iso-root/EFI/BOOT/BOOTX64.EFI`
- final ISO:
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/limine-smoke.iso`

`craft fetch` materializes the pinned Limine resource into `.craft/resources/` and the
build script reshapes the flat upstream binary layout into `iso-root/` using
ordinary file-copy primitives. A separate host tool package then runs
`xorriso` plus `limine bios-install` to emit the final ISO.

BIOS boot verification from the repository root:

```sh
timeout 10s qemu-system-x86_64 \
  -nographic \
  -no-reboot \
  -no-shutdown \
  -cdrom examples/limine-smoke/.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/limine-smoke.iso
```

This now shows Limine's verbose boot log and the kernel success line:

```text
limine-smoke: kernel booted
```

If you want a minimal kernel-only signal in QEMU, use the debugcon path:

```sh
timeout 10s qemu-system-x86_64 \
  -display none \
  -serial none \
  -debugcon stdio \
  -global isa-debugcon.iobase=0xe9 \
  -no-reboot \
  -no-shutdown \
  -cdrom examples/limine-smoke/.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/limine-smoke.iso
```

What this proves today:

- `craft` can fetch/materialize real non-package resources
- `build.kn` can compose a bootable directory tree and a final ISO from small orthogonal primitives
- `build-dependencies` can expose explicit host tools for post-link packaging without turning `build.kn` into a shell escape hatch
- a freestanding kernel package can consume that flow without shell hooks or
  hidden pre-build side effects
- `link_arg_path(...)` edits really invalidate the link step
- staged `depend(...)` edges really invalidate downstream post-link outputs such as the final ISO

What it does not prove yet:

- HDD image generation
- first-class built-in image-packaging commands in `craft`
