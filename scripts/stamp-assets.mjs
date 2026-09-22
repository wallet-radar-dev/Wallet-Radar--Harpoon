// Cache-busting: stamp app.js / app.css with a content hash so the browser always
// fetches the freshly built bundle instead of a stale cached copy.
import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";

const hash = (f) => createHash("md5").update(readFileSync(f)).digest("hex").slice(0, 8);
const js = hash("web/app.js");
const css = hash("web/app.css");

let html = readFileSync("web/index.html", "utf8");
html = html.replace(/app\.js(\?v=[^"]*)?"/g, `app.js?v=${js}"`);
html = html.replace(/app\.css(\?v=[^"]*)?"/g, `app.css?v=${css}"`);
writeFileSync("web/index.html", html);
console.log(`stamped app.js?v=${js} app.css?v=${css}`);
