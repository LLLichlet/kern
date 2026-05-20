import "./styles.css";
import { docs, type DocPage, type DocPageMeta } from "./generated/docs";

type Route = { kind: "home" } | { kind: "docs"; slug: string; hash?: string };
type SearchEntry = {
  slug: string;
  title: string;
  sourcePath: string;
  summary: string;
  searchText: string;
};

const installCommands = {
  unix: {
    label: "Linux/macOS",
    command: "curl -sSf https://raw.githubusercontent.com/kern-project/kern/main/install.sh | bash"
  },
  windows: {
    label: "Windows",
    command:
      'powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/kern-project/kern/main/install.ps1 -UseBasicParsing).Content"'
  },
  cargo: {
    label: "Cargo",
    command: "cargo install kernup\nkernup install"
  }
} as const;

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("missing #app root");
}
const root = app;
let renderGeneration = 0;
let searchIndexPromise: Promise<readonly SearchEntry[]> | null = null;

applyStoredTheme();
window.addEventListener("hashchange", render);
window.addEventListener("DOMContentLoaded", render);
render();

function render() {
  void renderCurrentRoute();
}

async function renderCurrentRoute() {
  const generation = ++renderGeneration;
  const route = currentRoute();
  document.documentElement.lang =
    route.kind === "docs" ? docMetaBySlug(route.slug)?.language ?? "en" : "en";

  if (route.kind === "home") {
    document.title = "Kern";
    root.innerHTML = homeTemplate();
    wireHome();
    return;
  }

  const page = await loadDocPage(route.slug);
  if (generation !== renderGeneration) {
    return;
  }
  root.innerHTML = docsTemplate(page);
  wireDocs(page);
  if (route.hash) {
    requestAnimationFrame(() => document.getElementById(route.hash ?? "")?.scrollIntoView());
  }
}

function currentRoute(): Route {
  const hash = window.location.hash.replace(/^#\/?/, "");
  if (!hash) {
    return { kind: "home" };
  }
  const [pathPart, anchor] = hash.split("#");
  const parts = pathPart.split("/");
  if (parts[0] === "docs") {
    return { kind: "docs", slug: parts[1] || docs[0].slug, hash: anchor };
  }
  return { kind: "home" };
}

function homeTemplate() {
  const overview = docMetaBySlug("index");
  const zhOverview = docMetaBySlug("readme-zh");
  const install = docMetaBySlug("install");
  const design = docMetaBySlug("design");
  const craft = docMetaBySlug("craft");
  const tutorial = docMetaBySlug("tutorial--en--01-quick-start") ?? docMetaBySlug("tutorial--index");

  return `
    <header class="site-header">
      <a class="brand" href="#/" aria-label="Kern home">
        <img src="/brand/kern-logo.svg" alt="Kern" />
      </a>
      <nav class="top-nav" aria-label="Primary">
        <a href="#/docs/${install?.slug ?? "install"}">Install</a>
        <a href="#/docs/${tutorial?.slug ?? "tutorial--index"}">Tutorial</a>
        <a href="#/docs/${design?.slug ?? "design"}">Design</a>
        <a href="https://github.com/kern-project/kern">GitHub</a>
      </nav>
      ${themeToggleTemplate()}
    </header>

    <main>
      <section class="hero">
        <div class="hero-copy">
          <h1>Kern</h1>
          <p class="eyebrow">Systems programming without hidden runtime policy</p>
          <p class="hero-text">
            A programming language for kernels, firmware, and freestanding software,
            with explicit modules, traits, generics, pattern matching, and a package
            tool built around low-level workflows.
          </p>
          <div class="hero-actions">
            <a class="button primary" href="#/docs/${install?.slug ?? "install"}">Install Kern</a>
            <a class="button" href="#/docs/${overview?.slug ?? "index"}">Read Overview</a>
            <a class="button ghost" href="#/docs/${zhOverview?.slug ?? "readme-zh"}">中文文档</a>
          </div>
        </div>
        <div class="install-panel" aria-label="Kern install commands">
          <div class="install-tabs" role="tablist" aria-label="Install platform">
            ${Object.entries(installCommands)
              .map(
                ([key, value], index) =>
                  `<button type="button" class="${index === 0 ? "active" : ""}" data-install-platform="${key}">${value.label}</button>`
              )
              .join("")}
          </div>
          <div class="terminal">
            <div class="terminal-bar">
              <span id="install-label">${installCommands.unix.label}</span>
              <button class="copy-button" data-install-copy data-copy="${escapeHtml(installCommands.unix.command)}">Copy</button>
            </div>
            <pre><code id="install-code">${formatShellCommand(installCommands.unix.command)}</code></pre>
          </div>
          <div class="install-next">
            <span>then</span>
            <code>craft init</code>
            <code>craft run</code>
          </div>
        </div>
      </section>

      <section class="code-band">
        <div class="section-heading">
          <p class="eyebrow">A small taste</p>
          <h2>Low-level code with modern structure.</h2>
        </div>
        <pre class="sample-code"><code>use std.io;

enum ParseResult {
    Number: i32,
    Missing,
};

fn describe(result: ParseResult) void {
    match (result) {
        .{ Number: value } => "number = {}".fmt(.{value}).println(),
        .Missing => "missing".println(),
    }
}</code></pre>
      </section>

      <section class="feature-grid" aria-label="Kern features">
        ${featureCard("Explicit runtime boundaries", "No garbage collector, no exceptions, no implicit allocation, and no compiler-mandated hosted runtime.")}
        ${featureCard("Traits and generics", "Advanced type-level behavior without hiding the generated low-level shape from compiler analysis.")}
        ${featureCard("Freestanding first", "The library stack separates base, runtime, and hosted std layers so kernels can choose what exists.")}
        ${featureCard("Craft-native packages", "Builds, lockfiles, generated files, examples, and tests are coordinated by the first-party craft tool.")}
      </section>

      <section class="doc-band">
        <div class="section-heading">
          <p class="eyebrow">Documentation</p>
          <h2>Start with the actual manuals.</h2>
        </div>
        <div class="doc-cards">
          ${docCard(install)}
          ${docCard(tutorial)}
          ${docCard(design)}
          ${docCard(craft)}
        </div>
      </section>
    </main>
  `;
}

function docsTemplate(page: DocPage) {
  const grouped = groupDocs();
  const tocHeadings = page.headings.filter((heading) => heading.depth > 1 && heading.depth < 4);
  const toc = tocHeadings.slice(0, 18).map((heading, index) => tocLinkTemplate(page, heading, index)).join("");

  return `
    <header class="site-header docs-header">
      <a class="brand" href="#/" aria-label="Kern home">
        <img src="/brand/kern-logo.svg" alt="Kern" />
      </a>
      <nav class="top-nav" aria-label="Primary">
        <a href="#/">Home</a>
        <a href="#/docs/install">Install</a>
        <a href="#/docs/tutorial--en--01-quick-start">Tutorial</a>
        <a href="https://github.com/kern-project/kern">GitHub</a>
      </nav>
      ${themeToggleTemplate()}
    </header>

    <main class="docs-shell">
      <aside class="docs-sidebar" aria-label="Documentation">
        <label class="search-label">
          <span>Search docs</span>
          <input id="doc-search" type="search" placeholder="trait, craft, install" />
        </label>
        <div id="search-results" class="search-results" hidden></div>
        <nav id="doc-nav">
          ${Array.from(grouped.entries())
            .map(([section, pages]) => docsGroupTemplate(section, pages, page.slug))
            .join("")}
        </nav>
      </aside>

      <article class="doc-article">
        <div class="doc-meta">
          <span>${escapeHtml(page.section)}</span>
          <a href="https://github.com/kern-project/kern/blob/main/${page.sourcePath}">${escapeHtml(page.sourcePath)}</a>
        </div>
        <div class="markdown-body">${page.html}</div>
      </article>

      <aside class="toc" aria-label="On this page">
        <div class="toc-card">
          <span class="toc-kicker">${escapeHtml(page.section)}</span>
          <strong>${escapeHtml(page.title)}</strong>
          <small>${tocHeadings.length} section${tocHeadings.length === 1 ? "" : "s"}</small>
          <div class="toc-list">
            ${toc || "<span>No section headings</span>"}
          </div>
        </div>
      </aside>
    </main>
  `;
}

function docsGroupTemplate(section: string, pages: readonly DocPageMeta[], activeSlug: string) {
  return `
    <section class="docs-group">
      <h2>${escapeHtml(section)}</h2>
      <div class="docs-branch">
      ${pages
        .map(
          (page) => `
          <a class="${docLinkClass(page, activeSlug)}" data-doc-link data-title="${escapeHtml(
            `${page.title} ${page.sourcePath}`
          )}" href="#/docs/${page.slug}">
            <span class="branch-mark" aria-hidden="true"></span>
            <span>${escapeHtml(page.title)}</span>
            <small>${escapeHtml(shortPath(page.sourcePath))}</small>
          </a>
        `
        )
        .join("")}
      </div>
    </section>
  `;
}

function wireHome() {
  wireThemeToggle();
  wireInstallTabs();
  document.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((button) => {
    button.addEventListener("click", () => copyText(button.dataset.copy ?? "", button));
  });
}

function wireInstallTabs() {
  const code = document.querySelector<HTMLElement>("#install-code");
  const label = document.querySelector<HTMLElement>("#install-label");
  const copy = document.querySelector<HTMLButtonElement>("[data-install-copy]");
  document.querySelectorAll<HTMLButtonElement>("[data-install-platform]").forEach((button) => {
    button.addEventListener("click", () => {
      const platform = button.dataset.installPlatform as keyof typeof installCommands;
      const install = installCommands[platform];
      code!.innerHTML = formatShellCommand(install.command);
      label!.textContent = install.label;
      copy!.dataset.copy = install.command;
      document.querySelectorAll<HTMLButtonElement>("[data-install-platform]").forEach((choice) => {
        choice.classList.toggle("active", choice === button);
      });
    });
  });
}

function wireDocs(page: DocPage) {
  wireThemeToggle();
  document.querySelectorAll<HTMLPreElement>(".markdown-body pre").forEach((pre) => {
    const button = document.createElement("button");
    button.className = "copy-code";
    button.type = "button";
    button.textContent = "Copy";
    button.addEventListener("click", () => copyText(pre.innerText, button));
    pre.append(button);
  });

  const search = document.querySelector<HTMLInputElement>("#doc-search");
  const searchResults = document.querySelector<HTMLDivElement>("#search-results");
  search?.addEventListener("input", () => {
    const query = search.value.trim().toLowerCase();
    void renderSearchResults(query, searchResults);
  });

  document.title = `${page.title} - Kern`;
}

async function renderSearchResults(query: string, container: HTMLDivElement | null) {
  if (!container) {
    return;
  }
  if (!query) {
    container.hidden = true;
    container.innerHTML = "";
    return;
  }

  const searchIndex = await loadSearchIndex();
  const results = searchIndex
    .map((page) => ({ page, score: searchScore(page, query) }))
    .filter((result) => result.score > 0)
    .sort((left, right) => right.score - left.score)
    .slice(0, 8);

  container.hidden = false;
  container.innerHTML = `
    <div class="search-results-head">
      <span>${results.length} result${results.length === 1 ? "" : "s"}</span>
      <small>${escapeHtml(query)}</small>
    </div>
    ${
      results.length
        ? results.map(({ page }) => searchResultTemplate(page, query)).join("")
        : `<p class="search-empty">No matching docs.</p>`
    }
  `;
}

function searchScore(page: SearchEntry, query: string) {
  const haystack = page.searchText;
  if (!haystack.includes(query)) {
    return 0;
  }
  let score = 1;
  if (page.title.toLowerCase().includes(query)) {
    score += 8;
  }
  if (page.sourcePath.toLowerCase().includes(query)) {
    score += 4;
  }
  return score;
}

function searchResultTemplate(page: SearchEntry, query: string) {
  return `
    <a class="search-hit" href="#/docs/${page.slug}">
      <span>${highlightMatch(page.title, query)}</span>
      <small>${escapeHtml(shortPath(page.sourcePath))}</small>
      <p>${highlightMatch(page.summary, query)}</p>
    </a>
  `;
}

function loadSearchIndex() {
  searchIndexPromise ??= fetch("/docs-data/search-index.json").then((response) => {
    if (!response.ok) {
      throw new Error("failed to load documentation search index");
    }
    return response.json() as Promise<readonly SearchEntry[]>;
  });
  return searchIndexPromise;
}

function themeToggleTemplate() {
  const current = currentTheme();
  return `
    <div class="theme-toggle" role="group" aria-label="Color theme">
      <button type="button" data-theme-choice="light" class="${current === "light" ? "active" : ""}">Light</button>
      <button type="button" data-theme-choice="dark" class="${current === "dark" ? "active" : ""}">Dark</button>
    </div>
  `;
}

function wireThemeToggle() {
  document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]").forEach((button) => {
    button.addEventListener("click", () => {
      const theme = button.dataset.themeChoice === "dark" ? "dark" : "light";
      setTheme(theme);
      document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]").forEach((choice) => {
        choice.classList.toggle("active", choice.dataset.themeChoice === theme);
      });
    });
  });
}

function applyStoredTheme() {
  setTheme(currentTheme());
}

function currentTheme() {
  return localStorage.getItem("kern-theme") === "dark" ? "dark" : "light";
}

function setTheme(theme: "light" | "dark") {
  localStorage.setItem("kern-theme", theme);
  document.documentElement.dataset.theme = theme;
}

async function copyText(text: string, button: HTMLButtonElement) {
  await navigator.clipboard.writeText(text.trim());
  const previous = button.textContent;
  button.textContent = "Copied";
  window.setTimeout(() => {
    button.textContent = previous;
  }, 1300);
}

function groupDocs() {
  const groups = new Map<string, readonly DocPageMeta[]>();
  for (const section of ["Overview", "Install", "Tutorial", "Tutorial 中文", "Reference"]) {
    groups.set(
      section,
      docs.filter((page) => page.section === section)
    );
  }
  return groups;
}

function docMetaBySlug(slug: string) {
  return docs.find((page) => page.slug === slug);
}

async function loadDocPage(slug: string): Promise<DocPage> {
  const meta = docMetaBySlug(slug) ?? docs[0];
  const response = await fetch(`/docs-data/${meta.slug}.json`);
  if (!response.ok) {
    throw new Error(`failed to load documentation page ${meta.slug}`);
  }
  const data = (await response.json()) as { html: string };
  return { ...meta, html: data.html };
}

function featureCard(title: string, body: string) {
  return `
    <article class="feature-card">
      <h3>${escapeHtml(title)}</h3>
      <p>${escapeHtml(body)}</p>
    </article>
  `;
}

function docLinkClass(page: DocPageMeta, activeSlug: string) {
  const classes = ["doc-link"];
  if (page.slug === activeSlug) {
    classes.push("active");
  }
  if (
    (page.sourcePath.includes("/en/") || page.sourcePath.includes("/zh/")) &&
    !page.sourcePath.endsWith("/README.md")
  ) {
    classes.push("nested");
  }
  if (/\/\d\d-/.test(page.sourcePath)) {
    classes.push("chapter");
  }
  return classes.join(" ");
}

function tocLinkTemplate(page: DocPage, heading: DocPage["headings"][number], index: number) {
  const number = String(index + 1).padStart(2, "0");
  return `
    <a class="toc-depth-${heading.depth}" href="#/docs/${page.slug}#${heading.slug}">
      <span>${number}</span>
      <strong>${escapeHtml(heading.text)}</strong>
    </a>
  `;
}

function terminalLine(command: string, rest: string) {
  return `<span class="shell-prompt">$</span> <span class="shell-command">${escapeHtml(command)}</span> <span class="shell-args">${escapeHtml(rest)}</span>`;
}

function formatShellCommand(command: string) {
  return command
    .split("\n")
    .map((line) => {
      const [commandName = "", ...rest] = line.split(/\s+/);
      return terminalLine(commandName, rest.join(" "));
    })
    .join("\n");
}

function docCard(page: DocPageMeta | undefined) {
  if (!page) {
    return "";
  }
  return `
    <a class="doc-card" href="#/docs/${page.slug}">
      <span>${escapeHtml(page.section)}</span>
      <strong>${escapeHtml(page.title)}</strong>
      <small>${escapeHtml(page.sourcePath)}</small>
    </a>
  `;
}

function shortPath(sourcePath: string) {
  return sourcePath
    .replace(/^docs\//, "")
    .replace(/^tutorial\/en\//, "tutorial/")
    .replace(/^tutorial\/zh\//, "教程/");
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function highlightMatch(value: string, query: string) {
  const index = value.toLowerCase().indexOf(query.toLowerCase());
  if (index < 0) {
    return escapeHtml(value);
  }
  return `${escapeHtml(value.slice(0, index))}<mark>${escapeHtml(value.slice(index, index + query.length))}</mark>${escapeHtml(value.slice(index + query.length))}`;
}
