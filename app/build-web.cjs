// Builds public/coded-launch.js and copies the program IDL next to it.
// Runs on Vercel via `npm run build:web --prefix app`.
const { build } = require("esbuild");
const { existsSync, copyFileSync } = require("node:fs");
const { join } = require("node:path");

const root = join(__dirname, "..");
const programId = process.env.CODED_PROGRAM_ID || "Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS";

build({
  entryPoints: [join(__dirname, "src/web-entry.ts")],
  bundle: true,
  minify: true,
  format: "iife",
  platform: "browser",
  target: "es2020",
  outfile: join(root, "public/coded-launch.js"),
  inject: [join(__dirname, "src/buffer-shim.ts")],
  define: { "process.env.CODED_PROGRAM_ID": JSON.stringify(programId), global: "globalThis" },
  logLevel: "info",
})
  .then(() => {
    const idl = join(root, "idl/coded_router.json");
    if (existsSync(idl)) {
      copyFileSync(idl, join(root, "public/coded_router.json"));
      console.log("copied idl/coded_router.json");
    } else {
      console.log("idl/coded_router.json not found: the site works, launching stays off until the program is deployed");
    }
  })
  .catch(() => process.exit(1));
