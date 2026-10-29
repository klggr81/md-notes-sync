// Writes the icons: app-icon.svg (the app) and tray-<state>.svg (the tray icon
// when idle, while syncing, after an error), then renders them with `tauri icon`.
// Fluent style: a note page with a folded corner in a bold gradient, white sync
// arrows. The tray icon leaves out the text lines and fills the whole square, so
// it is as big as other tray icons; it is rendered for every taskbar scale
// (16 px at 100 % … 48 px at 300 %), so Windows doesn't have to resize it.
//
//   node icons/icons.ts
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

// Paths from this folder, so it works from any working directory
const here = dirname(fileURLToPath(import.meta.url));
const icons = join(here, "../src-tauri/icons");

export const TRAY_SIZES = [16, 20, 24, 32, 40, 48];

interface Colors {
  light: string;
  dark: string;
  fold: string;
}

const green: Colors = { light: "#5ad15a", dark: "#0b7a0b", fold: "#b4ecac" };
const blue: Colors = { light: "#4cc2ff", dark: "#0062c4", fold: "#b8e6ff" };
const red: Colors = { light: "#ff6f61", dark: "#c42b1c", fold: "#ffc7c1" };

// The page: left 104, right 920, top 16, bottom 1008, folded corner 300, corners 104
const L = 104, R = 920, T = 16, B = 1008, F = 300, C = 104;
const page = `M${L + C} ${T} H${R - F} L${R} ${T + F} V${B - C} Q${R} ${B} ${R - C} ${B} H${L + C} Q${L} ${B} ${L} ${B - C} V${T + C} Q${L} ${T} ${L + C} ${T} Z`;
const fold = `M${R - F} ${T} V${T + F - C * 0.6} Q${R - F} ${T + F} ${R - F + C * 0.6} ${T + F} H${R} Z`;

const f = (n: number) => n.toFixed(1);

/** Two arrows around (cx, cy), each turning 140°, like the Windows sync glyph. */
function syncArrows(cx: number, cy: number, r: number, stroke: number) {
  const point = (deg: number, radius = r) => {
    const a = (deg * Math.PI) / 180;
    return [cx + radius * Math.cos(a), cy + radius * Math.sin(a)];
  };
  const head = stroke * 1.7; // length of the arrow head
  const half = stroke * 1.15; // half its width
  let arcs = "";
  let heads = "";
  for (const start of [200, 20]) {
    const end = start + 140;
    // The arc ends at the base of the arrow head; the tip follows the tangent there
    const stop = end - (head / r) * (180 / Math.PI);
    const [x0, y0] = point(start);
    const [x1, y1] = point(stop + 4);
    arcs += `M${f(x0)} ${f(y0)} A${r} ${r} 0 0 1 ${f(x1)} ${f(y1)} `;
    const a = (stop * Math.PI) / 180;
    const [bx, by] = point(stop);
    const [tx, ty] = [bx - Math.sin(a) * head, by + Math.cos(a) * head];
    const [ox, oy] = point(stop, r + half);
    const [ix, iy] = point(stop, r - half);
    heads += `<path d="M${f(ox)} ${f(oy)} L${f(tx)} ${f(ty)} L${f(ix)} ${f(iy)} Z" fill="#fff" stroke="#fff" stroke-width="${f(stroke * 0.3)}" stroke-linejoin="round"/>`;
  }
  return `<path d="${arcs.trim()}" fill="none" stroke="#fff" stroke-width="${stroke}" stroke-linecap="round"/>${heads}`;
}

const exclamation =
  '<rect x="452" y="300" width="120" height="400" rx="60" fill="#fff"/><circle cx="512" cy="836" r="68" fill="#fff"/>';

function icon(c: Colors, inside: string) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <!-- Written by icons.ts: edit that, not this file -->
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="${c.light}"/>
      <stop offset="1" stop-color="${c.dark}"/>
    </linearGradient>
  </defs>
  <path d="${page}" fill="url(#g)"/>
  <path d="${fold}" fill="${c.fold}"/>
  ${inside}
</svg>
`;
}

const lines =
  '<rect x="248" y="244" width="300" height="72" rx="36" fill="#fff" opacity="0.92"/>' +
  '<rect x="248" y="388" width="528" height="72" rx="36" fill="#fff" opacity="0.92"/>';

const svgs = {
  "app-icon.svg": icon(green, lines + syncArrows(512, 724, 188, 80)),
  "tray-idle.svg": icon(green, syncArrows(512, 628, 250, 116)),
  "tray-syncing.svg": icon(blue, syncArrows(512, 628, 250, 116)),
  "tray-error.svg": icon(red, exclamation),
};
for (const [name, svg] of Object.entries(svgs)) writeFileSync(join(here, name), svg);

// Render: the app icon in Tauri's sizes, the tray icons in every taskbar size
const tmp = mkdtempSync(join(tmpdir(), "md-notes-sync-icons-"));
const tauri = (args: string) => execSync(`npx tauri icon ${args}`, { stdio: "ignore" });
try {
  tauri(`"${join(here, "app-icon.svg")}" -o "${tmp}/app"`);
  for (const file of ["32x32.png", "128x128.png", "128x128@2x.png", "icon.png", "icon.ico"]) {
    copyFileSync(join(tmp, "app", file), join(icons, file));
  }
  mkdirSync(join(icons, "tray"), { recursive: true });
  for (const state of ["idle", "syncing", "error"]) {
    tauri(`"${join(here, `tray-${state}.svg`)}" -o "${tmp}/${state}" -p ${TRAY_SIZES.join(",")}`);
    for (const size of TRAY_SIZES) {
      copyFileSync(join(tmp, state, `${size}x${size}.png`), join(icons, "tray", `${state}-${size}.png`));
    }
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
