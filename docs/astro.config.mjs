// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

import cloudflare from "@astrojs/cloudflare";

// https://astro.build/config
export default defineConfig({
  site: "https://docs.spawn.dev",
  server: { port: 4321 },

  integrations: [
    starlight({
      title: "Spawn",
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/saward/spawn",
        },
      ],
      sidebar: [
        {
          label: "Getting Started",
          items: [
            { label: "Installing", slug: "getting-started/install" },
            { label: "The Magic of Spawn", slug: "getting-started/magic" },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Manage Databases", slug: "guides/manage-databases" },
          ],
        },
        {
          label: "Recipes",
          items: [
            { label: "Introduction", slug: "recipes/introduction" },
            { label: "Test Macros", slug: "recipes/test-macros" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "Configuration File", slug: "reference/config" },
            { label: "Templating", slug: "reference/templating" },
          ],
        },
        {
          label: "CLI Reference",
          items: [
            { label: "spawn init", slug: "cli/init" },
            { label: "spawn check", slug: "cli/check" },
            {
              label: "Migration",
              items: [
                { label: "spawn migration new", slug: "cli/migration-new" },
                { label: "spawn migration pin", slug: "cli/migration-pin" },
                { label: "spawn migration build", slug: "cli/migration-build" },
                { label: "spawn migration apply", slug: "cli/migration-apply" },
                { label: "spawn migration adopt", slug: "cli/migration-adopt" },
                {
                  label: "spawn migration status",
                  slug: "cli/migration-status",
                },
              ],
            },
            {
              label: "Test",
              items: [
                { label: "spawn test new", slug: "cli/test-new" },
                { label: "spawn test build", slug: "cli/test-build" },
                { label: "spawn test run", slug: "cli/test-run" },
                { label: "spawn test compare", slug: "cli/test-compare" },
                { label: "spawn test expect", slug: "cli/test-expect" },
              ],
            },
          ],
        },
      ],
    }),
  ],

  adapter: cloudflare(),
});
