import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

export default defineConfig({
  // GitHub Pages publishes from `<org>.github.io/memstead/` by default;
  // a custom domain can override `site` + `base` later without changing
  // the docs build itself.
  site: "https://memstead.github.io",
  base: "/memstead",
  integrations: [
    starlight({
      title: "Memstead API",
      description:
        "Auto-generated reference for the Memstead engine's MCP, CLI, UniFFI, WASM, and Registry HTTP surfaces.",
      components: {
        Footer: "./src/components/Footer.astro",
      },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/memstead/memstead",
        },
      ],
      sidebar: [
        {
          label: "Overview",
          link: "/",
        },
        {
          label: "Reference",
          items: [
            { label: "MCP tools", link: "/reference/mcp/" },
            { label: "CLI", link: "/reference/cli/cli/" },
            { label: "UniFFI surface", link: "/reference/uniffi/" },
            { label: "WASM surface", link: "/reference/wasm/" },
            { label: "Registry HTTP", link: "/reference/registry/" },
            { label: "Surface Parity Matrix", link: "/reference/parity/" },
            { label: "Error Code Index", link: "/reference/errors/" },
          ],
        },
      ],
    }),
  ],
});
