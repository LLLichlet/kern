# Website

This directory contains the public website and future guide/reference surface
for Kern.

## Stack

- Astro
- TypeScript
- Tailwind CSS
- Bun

## Current Role

Today the repository `docs/` directory still contains the authoritative
technical reference for Kern's language, runtime, compiler, and package
behavior.

The website is the product-facing layer on top of that material:

- landing pages
- information architecture
- guide chapters
- future website-native reference pages

The migration rule is deliberate:

- move material here gradually
- do not duplicate or weaken authoritative docs casually
- prefer explicit links back to the repository docs over stale copies

## Local Development

From the repository root:

```bash
cd website
/home/lenovo/.bun/bin/bun install
/home/lenovo/.bun/bin/bun run dev
```

Useful commands:

```bash
/home/lenovo/.bun/bin/bun run check
/home/lenovo/.bun/bin/bun run build
```

## Deployment

GitHub Pages deployment is defined in:

- [`../.github/workflows/website.yml`](../.github/workflows/website.yml)

The workflow:

- installs Bun
- installs `website/` dependencies
- builds the static Astro site
- uploads `website/dist`
- deploys to GitHub Pages on pushes to `main`

## Content Model

Guide content currently lives under:

- [`src/content/guide`](./src/content/guide)

Top-level website sections currently include:

- `/`
- `/guide`
- `/reference`
- `/tooling`
- `/architecture`
- `/docs`
