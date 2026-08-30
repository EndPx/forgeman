// Copy the exported dashboard (web/out) into target/dashboard, which the
// Rust build embeds via rust-embed. Run after `npm run build --prefix web`.
const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "..");
const src = path.join(root, "web", "out");
const dest = path.join(root, "target", "dashboard");

if (!fs.existsSync(path.join(src, "index.html"))) {
  console.error("web/out/index.html not found — run `npm run build --prefix web` first.");
  process.exit(1);
}

fs.rmSync(dest, { recursive: true, force: true });
fs.mkdirSync(dest, { recursive: true });
fs.cpSync(src, dest, { recursive: true });

const files = [];
(function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full);
    else files.push(path.relative(dest, full));
  }
})(dest);
console.log(`dashboard assets copied to target/dashboard (${files.length} files)`);
