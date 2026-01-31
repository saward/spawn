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
          label: "CLI Reference",
          items: [{ label: "spawn check", slug: "cli/check" }],
        },
      ],
    }),
  ],

  adapter: cloudflare(),
});
