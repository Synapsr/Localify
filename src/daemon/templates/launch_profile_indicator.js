// Localify launch profile indicator.
//
// Served by the Localify proxy on the app's own origin and added to HTML pages
// of services launched with a dangerous launch profile. It prefixes the tab
// title and shows a small draggable pill. Styles are set through the CSSOM
// inside a shadow root, so a strict `style-src` CSP does not block them and the
// app's CSS cannot reach them. It waits for `load`, after frameworks hydrate.
(() => {
  if (window.__localifyLaunchProfile) return;

  const script = document.currentScript;
  const profile = (script && script.dataset.profile) || "profile";
  window.__localifyLaunchProfile = profile;

  const label = profile.toUpperCase();
  const titlePrefix = `[${label}] `;
  const storageKey = "localify:launch-profile-indicator";
  const margin = 12;

  const setStyles = (element, styles) => {
    for (const [name, value] of Object.entries(styles)) {
      element.style.setProperty(name, value);
    }
  };

  const prefixTitle = () => {
    if (document.title && !document.title.startsWith(titlePrefix)) {
      document.title = titlePrefix + document.title;
    }
  };

  const host = document.createElement("localify-launch-profile");
  const pill = document.createElement("div");

  const buildPill = () => {
    const root = host.attachShadow({ mode: "open" });
    const dot = document.createElement("span");
    const text = document.createElement("span");
    text.textContent = label;

    pill.setAttribute("role", "status");
    pill.setAttribute("aria-label", `Localify: running on the ${profile} launch profile`);
    pill.title = `Localify: this app runs on the "${profile}" launch profile. Drag to move.`;
    pill.append(dot, text);
    root.append(pill);

    setStyles(host, {
      position: "fixed",
      "z-index": "2147483647",
      left: "0px",
      top: "0px",
    });
    setStyles(pill, {
      display: "inline-flex",
      "align-items": "center",
      gap: "6px",
      padding: "4px 10px",
      "border-radius": "999px",
      background: "#dc2626",
      color: "#ffffff",
      font: "600 11px/1.4 ui-sans-serif, system-ui, -apple-system, sans-serif",
      "letter-spacing": "0.04em",
      "box-shadow": "0 2px 10px rgba(0, 0, 0, 0.25), 0 0 0 1px rgba(255, 255, 255, 0.25) inset",
      cursor: "grab",
      "user-select": "none",
      "-webkit-user-select": "none",
      "touch-action": "none",
    });
    setStyles(dot, {
      width: "6px",
      height: "6px",
      "border-radius": "999px",
      background: "#ffffff",
    });
  };

  const clampPosition = (x, y) => {
    const width = pill.offsetWidth || 60;
    const height = pill.offsetHeight || 22;
    return {
      x: Math.min(Math.max(margin, x), Math.max(margin, window.innerWidth - width - margin)),
      y: Math.min(Math.max(margin, y), Math.max(margin, window.innerHeight - height - margin)),
    };
  };

  const moveTo = (x, y) => {
    const position = clampPosition(x, y);
    host.style.setProperty("left", `${position.x}px`);
    host.style.setProperty("top", `${position.y}px`);
    return position;
  };

  const savedPosition = () => {
    try {
      const saved = JSON.parse(localStorage.getItem(storageKey) || "null");
      return saved && Number.isFinite(saved.x) && Number.isFinite(saved.y) ? saved : null;
    } catch {
      return null;
    }
  };

  const savePosition = (position) => {
    try {
      localStorage.setItem(storageKey, JSON.stringify(position));
    } catch {
      // Storage can be unavailable (private mode, sandboxed frames).
    }
  };

  const placeInitially = () => {
    const saved = savedPosition();
    if (saved) {
      moveTo(saved.x, saved.y);
      return;
    }
    moveTo((window.innerWidth - pill.offsetWidth) / 2, window.innerHeight - pill.offsetHeight - 16);
  };

  const enableDragging = () => {
    let drag = null;

    pill.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      const box = host.getBoundingClientRect();
      drag = { dx: event.clientX - box.left, dy: event.clientY - box.top, moved: false };
      try {
        pill.setPointerCapture(event.pointerId);
      } catch {
        // Synthetic or already released pointers cannot be captured.
      }
      pill.style.setProperty("cursor", "grabbing");
    });
    pill.addEventListener("pointermove", (event) => {
      if (!drag) return;
      drag.moved = true;
      moveTo(event.clientX - drag.dx, event.clientY - drag.dy);
    });
    const endDrag = (event) => {
      if (!drag) return;
      if (drag.moved) {
        savePosition(moveTo(event.clientX - drag.dx, event.clientY - drag.dy));
      }
      drag = null;
      pill.style.setProperty("cursor", "grab");
    };
    pill.addEventListener("pointerup", endDrag);
    pill.addEventListener("pointercancel", endDrag);

    window.addEventListener("resize", () => {
      const box = host.getBoundingClientRect();
      moveTo(box.left, box.top);
    });
  };

  const mount = () => {
    if (!document.body) return;
    if (!host.isConnected) {
      document.body.append(host);
    }
  };

  const start = () => {
    buildPill();
    mount();
    placeInitially();
    enableDragging();

    prefixTitle();
    new MutationObserver(prefixTitle).observe(document.head || document.documentElement, {
      subtree: true,
      childList: true,
      characterData: true,
    });

    // Frameworks may re-render <body> after a hydration error; put the pill back.
    window.setInterval(mount, 1000);
  };

  if (document.readyState === "complete") {
    window.setTimeout(start, 0);
  } else {
    window.addEventListener("load", () => window.setTimeout(start, 0), { once: true });
  }
})();
