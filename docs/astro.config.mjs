// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// https://astro.build/config
export default defineConfig({
  site: "https://isamisushi.github.io",
  base: "/kikimimi",
  integrations: [
    starlight({
      title: "kikimimi",
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/isamisushi/kikimimi",
        },
      ],
      editLink: {
        baseUrl: "https://github.com/isamisushi/kikimimi/edit/main/docs/",
      },
      sidebar: [
        { label: "Installation", slug: "installation" },
        { label: "Quickstart", slug: "quickstart" },
        { label: "How it works", slug: "how-it-works" },
        { label: "Queries", slug: "queries" },
        { label: "Teams", slug: "teams" },
        { label: "Bring your own bucket", slug: "sinks" },
        { label: "Privacy", slug: "privacy" },
        { label: "Development", slug: "development" },
      ],
    }),
  ],
});
