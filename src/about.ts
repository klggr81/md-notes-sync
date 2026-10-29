import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyLook, fillIcons, showAppIcon } from "./look";

// Mirrors `AboutInfo` in src-tauri/src/lib.rs and `Latest` in src-tauri/src/update.rs
interface AboutInfo {
  version: string;
  repository: string;
  accent: string[] | null;
  mica: boolean;
  updated: boolean;
}

interface Latest {
  version: string;
  newer: boolean;
  page: string;
  installable: boolean;
}

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const ring = document.getElementById("up-ring") as unknown as SVGElement;
const checkButton = $<HTMLButtonElement>("up-check");
const installButton = $<HTMLButtonElement>("up-install");

let info: AboutInfo;
/** Where "What's new" leads */
let page = "";
let busy = false;

/** The update card: a glyph, a progress ring or a coloured badge, a title and a line of text. */
function show(state: "idle" | "busy" | "success" | "info" | "critical", title: string, text: string) {
  $("up-icon").hidden = state !== "idle";
  ring.toggleAttribute("hidden", state !== "busy");
  $("up-badge").hidden = state === "idle" || state === "busy";
  $("up-badge").className = `badge ${state}`;
  $("up-title").textContent = title;
  $("up-text").textContent = text;
}

async function check() {
  if (busy) return;
  busy = true;
  checkButton.disabled = true;
  $("up-actions").hidden = true;
  show("busy", "Checking for updates…", "Asking GitHub for the newest version.");
  try {
    const latest = await invoke<Latest>("check_update");
    page = latest.page;
    if (!latest.newer) {
      show("success", "You're up to date", `${info.version} is the newest version.`);
      checkButton.textContent = "Check again";
      checkButton.hidden = false;
    } else {
      show(
        "info",
        `Version ${latest.version} is available`,
        latest.installable
          ? `You have ${info.version}. The app downloads the new version and restarts.`
          : `You have ${info.version}. Download the new version from its release page.`,
      );
      checkButton.hidden = true;
      installButton.hidden = !latest.installable;
      $("up-page").textContent = latest.installable ? "What's new" : "Open release page";
      $("up-actions").hidden = false;
    }
  } catch (e) {
    show("critical", "Couldn't check for updates", String(e));
    checkButton.textContent = "Try again";
    checkButton.hidden = false;
  } finally {
    checkButton.disabled = false;
    busy = false;
  }
}

const mb = (bytes: number) => (bytes / 1_000_000).toFixed(1);

async function install() {
  if (busy) return;
  busy = true;
  installButton.disabled = true;
  show("busy", "Installing the update…", "Downloading from GitHub.");
  $("up-progress").hidden = false;
  $("up-progress").classList.add("unknown");
  $("up-stats").textContent = "Starting…";
  try {
    // Quits this app and starts the new version when it's done
    await invoke("install_update");
    show("busy", "Restarting…", "The new version starts in a moment.");
    $("up-progress").hidden = true;
  } catch (e) {
    show("critical", "Couldn't install the update", String(e));
    $("up-progress").hidden = true;
    installButton.disabled = false;
    busy = false;
  }
}

void listen<{ done: number; total: number | null }>("update-download", ({ payload: { done, total } }) => {
  $("up-progress").classList.toggle("unknown", !total);
  $("up-fill").style.setProperty("--done", total ? `${(done / total) * 100}%` : "0%");
  $("up-stats").textContent = total ? `${mb(done)} of ${mb(total)} MB` : `${mb(done)} MB`;
});

checkButton.addEventListener("click", check);
installButton.addEventListener("click", install);
$("up-page").addEventListener("click", () => invoke("open_link", { url: page }));
$("link-source").addEventListener("click", () => invoke("open_link", { url: info.repository }));
$("link-license").addEventListener("click", () => invoke("open_link", { url: `${info.repository}/blob/main/LICENSE` }));
document.querySelectorAll<HTMLElement>("[data-url]").forEach((el) =>
  el.addEventListener("click", () => invoke("open_link", { url: el.dataset.url })),
);

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") void invoke("close_window");
});

async function init() {
  info = await invoke<AboutInfo>("about_info");
  applyLook(info.accent, info.mica);
  $("version").textContent = `Version ${info.version}`;
  $("source-text").textContent = info.repository.replace("https://", "");
  if (info.updated) {
    show("success", `Updated to version ${info.version}`, "You have the newest version.");
    page = `${info.repository}/releases/tag/v${info.version}`;
    checkButton.hidden = true;
    installButton.hidden = true;
    $("up-page").textContent = "What's new";
    $("up-actions").hidden = false;
  }
}

fillIcons();
showAppIcon();
void init();
