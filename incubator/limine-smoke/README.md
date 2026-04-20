# limine-smoke

`limine-smoke` is a minimal freestanding Kern package that exercises the new
`craft` resource pipeline with a Limine-shaped boot tree.

Current scope:

- a freestanding `_start` kernel that just spins forever
- a package-local linker script wired through `build.rn`
- a staged `boot/limine.conf`
- a staged Limine-like resource tree copied from `[resources]`
- no ISO/disk-image packing step yet
- the linked kernel binary and the post-link staged tree still live in separate
  craft outputs today

Build it from the repository root:

```sh
cargo run -q -p craft -- build --project-path incubator/limine-smoke
```

Build outputs currently land in two places:

- primary binary:
  - `.craft/build/dev/target/out/limine-smoke-0.1.0/bin/kernel`
- post-link staged tree:
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/boot/limine.conf`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/boot/limine-bios.sys`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/boot/limine-bios-cd.bin`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/boot/limine-uefi-cd.bin`
  - `.craft/build/dev/target/stage/limine-smoke-0.1.0/bin/kernel/EFI/BOOT/BOOTX64.EFI`

This package intentionally uses a local stub resource under `vendor/limine/` so
the flow can be tested offline.

To switch to the real Limine binary branch, replace the resource entry in
`Craft.toml`:

```toml
[resources]
limine = { git = "https://github.com/limine-bootloader/limine.git", branch = "0.9.x-binary" }
```

Then remove or ignore `vendor/limine/` and run:

```sh
cargo run -q -p craft -- fetch --project-path incubator/limine-smoke
cargo run -q -p craft -- build --project-path incubator/limine-smoke
```

What this proves today:

- `craft` can fetch/materialize non-package resources
- `build.rn` can resolve resource paths and stage resource directories into the
  final artifact tree
- a freestanding kernel package can consume that flow without shell hooks or
  hidden pre-build side effects

What it does not prove yet:

- assembling the linked kernel and staged boot tree into one final image root
- producing an ISO, HDD image, or installable EFI layout in one built-in step
