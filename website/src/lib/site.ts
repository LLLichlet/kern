export const siteMeta = {
  title: "Kern",
  shortTitle: "Kern",
  description:
    "A systems programming language for kernels, firmware, and performance-critical infrastructure.",
  repoUrl: "https://github.com/softfault/kern"
};

function normalizeBase(base: string) {
  if (!base || base === "/") {
    return "/";
  }

  const withLeading = base.startsWith("/") ? base : `/${base}`;
  return withLeading.endsWith("/") ? withLeading : `${withLeading}/`;
}

export function internalHref(path: string) {
  if (!path || path === "/") {
    return normalizeBase(import.meta.env.BASE_URL);
  }

  const normalizedPath = path.startsWith("/") ? path.slice(1) : path;
  return `${normalizeBase(import.meta.env.BASE_URL)}${normalizedPath}`;
}

export const navItems = [
  { href: internalHref("/install"), label: "Install" },
  { href: internalHref("/guide"), label: "Guide" },
  { href: internalHref("/reference"), label: "Reference" },
  { href: internalHref("/docs"), label: "Docs" }
];

export const authoritativeDocs = [
  {
    title: "Language Design",
    description: "Current language semantics, syntax, intrinsics, and type-system rules.",
    href: `${siteMeta.repoUrl}/blob/main/docs/design.md`
  },
  {
    title: "Runtime Architecture",
    description: "The `base` / `sys` / `rt` / `std` split and runtime policy.",
    href: `${siteMeta.repoUrl}/blob/main/docs/runtime-architecture.md`
  },
  {
    title: "kernc Guide",
    description: "Compiler driver modes, runtime/library flags, linking, and build integration.",
    href: `${siteMeta.repoUrl}/blob/main/docs/kernc.md`
  },
  {
    title: "craft Guide",
    description: "Package resolution, lockfiles, build planning, and execution model.",
    href: `${siteMeta.repoUrl}/blob/main/docs/craft.md`
  },
  {
    title: "Documentation Map",
    description: "Which repository documents are public references versus implementation notes.",
    href: `${siteMeta.repoUrl}/blob/main/docs/documentation-map.md`
  }
];

export const referenceTracks = [
  {
    title: "Language",
    description:
      "Types, control flow, traits, modules, intrinsics, inline assembly, and compile-time behavior.",
    href: internalHref("/reference/language"),
    sourceHref: `${siteMeta.repoUrl}/blob/main/docs/design.md`
  },
  {
    title: "Runtime And Libraries",
    description:
      "The `base` / `sys` / `rt` / `std` model, freestanding versus hosted, and libc policy.",
    href: internalHref("/reference/runtime"),
    sourceHref: `${siteMeta.repoUrl}/blob/main/docs/runtime-architecture.md`
  },
  {
    title: "Compiler Driver",
    description:
      "Driver modes, runtime/library flags, LLVM emission, linking, CGUs, and LTO behavior.",
    href: internalHref("/reference/kernc"),
    sourceHref: `${siteMeta.repoUrl}/blob/main/docs/kernc.md`
  },
  {
    title: "Package Manager",
    description:
      "Workspaces, `Craft.lock`, dependency resolution, build plans, and execution.",
    href: internalHref("/reference/craft"),
    sourceHref: `${siteMeta.repoUrl}/blob/main/docs/craft.md`
  }
];

export const toolingProducts = [
  {
    name: "kernc",
    summary:
      "The compiler and linker driver. It owns compilation of one explicit source entry or explicit link-only actions.",
    href: internalHref("/reference/kernc")
  },
  {
    name: "craft",
    summary:
      "The package manager and build orchestrator. It owns workspaces, lockfiles, dependency resolution, and action graphs.",
    href: internalHref("/reference/craft")
  },
  {
    name: "kern-lsp",
    summary:
      "The language server. It reuses compiler analysis rather than implementing a separate frontend.",
    href: internalHref("/tooling/editor-setup")
  },
  {
    name: "VS Code Extension",
    summary:
      "The first-party editor integration that launches `kern-lsp` and packages the language assets.",
    href: internalHref("/tooling/editor-setup")
  }
];

export const architectureLayers = [
  {
    title: "Frontend",
    body:
      "Lexer, parser, AST, semantic analysis, and staged structure/type resolution establish the checked program model."
  },
  {
    title: "Flow",
    body:
      "A source-near analysis layer for CFG/dataflow, warnings, reachability, and conservative lowering hints."
  },
  {
    title: "MAST",
    body:
      "The monomorphized lowering IR that owns emitted items, closure/vtable lowering, and backend-friendly body shapes."
  },
  {
    title: "MIR",
    body:
      "The transform-oriented mid-level IR for explicit basic blocks, verification, and compiler-owned optimization passes."
  },
  {
    title: "LLVM And Linking",
    body:
      "Backend lowering emits LLVM IR, native linker inputs, and LTO artifacts before the final system link step."
  }
];
