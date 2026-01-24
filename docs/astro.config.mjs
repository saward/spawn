// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// https://astro.build/config
export default defineConfig({
  site: "https://docs.spawn.dev",
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
        // {
        //   label: "Reference",
        //   autogenerate: { directory: "reference" },
        // },
      ],
    }),
  ],
});
