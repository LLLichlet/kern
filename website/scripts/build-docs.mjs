import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import MarkdownIt from "markdown-it";
import anchor from "markdown-it-anchor";
import { createHighlighter } from "shiki";

const websiteRoot = path.resolve(process.cwd());
const repoRoot = path.resolve(websiteRoot, "..");
const generatedPath = path.join(websiteRoot, "src", "generated", "docs.ts");
const docsDataDir = path.join(websiteRoot, "public", "docs-data");
const searchIndexPath = path.join(docsDataDir, "search-index.json");

const publicDocs = [
  "README.md",
  "README.zh.md",
  "docs/documentation-map.md",
  "docs/install.md",
  "docs/nix.md",
  "docs/tutorial/README.md",
  "docs/tutorial/en/01-quick-start.md",
  "docs/tutorial/en/02-language-basics.md",
  "docs/tutorial/en/03-data-and-control-flow.md",
  "docs/tutorial/en/04-memory-slices-and-collections.md",
  "docs/tutorial/en/05-modules-packages-and-libraries.md",
  "docs/tutorial/en/06-freestanding-and-runtime.md",
  "docs/tutorial/en/07-aggregates-and-initialization.md",
  "docs/tutorial/en/08-impl-traits-and-generics.md",
  "docs/tutorial/en/09-closures-and-function-values.md",
  "docs/tutorial/en/10-attributes-intrinsics-and-operators.md",
  "docs/tutorial/en/11-next-steps.md",
  "docs/tutorial/zh/README.md",
  "docs/tutorial/zh/01-快速开始.md",
  "docs/tutorial/zh/02-语言基础.md",
  "docs/tutorial/zh/03-数据与控制流.md",
  "docs/tutorial/zh/04-内存切片与集合.md",
  "docs/tutorial/zh/05-模块包与库分层.md",
  "docs/tutorial/zh/06-底层与freestanding入门.md",
  "docs/tutorial/zh/07-聚合类型与初始化.md",
  "docs/tutorial/zh/08-impl-trait与泛型约束.md",
  "docs/tutorial/zh/09-闭包与函数值.md",
  "docs/tutorial/zh/10-属性intrinsic与低层操作.md",
  "docs/tutorial/zh/11-下一步.md",
  "docs/design.md",
  "docs/kernc.md",
  "docs/craft.md",
  "docs/runtime-architecture.md",
  "docs/style.md",
  "docs/unix-distribution.md",
  "docs/windows-distribution.md"
];

const docPathToSlug = new Map(publicDocs.map((file) => [normalizePath(file), slugForPath(file)]));

const highlighter = await createHighlighter({
  themes: ["github-light", "github-dark"],
  langs: [
    "bash",
    "c",
    "cpp",
    "json",
    "llvm",
    "markdown",
    "nix",
    "powershell",
    "rust",
    "shellscript",
    "text",
    "toml",
    "yaml"
  ]
});

const md = new MarkdownIt({
  html: true,
  linkify: true,
  typographer: false,
  highlight(code, rawLanguage) {
    const lang = normalizeLanguage(rawLanguage);
    return highlighter.codeToHtml(code, {
      lang,
      themes: {
        light: "github-light",
        dark: "github-dark"
      }
    });
  }
}).use(anchor, {
  level: [1, 2, 3, 4],
  slugify: slugifyHeading,
  permalink: anchor.permalink.headerLink()
});

const defaultLinkOpen =
  md.renderer.rules.link_open ??
  ((tokens, idx, options, env, self) => self.renderToken(tokens, idx, options));
const defaultImage =
  md.renderer.rules.image ??
  ((tokens, idx, options, env, self) => self.renderToken(tokens, idx, options));

md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  const token = tokens[idx];
  const hrefIndex = token.attrIndex("href");
  if (hrefIndex >= 0) {
    const href = token.attrs[hrefIndex][1];
    token.attrs[hrefIndex][1] = rewriteHref(href, env.sourcePath);
  }
  return defaultLinkOpen(tokens, idx, options, env, self);
};

md.renderer.rules.image = (tokens, idx, options, env, self) => {
  const token = tokens[idx];
  const srcIndex = token.attrIndex("src");
  if (srcIndex >= 0) {
    const src = token.attrs[srcIndex][1];
    token.attrs[srcIndex][1] = rewriteImageSrc(src, env.sourcePath);
  }
  return defaultImage(tokens, idx, options, env, self);
};

const pages = [];
const searchIndex = [];
await fs.rm(docsDataDir, { recursive: true, force: true });
await fs.mkdir(docsDataDir, { recursive: true });

for (const sourcePath of publicDocs) {
  const absolutePath = path.join(repoRoot, sourcePath);
  const markdown = await fs.readFile(absolutePath, "utf8");
  const headings = collectHeadings(markdown);
  const title = titleForPage(sourcePath, headings);
  const html = md.render(prepareMarkdown(markdown, sourcePath), { sourcePath });
  const slug = slugForPath(sourcePath);
  await fs.writeFile(
    path.join(docsDataDir, `${slug}.json`),
    `${JSON.stringify({ html })}\n`
  );
  searchIndex.push({
    slug,
    title,
    sourcePath,
    summary: summarizeMarkdown(markdown),
    searchText: buildSearchText(sourcePath, title, headings, markdown)
  });
  pages.push({
    slug,
    sourcePath,
    title,
    section: sectionForPath(sourcePath),
    language: languageForPath(sourcePath),
    headings
  });
}

await fs.writeFile(searchIndexPath, `${JSON.stringify(searchIndex)}\n`);

await fs.mkdir(path.dirname(generatedPath), { recursive: true });
await fs.writeFile(
  generatedPath,
  `export const docs = ${JSON.stringify(pages, null, 2)} as const;\n\n` +
    "export type DocPageMeta = (typeof docs)[number];\n" +
    "export type DocPage = DocPageMeta & { html: string };\n"
);

console.log(`generated ${pages.length} documentation pages`);

function collectHeadings(markdown) {
  const headings = [];
  const seen = new Map();
  for (const line of markdown.split(/\r?\n/)) {
    const match = /^(#{1,4})\s+(.+?)\s*#*$/.exec(line);
    if (!match) {
      continue;
    }
    const depth = match[1].length;
    const text = stripInlineMarkdown(match[2]);
    const baseSlug = slugifyHeading(text);
    const count = seen.get(baseSlug) ?? 0;
    seen.set(baseSlug, count + 1);
    headings.push({
      depth,
      text,
      slug: count === 0 ? baseSlug : `${baseSlug}-${count}`
    });
  }
  return headings;
}

function prepareMarkdown(markdown, sourcePath) {
  return markdown.replace(/\b(href|src)="([^"]+)"/g, (_match, name, value) => {
    const rewritten =
      name === "href" ? rewriteHref(value, sourcePath) : rewriteImageSrc(value, sourcePath);
    return `${name}="${rewritten}"`;
  });
}

function rewriteHref(href, sourcePath) {
  if (isExternalHref(href) || href.startsWith("mailto:")) {
    return href;
  }
  if (href.startsWith("#")) {
    return `#/docs/${slugForPath(sourcePath)}${href}`;
  }

  const [rawTarget, hash = ""] = href.split("#");
  const targetPath = normalizePath(path.join(path.dirname(sourcePath), rawTarget));
  const targetSlug = docPathToSlug.get(targetPath);
  if (targetSlug) {
    return `#/docs/${targetSlug}${hash ? `#${hash}` : ""}`;
  }

  return `https://github.com/kern-project/kern/blob/main/${targetPath}${hash ? `#${hash}` : ""}`;
}

function rewriteImageSrc(src, sourcePath) {
  if (isExternalHref(src) || src.startsWith("data:")) {
    return src;
  }
  const targetPath = normalizePath(path.join(path.dirname(sourcePath), src));
  if (targetPath === "assets/brand/kern-logo.svg") {
    return "/brand/kern-logo.svg";
  }
  return `https://raw.githubusercontent.com/kern-project/kern/main/${targetPath}`;
}

function isExternalHref(href) {
  return /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//");
}

function normalizeLanguage(rawLanguage) {
  const language = (rawLanguage || "text").trim().split(/\s+/)[0].toLowerCase();
  if (language === "kern" || language === "kn") {
    return "rust";
  }
  if (language === "sh" || language === "shell") {
    return "bash";
  }
  if (language === "ps1") {
    return "powershell";
  }
  if (!language) {
    return "text";
  }
  return language;
}

function slugForPath(sourcePath) {
  return normalizePath(sourcePath)
    .replace(/(^|\/)README\.md$/i, "$1index")
    .replace(/\.md$/i, "")
    .replace(/^docs\//, "")
    .replace(/\//g, "--")
    .replace(/[^a-zA-Z0-9\u4e00-\u9fff_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .toLowerCase();
}

function slugifyHeading(value) {
  return value
    .trim()
    .toLowerCase()
    .replace(/`([^`]+)`/g, "$1")
    .replace(/[^\p{Letter}\p{Number}\s_-]/gu, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function stripInlineMarkdown(value) {
  return value
    .replace(/<[^>]+>/g, "")
    .replace(/!\[([^\]]*)\]\([^)]+\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/[`*_~]/g, "")
    .trim();
}

function summarizeMarkdown(markdown) {
  return plainTextFromMarkdown(markdown).split(/\s+/).slice(0, 28).join(" ");
}

function buildSearchText(sourcePath, title, headings, markdown) {
  return [
    sourcePath,
    title,
    headings.map((heading) => heading.text).join(" "),
    plainTextFromMarkdown(markdown)
  ]
    .join(" ")
    .toLowerCase();
}

function plainTextFromMarkdown(markdown) {
  return markdown
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/!\[([^\]]*)\]\([^)]+\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/[#>*_`~|:-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function titleFromPath(sourcePath) {
  return path
    .basename(sourcePath, ".md")
    .replace(/[-_]/g, " ")
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

function titleForPage(sourcePath, headings) {
  if (sourcePath === "README.md") {
    return "Kern";
  }
  if (sourcePath === "README.zh.md") {
    return "Kern 中文";
  }
  return headings[0]?.text ?? titleFromPath(sourcePath);
}

function sectionForPath(sourcePath) {
  if (sourcePath.startsWith("docs/tutorial/zh")) {
    return "Tutorial 中文";
  }
  if (sourcePath.startsWith("docs/tutorial/en")) {
    return "Tutorial";
  }
  if (sourcePath.startsWith("docs/tutorial")) {
    return "Tutorial";
  }
  if (sourcePath === "README.md" || sourcePath === "README.zh.md") {
    return "Overview";
  }
  if (sourcePath.includes("distribution") || sourcePath.endsWith("install.md") || sourcePath.endsWith("nix.md")) {
    return "Install";
  }
  return "Reference";
}

function languageForPath(sourcePath) {
  return sourcePath.includes("/zh/") || sourcePath.endsWith(".zh.md") ? "zh" : "en";
}

function normalizePath(value) {
  return value.replace(/\\/g, "/").replace(/^\.\//, "");
}
