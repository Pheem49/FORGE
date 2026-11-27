(() => {
  "use strict";
  const TOKEN = __FORGE_TOKEN__;
  const RUNTIME = __FORGE_RUNTIME__;
  const WINDOW_LABEL = __FORGE_WINDOW__;
  const SMOKE = __FORGE_SMOKE__;
  const listeners = new Map();

  function dispatch(event, payload) {
    if (event === "__forge:css-reload") {
      const stamp = Date.now();
      document.querySelectorAll('link[rel="stylesheet"]').forEach((link) => {
        const href = link.getAttribute("href");
        if (href) {
          const clean = href.split("?")[0];
          link.setAttribute("href", clean + "?__forge_t=" + stamp);
        }
      });
      return;
    }
    if (event === "__forge:reload") {
      window.location.reload();
      return;
    }
    const set = listeners.get(event);
    if (!set) return;
    for (const fn of Array.from(set)) {
      try {
        fn(payload);
      } catch (err) {
        console.error("[FORGE] listener error on", event, err);
      }
    }
  }

  function listen(event, handler) {
    if (typeof event !== "string" || typeof handler !== "function") {
      throw new TypeError("forge.listen(event, handler) requires a string and a function");
    }
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
      if (set.size === 0) listeners.delete(event);
    };
  }

  function once(event, handler) {
    const unlisten = listen(event, (payload) => {
      unlisten();
      handler(payload);
    });
    return unlisten;
  }

  function isBinaryBuffer(val) {
    return (
      val instanceof Uint8Array ||
      val instanceof ArrayBuffer ||
      (typeof ArrayBuffer !== "undefined" && ArrayBuffer.isView(val))
    );
  }

  async function invoke(command, payload, meta) {
    if (typeof command !== "string" || command.length === 0) {
      const err = new Error("Command name must be a non-empty string");
      err.code = "FORGE_INVALID_COMMAND";
      throw err;
    }
    const binary = isBinaryBuffer(payload);
    const headers = {
      "Content-Type": binary ? "application/octet-stream" : "application/json",
      "X-Forge-Token": TOKEN
    };
    if (meta !== undefined) {
      headers["X-Forge-Meta"] = "uri:" + encodeURIComponent(JSON.stringify(meta));
    }
    const body = binary
      ? payload
      : JSON.stringify(payload === undefined ? null : payload);

    const res = await fetch("/__forge_ipc/" + encodeURIComponent(command), {
      method: "POST",
      headers,
      body
    });

    if (!res.ok) {
      const data = await res.json().catch(() => null);
      const message = (data && data.message) || ("IPC failed with status " + res.status);
      const err = new Error(message);
      err.code = (data && data.code) || "APP_ERROR";
      if (data && data.details !== undefined) {
        err.details = data.details;
      }
      throw err;
    }

    const ct = (res.headers.get("content-type") || "").toLowerCase();
    if (ct.startsWith("application/octet-stream")) {
      const buf = await res.arrayBuffer();
      return new Uint8Array(buf);
    }
    return res.json().catch(() => null);
  }

  function invokeBytes(command, payload, meta) {
    return invoke(command, payload, meta);
  }

  function emit(event, payload) {
    return invoke("event.emit_all", { event, payload: payload === undefined ? null : payload });
  }

  function emitTo(target, event, payload) {
    return invoke("event.emit_to", {
      target,
      event,
      payload: payload === undefined ? null : payload
    });
  }

  const windowApi = Object.freeze({
    label: WINDOW_LABEL,
    close: () => invoke("window.close"),
    destroy: () => invoke("window.destroy"),
    minimize: (minimized = true) => invoke("window.minimize", { minimized }),
    maximize: (maximized) => invoke("window.maximize", maximized === undefined ? null : { maximized }),
    toggleFullscreen: (fullscreen) =>
      invoke("window.toggle_fullscreen", fullscreen === undefined ? null : { fullscreen }),
    setAlwaysOnTop: (always_on_top = true) => invoke("window.set_always_on_top", { always_on_top }),
    setOnClose: (on_close) => invoke("window.set_on_close", { on_close }),
    setTitle: (title) => invoke("window.set_title", { title }),
    show: () => invoke("window.show"),
    hide: () => invoke("window.hide"),
    focus: () => invoke("window.focus"),
    startDragging: () => invoke("window.start_dragging"),
    create: (opts) => invoke("window.create", opts),
    list: () => invoke("window.list")
  });

  function installUnifiedDragRegions() {
    if (RUNTIME === "cef") {
      const style = document.createElement("style");
      style.setAttribute("data-forge-drag-style", "");
      style.textContent =
        "[data-forge-drag-region] { -webkit-app-region: drag; app-region: drag; user-select: none; }\n" +
        "[data-forge-no-drag], [data-forge-drag-region] button, [data-forge-drag-region] input, " +
        "[data-forge-drag-region] a, [data-forge-drag-region] select, [data-forge-drag-region] textarea { " +
        "-webkit-app-region: no-drag; app-region: no-drag; }";
      (document.head || document.documentElement).appendChild(style);
    } else {
      document.addEventListener("mousedown", (ev) => {
        if (ev.button !== 0 || ev.detail !== 1) return;
        const target = ev.target;
        if (!(target instanceof Element)) return;
        if (!target.closest("[data-forge-drag-region]")) return;
        if (target.closest("button, input, a, select, textarea, [contenteditable='true'], [data-forge-no-drag]")) {
          return;
        }
        windowApi.startDragging().catch(() => {});
      });
    }
  }

  Object.defineProperty(window, "__forge_dispatch", {
    value: dispatch,
    writable: false,
    configurable: false,
    enumerable: false
  });

  Object.defineProperty(window, "forge", {
    value: Object.freeze({
      runtime: RUNTIME,
      window: windowApi,
      invoke,
      invokeBytes,
      listen,
      once,
      emit,
      emitTo
    }),
    writable: false,
    configurable: false,
    enumerable: true
  });

  window.addEventListener("DOMContentLoaded", () => {
    installUnifiedDragRegions();
    invoke("__forge_drain_events").then((queued) => {
      if (Array.isArray(queued)) {
        for (const [ev, payload] of queued) {
          dispatch(ev, payload);
        }
      }
      if (SMOKE) {
        return invoke("__forge_ready", { ua: navigator.userAgent });
      }
    }).catch((err) => {
      console.error("[FORGE] bootstrap drain/smoke error:", err);
    });
  }, { once: true });
})();
