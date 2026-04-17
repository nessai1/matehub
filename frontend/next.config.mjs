import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** @type {import('next').NextConfig} */
const nextConfig = {
  // Turbopack: resolve aliases
  turbopack: {
    resolveAlias: {
      "shadcn/tailwind.css": path.resolve(__dirname, "node_modules/shadcn/dist/tailwind.css"),
    },
  },

  // Transpile SDK packages (they're TypeScript source, not pre-built)
  transpilePackages: ["@matehub/sdk-video", "@matehub/sdk-chat"],
};

export default nextConfig;
