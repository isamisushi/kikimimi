// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// https://astro.build/config
export default defineConfig({
  site: "https://kikimimi.dev",
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
        { label: "Overview", slug: "overview" },
        { label: "Installation", slug: "installation" },
        { label: "Quickstart", slug: "quickstart" },
        { label: "Storage & sharing", slug: "storage-and-sharing" },
        { label: "Team dashboard from S3", slug: "s3-dashboard" },
        { label: "How it works", slug: "how-it-works" },
        { label: "Queries", slug: "queries" },
        { label: "Subscription usage", slug: "subscription-usage" },
        { label: "The improvement loop", slug: "improvement-loop" },
        { label: "Teams", slug: "teams" },
        { label: "Bring your own bucket", slug: "sinks" },
        { label: "Privacy", slug: "privacy" },
        { label: "Development", slug: "development" },
      ],
    }),
  ],
});
