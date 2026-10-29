import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyLook, fillIcons, showAppIcon } from "./look";
import { openWizard, type Ending } from "./wizard";

// Mirrors `Status`, `Begin` and `Finished` in src-tauri/src/lib.rs
interface Status {
  version: string;
  folder: string;
  local: string | null;
  remote: string | null;
  problem: string | null;
  lastSync: string | null;
  failed: string | null;
  running: boolean;
  accent: string[] | null;
  mica: boolean;
}

interface Begin {
  dryRun: boolean;
  resync: boolean;
}

interface Summary {
  dryRun: boolean;
  resync: boolean;
  copied: number;
  deleted: number;
  warnings: number;
}

interface Finished {
  summary: Summary | null;
  error: string | null;
  needsResync: boolean;
  leftLock: boolean;
}

// Mirrors `Event` in sync-core/src/runner.rs
type SyncEvent =
  | { type: "waiting" }
  | { type: "started"; local: string; remote: string; dryRun: boolean; resync: boolean }
  | { type: "backup"; path: string }
  | { type: "backupFailed"; message: string }
  | { type: "line"; time: string | null; text: string; kind: string }
  | { type: "progress"; text: string; percent: number | null };

const MAX_LINES = 3000;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const statusBadge = $("status-badge");
const statusRing = document.getElementById("status-ring") as unknown as SVGElement;
const lines = $<HTMLOListElement>("lines");
const syncNow = $<HTMLButtonElement>("sync-now");

let running = false;
let currentDryRun = false;
let problem: string | null = null;

/** Header status: a coloured badge, or a progress ring while syncing. */
function setStatus(state: "idle" | "never" | "error" | "syncing") {
  statusRing.toggleAttribute("hidden", state !== "syncing");
  statusBadge.hidden = state === "syncing";
  statusBadge.className = `badge ${state === "idle" ? "success" : state === "error" ? "critical" : "neutral"}`;
}

// --- Status --------------------------------------------------------------------

async function refresh() {
  const s = await invoke<Status>("status");
  applyLook(s.accent, s.mica);
  $("local").textContent = s.local ?? "–";
  $("local").title = s.local ?? "";
  $("remote").textContent = s.remote ?? "–";
  $("remote").title = s.remote ?? "";
  $("footer").textContent = `md-notes-sync ${s.version} · ${s.folder}`;
  problem = s.problem;
  $("problem").hidden = !problem;
  $("problem-text").textContent = problem ?? "";
  running = s.running;
  syncNow.disabled = running || !!problem;
  // Not set up: the setup opens by itself, once
  if (problem && !setupOffered) {
    setupOffered = true;
    startSetup();
  } else if (!inSetup) {
    $("home").hidden = false;
  }
  if (running) return;
  if (problem) {
    setSubtitle("Not set up yet");
    setStatus("error");
  } else if (s.failed) {
    setSubtitle(s.lastSync ? `Last sync failed · last good sync ${s.lastSync}` : "Last sync failed");
    setStatus("error");
  } else if (s.lastSync) {
    setSubtitle(`Last sync ${s.lastSync}`);
    setStatus("idle");
  } else {
    setSubtitle("Not synced yet");
    setStatus("never");
  }
}

function setSubtitle(text: string) {
  $("subtitle").textContent = text;
}

// --- Output --------------------------------------------------------------------

function addLine(text: string, kind: string, time: string | null = null) {
  const atBottom = lines.scrollHeight - lines.scrollTop - lines.clientHeight < 40;
  const li = document.createElement("li");
  li.className = kind;
  const t = document.createElement("time");
  t.textContent = time ?? "";
  li.append(t, text);
  lines.append(li);
  while (lines.childElementCount > MAX_LINES) lines.firstElementChild?.remove();
  if (atBottom) lines.scrollTop = lines.scrollHeight;
}

function now() {
  return new Date().toTimeString().slice(0, 8);
}

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

function showProgress(text: string, percent: number | null) {
  const box = $("progress");
  box.hidden = false;
  box.classList.toggle("unknown", percent === null);
  $("fill").style.setProperty("--done", `${percent ?? 0}%`);
  $("stats").textContent = text;
}

function onBegin(b: Begin) {
  running = true;
  currentDryRun = b.dryRun;
  syncNow.disabled = true;
  $("run").hidden = false;
  $("result").hidden = true;
  lines.replaceChildren();
  showProgress("Starting…", null);
  setSubtitle(b.dryRun ? "Test run…" : "Syncing…");
  setStatus("syncing");
}

function onEvent(e: SyncEvent) {
  switch (e.type) {
    case "waiting":
      showProgress("Waiting for another sync to finish…", null);
      addLine("Another sync is running. Waiting up to a minute…", "dim", now());
      break;
    case "started": {
      const flags = `${e.dryRun ? " (test run)" : ""}${e.resync ? " (resync)" : ""}`;
      addLine(`Syncing ${e.local} ↔ ${e.remote}${flags}`, "start", now());
      showProgress("Listing files…", null);
      break;
    }
    case "backup":
      addLine(`Backup: ${e.path}`, "dim", now());
      break;
    case "backupFailed":
      addLine(`Backup failed: ${e.message}`, "warning", now());
      break;
    case "line":
      addLine(e.text, e.kind, e.time);
      break;
    case "progress":
      showProgress(e.text, e.percent);
      break;
  }
}

/** The result card under the buttons, with its appear animation. */
function showResult(severity: "success" | "info" | "critical", title: string, text: string) {
  // The card takes room from the log: keep its last lines in view
  const atBottom = lines.scrollHeight - lines.scrollTop - lines.clientHeight < 40;
  requestAnimationFrame(() => {
    if (atBottom) lines.scrollTop = lines.scrollHeight;
  });
  const box = $("result");
  box.hidden = false;
  box.className = `infobar ${severity}`;
  $("result-badge").className = `badge ${severity}`;
  $("result-title").textContent = title;
  $("result-text").textContent = text;
  $("resync").hidden = true;
  $("unlock").hidden = true;
  // Restart the appear animation
  box.style.animation = "none";
  void box.offsetWidth;
  box.style.animation = "";
}

function onFinished(f: Finished) {
  running = false;
  $("progress").hidden = true;
  if (f.summary) {
    const s = f.summary;
    const warnings = plural(s.warnings, "warning");
    if (s.dryRun) {
      showResult("info", "Test run done", `Would copy ${s.copied}, delete ${s.deleted}. ${warnings}. Nothing was changed.`);
    } else {
      showResult("success", s.resync ? "Synced (resync)" : "Synced", `${s.copied} copied, ${s.deleted} deleted, ${warnings}.`);
    }
    addLine(currentDryRun ? "Test run OK" : "Sync OK", "success", now());
  } else {
    showResult(
      "critical",
      currentDryRun ? "Test run failed" : "Sync failed",
      f.needsResync ? `${f.error} A resync merges both sides: the newer file wins, nothing is deleted.` : (f.error ?? ""),
    );
    $("resync").hidden = !f.needsResync;
    $("unlock").hidden = !f.leftLock;
    addLine(`ERROR: ${f.error}`, "error", now());
  }
  void refresh();
}

// --- Back up now (tray menu) -----------------------------------------------------

// Mirrors `Progress` in sync-core/src/backup.rs and `BackupFinished` in src-tauri/src/lib.rs
interface BackupProgress {
  files: number;
  total: number;
  bytes: number;
}

interface BackupFinished {
  path: string | null;
  size: number;
  error: string | null;
}

const mb = (bytes: number) => (bytes / 1_000_000).toFixed(1);

function onBackupBegin(notes: string) {
  running = true;
  syncNow.disabled = true;
  $("run").hidden = false;
  $("result").hidden = true;
  lines.replaceChildren();
  addLine(`Backing up ${notes}`, "start", now());
  showProgress("Counting files…", null);
  setSubtitle("Backing up…");
  setStatus("syncing");
}

function onBackupProgress(p: BackupProgress) {
  const percent = p.total ? Math.round((p.files / p.total) * 100) : null;
  showProgress(`${p.files} of ${plural(p.total, "file")} · ${mb(p.bytes)} MB`, percent);
}

function onBackupFinished(f: BackupFinished) {
  running = false;
  $("progress").hidden = true;
  if (f.path) {
    const name = f.path.split("\\").pop();
    addLine(`Backup made: ${f.path} (${mb(f.size)} MB)`, "success", now());
    showResult("success", "Backup made", `${name}, ${mb(f.size)} MB. Backups opens the folder.`);
  } else {
    addLine(`ERROR: ${f.error}`, "error", now());
    showResult("critical", "Backup failed", f.error ?? "");
  }
  void refresh();
}

// --- Actions -------------------------------------------------------------------

let messageTimer = 0;

/** A message over the window for a few seconds: an error, or a note after the setup. */
function showMessage(text: string, severity: "critical" | "caution" | "info" = "critical") {
  $("message").className = `message ${severity}`;
  $("message-badge").className = `badge ${severity}`;
  $("message-text").textContent = text;
  $("message").hidden = false;
  window.clearTimeout(messageTimer);
  messageTimer = window.setTimeout(() => ($("message").hidden = true), severity === "critical" ? 5000 : 9000);
}

// --- Setup ---------------------------------------------------------------------

let inSetup = false;
/** The setup opened by itself once (when not set up); after that only on request */
let setupOffered = false;

function startSetup() {
  if (inSetup) return;
  inSetup = true;
  $("home").hidden = true;
  void openWizard(closeSetup, !problem);
}

function closeSetup(ending: Ending) {
  inSetup = false;
  $("home").hidden = false;
  // The setup showed its test run; one that still runs stays in view
  if (ending && !running) $("run").hidden = true;
  void refresh();
  if (ending === "sync") void call("sync", { dryRun: false, resync: false });
}

async function call(command: string, args?: Record<string, unknown>) {
  try {
    await invoke(command, args);
  } catch (e) {
    showMessage(String(e));
  }
}

$("sync-now").addEventListener("click", () => call("sync", { dryRun: false, resync: false }));
$("resync").addEventListener("click", () => call("sync", { dryRun: false, resync: true }));
$("unlock").addEventListener("click", () => call("unlock_and_sync"));
$("open-notes").addEventListener("click", () => call("open_notes"));
$("open-log").addEventListener("click", () => call("open_log"));
$("open-backups").addEventListener("click", () => call("open_backups"));
$("change-setup").addEventListener("click", startSetup);
$("problem-setup").addEventListener("click", startSetup);
window.addEventListener("app-message", (e) => showMessage(String((e as CustomEvent).detail)));

window.addEventListener("keydown", (e) => {
  // Esc hides the window to the tray, like the close button
  if (e.key === "Escape") void call("hide_window");
});

fillIcons();
showAppIcon();
void listen("open-setup", startSetup);
void listen<Begin>("sync-begin", (e) => onBegin(e.payload));
void listen<SyncEvent>("sync-event", (e) => onEvent(e.payload));
void listen<Finished>("sync-finished", (e) => onFinished(e.payload));
void listen<string>("app-error", (e) => showMessage(e.payload));
void listen<string>("backup-begin", (e) => onBackupBegin(e.payload));
void listen<BackupProgress>("backup-progress", (e) => onBackupProgress(e.payload));
void listen<BackupFinished>("backup-finished", (e) => onBackupFinished(e.payload));
void refresh();
// Keeps "Last sync 5 min ago" current
window.setInterval(() => {
  if (!running) void refresh();
}, 30_000);
