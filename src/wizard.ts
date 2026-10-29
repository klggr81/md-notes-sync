// The setup wizard: rclone, the cloud storage and its sign-in, the folder in
// the cloud, the folder on this PC, save, and a test run. The work happens in
// setup.rs; the test run is a normal sync with dryRun.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { glyphs } from "./glyphs";

type Provider = "nextcloud" | "dropbox" | "googleDrive" | "sftp";
const NAMES: Record<Provider, string> = { nextcloud: "Nextcloud", dropbox: "Dropbox", googleDrive: "Google Drive", sftp: "SFTP server" };

// Mirrors src-tauri/src/setup.rs and sync_core::setup
interface RcloneInfo {
  path: string;
  version: string | null;
}
interface Connection {
  provider: Provider;
  name: string;
  label: string;
  /** SFTP: fingerprint of the server's key */
  serverKey?: string;
}
interface Current {
  provider: Provider | null;
  remotePath: string;
  local: string;
}
interface Info {
  rclone: RcloneInfo | null;
  current: Current | null;
  defaultLocal: string;
  connected: Connection[];
  existing: Connection[];
  autostart: boolean;
}
interface NotesFolder {
  path: string;
  fromApp: boolean;
}
interface LocalFolder {
  path: string;
  exists: boolean;
  files: number;
}
interface Saved {
  freshStart: boolean;
  markerProblem: string | null;
}
// Mirrors `Finished` in src-tauri/src/lib.rs and `Event` in sync-core/src/runner.rs (the parts used here)
interface Finished {
  summary: { copied: number; deleted: number; warnings: number } | null;
  error: string | null;
}
type SyncEvent =
  | { type: "waiting" | "started" | "backup" | "backupFailed" }
  | { type: "line"; text: string; kind: string }
  | { type: "progress"; text: string; percent: number | null };

/** How the setup ended: sync now, closed after saving, or cancelled */
export type Ending = "sync" | "close" | null;

const PAGES = ["rclone", "cloud", "signin", "cloud-folder", "local", "done", "test"] as const;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);
const next = $<HTMLButtonElement>("wz-next");
const back = $<HTMLButtonElement>("wz-back");

let info: Info;
let page = 0;
let provider: Provider = "nextcloud";
/** The working connection for `provider` */
let connection: Connection | null = null;
/** Nextcloud: the Notes app's folder; undefined = not asked yet */
let notes: NotesFolder | null | undefined;
/** The folder in the cloud is another than the Notes app's, and the user knows */
let otherFolderOk = false;
let local: LocalFolder | null = null;
let busy = false;
let cancelable = true;
let onClose: (ending: Ending) => void = () => {};
/** What saving did */
let saved: Saved | null = null;
/** The test run is running; its events go to the test page */
let testing = false;
/** The last test run went through, so Next is Sync now */
let testOk = false;

/** Opens the wizard. `close` gets how it ended. */
export async function openWizard(close: (ending: Ending) => void, canCancel: boolean) {
  onClose = close;
  cancelable = canCancel;
  saved = null;
  testing = false;
  info = await invoke<Info>("setup_info");
  provider = info.current?.provider ?? "nextcloud";
  connection = info.connected.find((c) => c.provider === provider) ?? null;
  notes = undefined;
  otherFolderOk = false;
  local = null;
  input("cf-path").value = info.current?.remotePath ?? "";
  input("lf-path").value = info.current?.local ?? info.defaultLocal;
  input("dn-autostart").checked = info.autostart;
  $("wizard").hidden = false;
  await show(info.rclone ? 1 : 0);
}

/** Disables Next and Back while something runs (always during the test run). */
function setBusy(on: boolean) {
  busy = on || testing;
  next.disabled = busy;
  back.disabled = busy || page === 0;
}

async function show(i: number) {
  page = i;
  const name = PAGES[i];
  document.querySelectorAll<HTMLElement>("#wizard .page").forEach((p) => (p.hidden = p.dataset.page !== name));
  $("wz-step").textContent = `Step ${i + 1} of ${PAGES.length}`;
  next.disabled = false;
  back.disabled = i === 0;
  next.textContent = name === "done" ? "Save" : name === "test" ? "Sync now" : "Next";
  // Saved by now: Close instead of Cancel
  $("wz-cancel").hidden = !cancelable || name === "test";
  $("wz-close").hidden = name !== "test";
  $("wizard").querySelector(".wizard-body")?.scrollTo(0, 0);
  await enter[name]();
}

function hideAll(...ids: string[]) {
  ids.forEach((id) => ($(id).hidden = true));
}

// --- What each page shows when it opens --------------------------------------------

const enter: Record<(typeof PAGES)[number], () => Promise<void> | void> = {
  rclone() {
    const r = info.rclone;
    $("rc-found").hidden = !r;
    $("rc-missing").hidden = !!r;
    $("rc-progress").hidden = true;
    if (r) $("rc-version").textContent = `${r.version ?? "rclone"} · ${r.path}`;
  },

  cloud() {
    document.querySelectorAll<HTMLInputElement>('input[name="provider"]').forEach((r) => (r.checked = r.value === provider));
  },

  signin() {
    $("si-title").textContent = `Connect to ${NAMES[provider]}`;
    hideAll("si-waiting", "si-error", "si-password");
    showConnection();
    const existing = info.existing.filter((c) => c.provider === provider);
    $("si-existing").hidden = existing.length === 0;
    const list = $("si-existing-list");
    list.replaceChildren(
      ...existing.map((c) => {
        const li = document.createElement("li");
        li.className = "card";
        li.innerHTML = `<span class="icon">${glyphs.cloud}</span><div class="label"><span></span><span class="path"></span></div><button class="btn">Use</button>`;
        li.querySelector(".label span")!.textContent = `rclone remote “${c.name}”`;
        li.querySelector(".path")!.textContent = c.label;
        li.querySelector("button")!.addEventListener("click", () => signIn("use_existing", { provider, name: c.name }, false));
        return li;
      }),
    );
    $("si-nextcloud").hidden = provider !== "nextcloud";
    $("si-sftp").hidden = provider !== "sftp";
    $("si-browser").hidden = provider === "nextcloud" || provider === "sftp";
    $("si-browser-text").textContent =
      `Your browser opens. Sign in to ${NAMES[provider]} and allow rclone to access your files, then come back here. Safest: a separate account that only has your notes folder shared with it.` +
      (provider === "googleDrive" ? " Google Drive syncs more slowly than the others: Google limits how fast apps may list files." : "");
  },

  async ["cloud-folder"]() {
    const name = NAMES[provider];
    $("cf-title").textContent = `Notes folder in ${name}`;
    $("cf-caption").textContent =
      provider === "sftp" ? "A path from your folder on the server, for example Notes." : `A path from the top of your ${name}, for example Documents/Notes.`;
    hideAll("cf-warning", "cf-hint");
    if (provider === "nextcloud" && notes === undefined) {
      setBusy(true);
      notes = await invoke<NotesFolder | null>("notes_folder");
      setBusy(false);
    }
    const path = input("cf-path");
    const sameProvider = info.current?.provider === provider;
    if (!path.value || !sameProvider) path.value = (sameProvider && info.current?.remotePath) || notes?.path || "Notes";
    if (provider === "nextcloud") {
      $("cf-hint").hidden = false;
      $("cf-hint-text").textContent = notes?.fromApp
        ? `The Nextcloud Notes app keeps its notes in “${notes.path}”. Use this folder to see the notes in the app.`
        : notes
          ? `“${notes.path}” has the usual name of the Notes app's folder. The Notes app didn't answer: is it installed and turned on? With a separate user for the sync, pick the folder shared with it.`
          : "The Notes app didn't say which folder it uses. Is it installed and turned on? It shows the folder in its settings. With a separate user for the sync, pick the folder shared with it.";
    }
    await browse("");
  },

  local() {
    hideAll("lf-error");
  },

  done() {
    $("dn-remote").textContent = `${NAMES[provider]}: ${input("cf-path").value}`;
    $("dn-local").textContent = local?.path ?? "";
    const note = !local?.exists
      ? "The folder doesn't exist yet: saving creates it."
      : local.files > 0
        ? `The folder already has ${local.files >= 100000 ? "more than 100 000" : local.files} file${local.files === 1 ? "" : "s"}. The first sync merges the folder with the cloud: nothing is deleted, and where a file differs on both sides, the newer one wins.`
        : "";
    $("dn-note").hidden = !note;
    $("dn-note-text").textContent = note;
  },

  test() {
    hideAll("tr-progress", "tr-result", "tr-lines");
    $("tr-intro").textContent = saved?.freshStart
      ? "Saved. The first sync with these folders merges both sides: nothing is deleted. The test run shows what it would copy, and changes nothing."
      : "Saved. The test run shows what the next sync would copy and delete, and changes nothing.";
    if (saved?.markerProblem) {
      testResult("critical", "Can't sync yet", `RCLONE_TEST couldn't be put on both sides, so the sync would refuse to run: ${saved.markerProblem}`);
      next.disabled = true;
      return;
    }
    void startTest();
  },
};

// --- What Next does on each page ------------------------------------------------------

const leave: Record<(typeof PAGES)[number], () => Promise<boolean>> = {
  async rclone() {
    if (!info.rclone) showError("rc", "Download rclone first.");
    return !!info.rclone;
  },

  async cloud() {
    const chosen = document.querySelector<HTMLInputElement>('input[name="provider"]:checked')?.value as Provider | undefined;
    if (!chosen) return false;
    if (chosen !== provider) {
      provider = chosen;
      connection = info.connected.find((c) => c.provider === provider) ?? null;
      notes = undefined;
      otherFolderOk = false;
      input("cf-path").value = "";
    }
    return true;
  },

  async signin() {
    if (!connection) showError("si", `Connect to ${NAMES[provider]} first.`);
    return !!connection;
  },

  async ["cloud-folder"]() {
    const path = input("cf-path").value.trim().replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
    input("cf-path").value = path;
    if (!path) {
      warn("Enter the folder that holds your notes.");
      return false;
    }
    if (provider === "nextcloud" && notes?.fromApp && path !== notes.path && !otherFolderOk) {
      warn(`The Notes app only shows notes in “${notes.path}”. Notes in “${path}” won't appear there.`, "Use it anyway", () => {
        otherFolderOk = true;
        void goNext();
      });
      return false;
    }
    if (!(await invoke<boolean>("check_cloud_folder", { provider, path }))) {
      const hint = provider === "nextcloud" ? " The Notes app creates it the first time it's opened, or create it now." : "";
      warn(`“${path}” doesn't exist yet.${hint}`, "Create it", async () => {
        try {
          await invoke("create_cloud_folder", { provider, path });
          void goNext();
        } catch (e) {
          warn(`Couldn't create it: ${e}`);
        }
      });
      return false;
    }
    return true;
  },

  async local() {
    try {
      local = await invoke<LocalFolder>("check_local_folder", { path: input("lf-path").value });
      input("lf-path").value = local.path;
      return true;
    } catch (e) {
      showError("lf", String(e));
      return false;
    }
  },

  async done() {
    saved = await invoke<Saved>("save_setup", { provider, remotePath: input("cf-path").value, local: local?.path ?? "" });
    const autostart = input("dn-autostart").checked;
    if (autostart !== info.autostart) await invoke("set_autostart", { on: autostart });
    info.autostart = autostart;
    return true;
  },

  // Sync now, or Try again after a failed test run
  async test() {
    if (testOk) finish("sync");
    else void startTest();
    return false;
  },
};

async function goNext() {
  if (busy) return;
  setBusy(true);
  let ok = false;
  try {
    ok = await leave[PAGES[page]]();
  } catch (e) {
    showError(PAGES[page] === "done" ? "dn" : "si", String(e));
  } finally {
    setBusy(false);
  }
  if (ok && page < PAGES.length - 1) await show(page + 1);
}

function showError(prefix: string, text: string) {
  const box = document.getElementById(`${prefix}-error`);
  if (box) {
    box.hidden = false;
    $(`${prefix}-error-text`).textContent = text;
  } else {
    window.dispatchEvent(new CustomEvent("app-message", { detail: text }));
  }
}

function warn(text: string, action?: string, then?: () => void) {
  $("cf-warning").hidden = false;
  $("cf-warning-text").textContent = text;
  const button = $<HTMLButtonElement>("cf-warning-action");
  button.hidden = !action;
  button.textContent = action ?? "";
  button.onclick = then ?? null;
}

// --- Sign-in ------------------------------------------------------------------------

function showConnection() {
  $("si-connected").hidden = !connection;
  $("si-forms").hidden = !!connection;
  if (connection) $("si-connected-text").textContent = connection.label + (connection.serverKey ? ` · server key ${connection.serverKey}` : "");
}

/** Runs one of the sign-in commands; `browser`: it waits for the browser and can be cancelled. */
async function signIn(command: string, args: Record<string, unknown>, browser = true) {
  if (busy) return;
  setBusy(true);
  hideAll("si-error", "si-link-line", "si-copy");
  $("si-waiting").hidden = false;
  $("si-cancel").hidden = !browser;
  $("si-waiting-text").textContent = browser ? "Waiting for the sign-in in your browser…" : "Connecting…";
  $("si-forms").hidden = true;
  try {
    connection = await invoke<Connection>(command, args);
    info.connected = [...info.connected.filter((c) => c.provider !== provider), connection];
    notes = undefined;
  } catch (e) {
    showError("si", String(e));
  } finally {
    $("si-waiting").hidden = true;
    setBusy(false);
    showConnection();
  }
}

$("si-nc-browser").addEventListener("click", () => signIn("nextcloud_sign_in", { server: input("si-server").value }));
$("si-nc-password").addEventListener("click", () =>
  signIn("nextcloud_app_password", { server: input("si-server").value, user: input("si-user").value, password: input("si-pass").value }, false),
);
$("si-cloud-browser").addEventListener("click", () => signIn("cloud_sign_in", { provider }));
$("si-sftp-connect").addEventListener("click", () =>
  signIn(
    "sftp_connect",
    { server: input("si-sftp-server").value, port: input("si-sftp-port").value, user: input("si-sftp-user").value, password: input("si-sftp-pass").value },
    false,
  ),
);
// A Hetzner Storage Box has SFTP on port 23
$("si-sftp-server").addEventListener("input", () => {
  const port = input("si-sftp-port");
  if (/\.your-storagebox\.de\s*$/i.test(input("si-sftp-server").value) && !port.value) port.value = "23";
});
$("si-show-password").addEventListener("click", () => ($("si-password").hidden = !$("si-password").hidden));
$("si-cancel").addEventListener("click", () => invoke("cancel_sign_in"));
$("si-again").addEventListener("click", () => {
  connection = null;
  showConnection();
});
$("si-copy").addEventListener("click", () => navigator.clipboard.writeText($("si-link").textContent ?? ""));

void listen<string>("setup-link", (e) => {
  $("si-link").textContent = e.payload;
  $("si-link-line").hidden = false;
  $("si-copy").hidden = false;
});

// --- rclone download ------------------------------------------------------------------

const mb = (bytes: number) => (bytes / 1_000_000).toFixed(1);

$("rc-download").addEventListener("click", async () => {
  if (busy) return;
  setBusy(true);
  $<HTMLButtonElement>("rc-download").disabled = true;
  $("rc-progress").hidden = false;
  $("rc-stats").textContent = "Starting…";
  try {
    const installed = await invoke<RcloneInfo>("install_rclone");
    // Only now can rclone tell which remotes there are already
    info = await invoke<Info>("setup_info");
    info.rclone ??= installed;
    enter.rclone();
  } catch (e) {
    $("rc-stats").textContent = String(e);
  } finally {
    $<HTMLButtonElement>("rc-download").disabled = false;
    setBusy(false);
  }
});

void listen<{ done: number; total: number | null }>("setup-download", ({ payload: { done, total } }) => {
  $("rc-progress").classList.toggle("unknown", !total);
  $("rc-fill").style.setProperty("--done", total ? `${(done / total) * 100}%` : "0%");
  $("rc-stats").textContent = total ? `${mb(done)} of ${mb(total)} MB` : `${mb(done)} MB`;
});

// --- Folder in the cloud: a small folder browser ----------------------------------------------

async function browse(path: string) {
  const name = NAMES[provider];
  $("cf-browse-title").textContent = path ? `Folders in “${path}”` : `Folders in your ${name}`;
  const list = $("cf-list");
  list.replaceChildren();
  let dirs: string[] = [];
  try {
    dirs = await invoke<string[]>("cloud_folders", { provider, path });
  } catch (e) {
    $("cf-browse-title").textContent = `Couldn't list the folders: ${e}`;
  }
  const row = (glyph: string, text: string, go: string) => {
    const li = document.createElement("li");
    li.innerHTML = `<button class="folder-row"><span class="icon">${glyph}</span><span></span></button>`;
    li.querySelector("span:last-child")!.textContent = text;
    li.querySelector("button")!.addEventListener("click", () => {
      input("cf-path").value = go;
      hideAll("cf-warning");
      otherFolderOk = false;
      void browse(go);
    });
    return li;
  };
  if (path) list.append(row(glyphs.up, "Up", path.split("/").slice(0, -1).join("/")));
  list.append(...dirs.map((d) => row(glyphs.folderClosed, d, path ? `${path}/${d}` : d)));
  if (!dirs.length && !path) $("cf-browse-title").textContent = `No folders in your ${name} yet.`;
}

input("cf-path").addEventListener("input", () => {
  otherFolderOk = false;
  hideAll("cf-warning");
});

// --- Folder on this PC ---------------------------------------------------------------------

$("lf-browse").addEventListener("click", async () => {
  const picked = await open({ directory: true, defaultPath: input("lf-path").value || undefined, title: "Notes folder on this PC" });
  if (typeof picked === "string") {
    input("lf-path").value = picked;
    hideAll("lf-error");
  }
});

// --- Test run --------------------------------------------------------------------------------

/** The lines of the test run that matter here: the files, warnings and errors */
const TEST_LINES = new Set(["wouldCopy", "wouldDelete", "warning", "error"]);

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

async function startTest() {
  testing = true;
  testOk = false;
  setBusy(true);
  next.textContent = "Sync now";
  hideAll("tr-result", "tr-lines");
  $("tr-lines").replaceChildren();
  showTestProgress("Starting…", null);
  try {
    await invoke("sync", { dryRun: true, resync: false });
  } catch (e) {
    endTest();
    testResult("critical", "Test run failed", String(e));
  }
}

function endTest() {
  testing = false;
  setBusy(false);
  $("tr-progress").hidden = true;
}

function showTestProgress(text: string, percent: number | null) {
  $("tr-progress").hidden = false;
  $("tr-progress").classList.toggle("unknown", percent === null);
  $("tr-fill").style.setProperty("--done", `${percent ?? 0}%`);
  $("tr-stats").textContent = text;
}

function testResult(severity: "success" | "critical", title: string, text: string) {
  $("tr-result").className = `infobar ${severity}`;
  $("tr-badge").className = `badge ${severity}`;
  $("tr-title").textContent = title;
  $("tr-text").textContent = text;
  $("tr-result").hidden = false;
  if (severity === "critical") next.textContent = "Try again";
}

void listen<SyncEvent>("sync-event", ({ payload: e }) => {
  if (!testing) return;
  if (e.type === "waiting") showTestProgress("Waiting for another sync to finish…", null);
  else if (e.type === "started") showTestProgress("Listing files…", null);
  else if (e.type === "progress") showTestProgress(e.text, e.percent);
  else if (e.type === "line" && TEST_LINES.has(e.kind)) {
    const li = document.createElement("li");
    li.className = e.kind;
    // "Ideas.md: Skipped copy as --dry-run is set (size 8)" → "Copy: Ideas.md"
    const file = /^(.*): Skipped (copy|delete) as --dry-run is set/.exec(e.text);
    li.textContent = file ? `${file[2] === "copy" ? "Copy" : "Delete"}: ${file[1]}` : e.text;
    $("tr-lines").append(li);
    $("tr-lines").hidden = false;
  }
});

void listen<Finished>("sync-finished", ({ payload: f }) => {
  if (!testing) return;
  endTest();
  if (f.summary) {
    const { copied, deleted, warnings } = f.summary;
    const changes = [copied && `copy ${plural(copied, "file")}`, deleted && `delete ${plural(deleted, "file")}`].filter(Boolean).join(" and ");
    const would = changes ? `The sync would ${changes}.` : "Both sides are already the same.";
    const warned = warnings ? ` ${plural(warnings, "warning")}: see the list.` : "";
    testOk = true;
    testResult("success", "Test run done", `${would}${warned} Nothing was changed. If that looks right, click Sync now.`);
  } else {
    testResult("critical", "Test run failed", f.error ?? "");
  }
});

// --- Navigation --------------------------------------------------------------------------------

next.addEventListener("click", () => void goNext());
back.addEventListener("click", () => {
  if (!busy && page > 0) void show(page - 1);
});
$("wz-cancel").addEventListener("click", () => {
  void invoke("cancel_sign_in");
  finish(null);
});
// After saving; a test run that still runs goes on in the main window
$("wz-close").addEventListener("click", () => finish("close"));

function finish(ending: Ending) {
  testing = false;
  busy = false;
  $("wizard").hidden = true;
  onClose(ending);
}
// Enter in a text box: the page's main action
$("wizard").addEventListener("keydown", (e) => {
  if (e.key !== "Enter" || !(e.target instanceof HTMLInputElement) || e.target.type === "checkbox" || e.target.type === "radio") return;
  e.preventDefault();
  if (e.target.id === "si-server") $("si-nc-browser").click();
  else if (e.target.id === "si-user" || e.target.id === "si-pass") $("si-nc-password").click();
  else if (e.target.id.startsWith("si-sftp-")) $("si-sftp-connect").click();
  else void goNext();
});
