import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";
import { z } from "astro/zod";

const guide = defineCollection({
  loader: glob({ pattern: "**/*.md", base: "./src/content/guide" }),
  schema: z.object({
    title: z.string(),
    summary: z.string(),
    order: z.number()
  })
});

export const collections = { guide };
