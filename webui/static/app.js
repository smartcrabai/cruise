(() => {
  "use strict";

  const STORAGE_KEY = "cruise-sidebar-width";
  const DEFAULT_WIDTH = 288;
  const MIN_WIDTH = 180;
  const MAX_WIDTH = 600;
  const RESIZE_STEP = 16;
  // Mirrors run_all_state::LOG_CAP so the live log DOM cannot grow without bound.
  const RUN_ALL_LOG_CAP = 2000;

  let mermaidImport;
  let mermaidInitialized = false;
  let mermaidQueue = Promise.resolve();
  let scrollState;

  const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

  function readStoredWidth() {
    try {
      const value = Number.parseInt(window.localStorage.getItem(STORAGE_KEY), 10);
      return Number.isNaN(value) ? DEFAULT_WIDTH : clamp(value, MIN_WIDTH, MAX_WIDTH);
    } catch {
      return DEFAULT_WIDTH;
    }
  }

  function saveWidth(width) {
    try {
      window.localStorage.setItem(STORAGE_KEY, String(width));
    } catch {
      // localStorage can be unavailable in restricted browser contexts.
    }
  }

  function setSidebarWidth(width, persist = true) {
    const sidebar = document.getElementById("sidebar");
    if (!sidebar) return null;
    const next = clamp(Math.round(width), MIN_WIDTH, MAX_WIDTH);
    sidebar.style.width = `${next}px`;
    if (persist) saveWidth(next);
    return next;
  }

  function setupSidebarResize() {
    const resizer = document.getElementById("sidebar-resizer");
    const sidebar = document.getElementById("sidebar");
    if (!resizer || !sidebar) return;

    setSidebarWidth(readStoredWidth(), false);

    let resizing = false;
    let startX = 0;
    let startWidth = 0;
    let currentWidth = 0;

    resizer.addEventListener("mousedown", (event) => {
      event.preventDefault();
      resizing = true;
      startX = event.clientX;
      startWidth = sidebar.getBoundingClientRect().width || sidebar.offsetWidth || DEFAULT_WIDTH;
      currentWidth = setSidebarWidth(startWidth, false) || DEFAULT_WIDTH;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
    });

    const stopResizing = () => {
      if (!resizing) return;
      resizing = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      saveWidth(currentWidth);
    };

    document.addEventListener("mousemove", (event) => {
      if (!resizing) return;
      currentWidth = setSidebarWidth(startWidth + event.clientX - startX, false) || currentWidth;
    });
    document.addEventListener("mouseup", stopResizing);

    resizer.addEventListener("keydown", (event) => {
      const direction = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0;
      if (!direction) return;
      event.preventDefault();
      const width = sidebar.getBoundingClientRect().width || sidebar.offsetWidth || readStoredWidth();
      setSidebarWidth(width + direction * RESIZE_STEP);
    });
  }

  function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
  }

  function loadMermaid() {
    if (!mermaidImport) {
      mermaidImport = import("https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs").then(
        (module) => module.default || module,
      );
    }
    return mermaidImport;
  }

  function scanMermaid() {
    const nodes = Array.from(document.querySelectorAll("pre.mermaid:not([data-mermaid-queued])"));
    if (!nodes.length) return;
    nodes.forEach((node) => node.setAttribute("data-mermaid-queued", "true"));

    mermaidQueue = mermaidQueue.then(async () => {
      try {
        const mermaid = await loadMermaid();
        if (!mermaidInitialized) {
          const dark = typeof window.matchMedia === "function" &&
            window.matchMedia("(prefers-color-scheme: dark)").matches;
          mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
          mermaidInitialized = true;
        }
        await mermaid.run({ nodes });
      } catch (error) {
        const message = errorMessage(error);
        nodes.forEach((node) => {
          node.textContent = `Failed to render Graph: ${message}`;
        });
      }
    });
  }

  function rememberScrollPositions() {
    const elements = [];
    const runAllLog = document.getElementById("run-all-log");
    if (runAllLog) elements.push(runAllLog);
    document.querySelectorAll('pre[id^="tab-log-"]').forEach((element) => elements.push(element));
    scrollState = elements.map((element) => ({
      id: element.id,
      element,
      stuck: element.scrollHeight - element.scrollTop - element.clientHeight < 24,
    }));
  }

  function restoreScrollPositions() {
    if (!scrollState) return;
    scrollState.forEach(({ id, element, stuck }) => {
      if (!stuck) return;
      const current = document.getElementById(id) || element;
      if (current) current.scrollTop = current.scrollHeight;
    });
    scrollState = undefined;
  }

  function trimRunAllLog() {
    const log = document.getElementById("run-all-log");
    if (!log) return;
    while (log.childElementCount > RUN_ALL_LOG_CAP) {
      log.removeChild(log.firstElementChild);
    }
  }

  // grill conflicts with both skipPlanning (silently dropped) and
  // noInteractivePlanning (server-side error), matching the CLI's conflicts_with.
  function syncGrillAvailability() {
    document.querySelectorAll('input[name="grill"]').forEach((grill) => {
      const scope = grill.form || document;
      const blocked = Array.from(
        scope.querySelectorAll('input[name="skipPlanning"], input[name="noInteractivePlanning"]'),
      ).some((input) => input.checked);
      grill.disabled = blocked;
      if (blocked) grill.checked = false;
    });
  }

  function setupToasts() {
    const toasts = document.getElementById("toasts");
    if (!toasts) return;
    const observeAdded = (node) => {
      if (!(node instanceof Element)) return;
      window.setTimeout(() => {
        if (node.isConnected) node.remove();
      }, 6000);
      node.addEventListener("click", () => node.remove());
    };
    const observer = new MutationObserver((mutations) => {
      mutations.forEach((mutation) => mutation.addedNodes.forEach(observeAdded));
    });
    observer.observe(toasts, { childList: true });
  }

  function setupNotifications() {
    if (typeof Notification === "undefined") return;
    if (Notification.permission === "default") {
      document.addEventListener("pointerdown", () => {
        if (Notification.permission !== "default") return;
        try {
          const request = Notification.requestPermission();
          if (request && typeof request.catch === "function") request.catch(() => {});
        } catch {
          // Notification permissions may be unavailable in an embedded context.
        }
      }, { once: true });
    }
  }

  function notify(event) {
    if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
    let data = event && event.detail ? event.detail.data : undefined;
    if (typeof data === "string") {
      try {
        data = JSON.parse(data);
      } catch {
        return;
      }
    }
    if (!data || typeof data !== "object") return;
    try {
      new Notification(data.title, { body: data.body });
    } catch {
      // Notification construction can fail when permission is revoked mid-request.
    }
  }

  function toggleSource(mode) {
    const directory = document.getElementById("source-directory");
    const repo = document.getElementById("source-repo");
    if (directory) directory.classList.toggle("hidden", mode !== "directory");
    if (repo) repo.classList.toggle("hidden", mode === "directory");
  }

  function pickDir(path) {
    if (typeof path !== "string") return;
    const input = document.querySelector('input[name="baseDir"]');
    const suggestions = document.getElementById("dir-suggestions");
    if (!input) return;
    input.value = path.endsWith("/") ? path : `${path}/`;
    if (suggestions) suggestions.innerHTML = "";
    input.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function setup() {
    setupSidebarResize();
    setupToasts();
    setupNotifications();
    document.addEventListener("htmx:before:swap", rememberScrollPositions);
    document.addEventListener("htmx:after:swap", () => {
      trimRunAllLog();
      restoreScrollPositions();
      scanMermaid();
      syncGrillAvailability();
    });
    document.addEventListener("htmx:after:settle", scanMermaid);
    document.addEventListener("change", (event) => {
      const target = event.target;
      if (!target || (target.name !== "skipPlanning" && target.name !== "noInteractivePlanning")) return;
      syncGrillAvailability();
    });
    scanMermaid();
    syncGrillAvailability();
  }

  window.app = { notify, toggleSource, pickDir };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", setup, { once: true });
  } else {
    setup();
  }

  // Tab strip: the panel swaps on its own, so keep aria-selected in sync here.
  document.addEventListener("click", (event) => {
    const target = event.target;
    if (!target || typeof target.closest !== "function") return;
    const tab = target.closest('[role="tab"]');
    if (!tab) return;
    const strip = tab.closest('[role="tablist"]');
    if (!strip) return;
    for (const sibling of strip.querySelectorAll('[role="tab"]')) {
      sibling.setAttribute("aria-selected", sibling === tab ? "true" : "false");
    }
  }, true);

})();
