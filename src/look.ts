import { glyphs, type GlyphName } from "./glyphs";

// The look shared by the main window and the About window.

/** Puts the Segoe Fluent Icons glyph into each element with `data-glyph`. */
export function fillIcons() {
  document.querySelectorAll<HTMLElement>("[data-glyph]").forEach((el) => {
    el.textContent = glyphs[el.dataset.glyph as GlyphName];
    el.setAttribute("aria-hidden", "true");
  });
}

/** The app icon in the header. */
export function showAppIcon() {
  const appIcon = new URL("../icons/app-icon.svg", import.meta.url).href;
  document.querySelectorAll<HTMLImageElement>(".app-icon").forEach((img) => (img.src = appIcon));
}

const ACCENT_SHADES = ["light-3", "light-2", "light-1", "base", "dark-1", "dark-2", "dark-3"];

/** The Windows accent colour (Settings > Personalization > Colors), and Mica or a solid background. */
export function applyLook(accent: string[] | null, mica: boolean) {
  const root = document.documentElement;
  accent?.forEach((color, i) => root.style.setProperty(`--accent-${ACCENT_SHADES[i]}`, color));
  root.classList.toggle("solid", !mica);
}
