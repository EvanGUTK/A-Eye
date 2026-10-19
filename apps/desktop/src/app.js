"use strict";

const POLL_INTERVAL_MS = 2000;
const NATIVE_REPLY_TIMEOUT_MS = 30_000;
// The native model request is bounded at 120 s; wait slightly longer so its own error wins.
const DESCRIBE_REPLY_TIMEOUT_MS = 130_000;
const EMPTY_PREVIEW_MESSAGE = "No frame captured.";

function monitorFingerprint(monitor) {
  return JSON.stringify([
    monitor.id, monitor.device, monitor.x, monitor.y,
    monitor.width, monitor.height, monitor.scale_percent,
  ]);
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function isLocalPreview(preview, selectedId) {
  return preview && preview.monitor_id === selectedId
    && Number.isInteger(preview.capture_id) && preview.capture_id > 0
    && typeof preview.data_url === "string"
    && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(preview.data_url)
    && Number.isInteger(preview.width) && preview.width > 0
    && Number.isInteger(preview.height) && preview.height > 0;
}

// Native description text is untrusted model output; keep only what the view renders.
function parseDescription(result, captureId) {
  const matches = Boolean(result) && result.capture_id === captureId;
  if (matches && typeof result.text === "string" && result.text.length > 0 && !result.error) {
    return {
      status: "done", captureId, text: result.text,
      model: typeof result.model === "string" && result.model ? result.model : null,
      elapsedMs: Number.isFinite(result.elapsed_ms) ? result.elapsed_ms : null,
    };
  }
  if (matches && typeof result.error === "string" && result.error.length > 0) {
    return { status: "error", captureId, error: result.error };
  }
  return { status: "error", captureId, error: "The native host returned an invalid description response." };
}

function createNativeInvoker(core, timers = globalThis) {
  return (command, args) => new Promise((resolve, reject) => {
    const reply = new core.Channel();
    const timeoutMs = command === "describe_frame" ? DESCRIBE_REPLY_TIMEOUT_MS : NATIVE_REPLY_TIMEOUT_MS;
    let settled = false;
    let timeout = null;

    function finish(callback, value) {
      if (settled) return;
      settled = true;
      if (timeout !== null) timers.clearTimeout(timeout);
      reply.onmessage = () => {};
      callback(value);
    }

    reply.onmessage = (outcome) => finish(resolve, outcome);
    timeout = timers.setTimeout(() => {
      finish(reject, new Error(`The native host did not reply within ${timeoutMs / 1000} seconds.`));
    }, timeoutMs);
    try {
      // Enqueue now so a later stop cannot overtake this action at the native boundary.
      Promise.resolve(core.invoke(command, { ...args, reply }))
        .catch((error) => finish(reject, error));
    } catch (error) {
      finish(reject, error);
    }
  });
}

function createController(invoke, onChange = () => {}) {
  const pending = new Map();
  const state = {
    status: { connected: false, error: null, monitors: [], overlay: null },
    selectedId: null,
    selectionFingerprint: null,
    selectionInitialized: false,
    selectionMessage: "",
    error: null,
    transportUnknown: false,
    preview: null,
    previewToken: 0,
    previewMessage: EMPTY_PREVIEW_MESSAGE,
    description: null,
  };
  let requestNumber = 0;
  let lastAppliedRequest = 0;
  let lifecycle = 0;

  function view() {
    const busy = pending.size > 0;
    const connecting = [...pending.values()].includes("connect");
    const disconnecting = [...pending.values()].includes("disconnect");
    const hiding = [...pending.values()].includes("hide_overlay");
    const ready = state.status.connected && !state.transportUnknown;
    const describing = state.description?.status === "running";
    return {
      ...state, busy, connecting, disconnecting, describing,
      canConnect: !busy && (!state.status.connected || state.transportUnknown),
      canDisconnect: !disconnecting && (state.status.connected || connecting || state.transportUnknown),
      canSelect: ready && !busy && !state.status.overlay,
      canRefresh: ready && !busy,
      canShow: ready && !busy && Boolean(state.selectedId) && !state.status.overlay,
      canHide: !disconnecting && !hiding && (state.status.connected || connecting || state.transportUnknown),
      // Capturing again would orphan the running description; wait for it to finish.
      canCapture: ready && !busy && !describing && Boolean(state.selectedId),
      canDescribe: ready && !describing && Boolean(state.preview),
    };
  }

  function emit() { onChange(view()); }

  function clearPreview(message = EMPTY_PREVIEW_MESSAGE) {
    state.previewToken += 1;
    state.preview = null;
    // A description belongs to one frame; any result still in flight is discarded by its token.
    state.description = null;
    state.previewMessage = message;
  }

  function resetSelection() {
    state.selectedId = null;
    state.selectionFingerprint = null;
    state.selectionInitialized = false;
    state.selectionMessage = "";
  }

  function updateSelection() {
    const monitors = state.status.monitors;
    if (state.selectedId) {
      const selected = monitors.find((monitor) => monitor.id === state.selectedId);
      if (!selected || monitorFingerprint(selected) !== state.selectionFingerprint) {
        state.selectedId = null;
        state.selectionFingerprint = null;
        state.selectionMessage = "The selected display changed or disappeared. Choose a display again.";
        clearPreview();
      }
    } else if (!state.selectionInitialized && monitors.length > 0) {
      const initial = monitors.find((monitor) => monitor.primary) || monitors[0];
      state.selectedId = initial.id;
      state.selectionFingerprint = monitorFingerprint(initial);
      state.selectionInitialized = true;
    }
  }

  function applySnapshot(snapshot) {
    if (!snapshot || !snapshot.status || typeof snapshot.status.connected !== "boolean"
        || !Array.isArray(snapshot.status.monitors)) {
      throw new Error("The native host returned an invalid status response.");
    }
    state.status = snapshot.status;
    state.error = snapshot.status.error;
    state.transportUnknown = false;
    if (!snapshot.status.connected) {
      resetSelection();
      clearPreview();
    } else {
      updateSelection();
    }
  }

  async function perform(type) {
    const current = view();
    const allowed = {
      connect: current.canConnect, disconnect: current.canDisconnect,
      refresh_monitors: current.canRefresh, show_overlay: current.canShow,
      hide_overlay: current.canHide, capture: current.canCapture,
      status: !current.busy,
    };
    if (!allowed[type]) return false;

    if (type === "connect" || type === "disconnect") {
      lifecycle += 1;
      resetSelection();
      clearPreview();
    } else if (type === "capture") {
      clearPreview("Capturing one frame…");
    } else if (type === "hide_overlay") {
      // A local stop wins over an in-flight capture, including its image decode.
      clearPreview();
    }
    const requestLifecycle = lifecycle;
    const previewToken = state.previewToken;
    const selectedId = state.selectedId;
    const requestId = ++requestNumber;
    const action = { type };
    if (type === "capture" || type === "show_overlay") action.monitor_id = selectedId;
    pending.set(requestId, type);
    state.error = null;
    emit();

    try {
      const snapshot = await invoke("desktop_action", { action });
      if (requestLifecycle !== lifecycle || requestId < lastAppliedRequest) return false;
      lastAppliedRequest = requestId;
      applySnapshot(snapshot);
      if (type === "capture" && previewToken === state.previewToken && state.status.connected) {
        if (!state.error && isLocalPreview(snapshot.preview, state.selectedId)) {
          state.preview = { ...snapshot.preview, token: previewToken };
          state.previewMessage = "Decoding captured frame…";
        } else {
          clearPreview("Frame was not captured. Try again.");
          if (!state.error) state.error = "The capture did not return a valid PNG for the selected display.";
        }
      }
      return true;
    } catch (error) {
      if (requestLifecycle === lifecycle && requestId >= lastAppliedRequest) {
        lastAppliedRequest = requestId;
        state.error = `Native host request failed: ${errorMessage(error)}`;
        state.transportUnknown = true;
        clearPreview(type === "capture" ? "Frame was not captured. Try again." : EMPTY_PREVIEW_MESSAGE);
      }
      return false;
    } finally {
      pending.delete(requestId);
      emit();
    }
  }

  function selectMonitor(id) {
    if (!view().canSelect) return false;
    const monitor = state.status.monitors.find((item) => item.id === id);
    if (!monitor) return false;
    state.selectedId = id;
    state.selectionFingerprint = monitorFingerprint(monitor);
    state.selectionInitialized = true;
    state.selectionMessage = "";
    clearPreview();
    emit();
    return true;
  }

  function previewDecoded(token, width, height) {
    if (!state.preview || token !== state.previewToken) return;
    if (width !== state.preview.width || height !== state.preview.height) {
      previewFailed(token, "The decoded image dimensions do not match the capture result.");
      return;
    }
    state.previewMessage = `Preview decoded: ${width} × ${height}`;
    emit();
  }

  function previewFailed(token, message = "The captured PNG could not be decoded. Capture another frame.") {
    if (!state.preview || token !== state.previewToken) return;
    clearPreview("Preview could not be decoded.");
    state.error = message;
    emit();
  }

  async function describe() {
    if (!view().canDescribe) return false;
    const previewToken = state.previewToken;
    const requestLifecycle = lifecycle;
    const captureId = state.preview.capture_id;
    state.description = { status: "running", captureId };
    emit();
    let outcome;
    try {
      outcome = parseDescription(await invoke("describe_frame", { captureId }), captureId);
    } catch (error) {
      outcome = { status: "error", captureId, error: `Description request failed: ${errorMessage(error)}` };
    }
    // The frame was replaced or cleared while the model ran: show nothing for it.
    if (previewToken !== state.previewToken || requestLifecycle !== lifecycle) return false;
    state.description = outcome;
    emit();
    return outcome.status === "done";
  }

  return { view, perform, describe, selectMonitor, previewDecoded, previewFailed };
}

function mountDesktop(document, invoke, timers = globalThis) {
  const ids = ["connection-state", "connect", "disconnect", "error", "monitor", "monitor-details",
    "selection-message", "refresh-monitors", "show-overlay", "hide-overlay", "capture", "overlay-state",
    "preview", "preview-state", "preview-details", "describe", "description", "description-text",
    "description-state"];
  const elements = Object.fromEntries(ids.map((id) => [id, document.getElementById(id)]));
  let monitorOptions = "";
  let renderedPreviewKey = null;
  let controller;

  function renderDescription(description) {
    elements.description.hidden = !description;
    // Model output is untrusted: textContent only, never markup.
    elements["description-text"].textContent = description?.status === "done" ? description.text : "";
    elements["description-text"].hidden = description?.status !== "done";
    if (!description) {
      elements["description-state"].textContent = "";
    } else if (description.status === "running") {
      elements["description-state"].textContent = "Asking the local model… the first request can take a minute while it loads.";
    } else if (description.status === "error") {
      elements["description-state"].textContent = `Description failed: ${description.error}`;
    } else {
      const seconds = description.elapsedMs === null ? "" : ` · ${(description.elapsedMs / 1000).toFixed(1)} s`;
      elements["description-state"].textContent =
        `Local model ${description.model ?? "(unnamed)"}${seconds} · Unverified model output; it can be wrong.`;
    }
  }

  function render(state) {
    const connection = elements["connection-state"];
    connection.textContent = state.disconnecting ? "Disconnecting…"
      : state.connecting ? "Connecting…"
        : state.transportUnknown ? "Status unavailable"
          : state.status.connected ? "Connected · local stdio" : "Not connected";
    connection.dataset.connected = String(state.status.connected && !state.transportUnknown);
    elements.connect.disabled = !state.canConnect;
    elements.connect.textContent = state.transportUnknown ? "Reconnect" : "Connect";
    elements.disconnect.disabled = !state.canDisconnect;
    elements["refresh-monitors"].disabled = !state.canRefresh;
    elements.monitor.disabled = !state.canSelect;
    elements["show-overlay"].disabled = !state.canShow;
    elements["hide-overlay"].disabled = !state.canHide;
    elements.capture.disabled = !state.canCapture;
    elements.describe.disabled = !state.canDescribe;
    elements.describe.textContent = state.describing ? "Describing…" : "Describe frame";
    renderDescription(state.description);
    elements.capture.textContent = state.previewMessage === "Capturing one frame…" ? "Capturing…" : "Capture one frame";
    elements.error.textContent = state.error || "";
    elements.error.hidden = !state.error;

    const nextOptions = JSON.stringify([state.status.monitors, state.selectedId, state.status.connected]);
    if (nextOptions !== monitorOptions) {
      const options = [];
      if (!state.selectedId) {
        const placeholder = document.createElement("option");
        placeholder.value = "";
        placeholder.textContent = !state.status.connected ? "Connect to list displays"
          : state.status.monitors.length ? "Choose a display" : "No displays available";
        options.push(placeholder);
      }
      for (const [index, monitor] of state.status.monitors.entries()) {
        const option = document.createElement("option");
        option.value = monitor.id;
        option.textContent = `Display ${index + 1}${monitor.primary ? " · Primary" : ""} · ${monitor.width} × ${monitor.height} · ${monitor.device}`;
        options.push(option);
      }
      elements.monitor.replaceChildren(...options);
      elements.monitor.value = state.selectedId || "";
      monitorOptions = nextOptions;
    }
    const monitor = state.status.monitors.find((item) => item.id === state.selectedId);
    elements["monitor-details"].textContent = monitor
      ? `${monitor.width} × ${monitor.height} px · ${monitor.scale_percent}% scale · origin (${monitor.x}, ${monitor.y})`
      : "No display selected.";
    elements["selection-message"].textContent = state.selectionMessage;
    const overlay = state.status.overlay;
    elements["overlay-state"].textContent = overlay
      ? `Overlay requested · up to ${Math.max(0, Math.ceil(overlay.remaining_secs))}s remaining. Hide it to change displays.`
      : "No overlay requested.";

    elements["preview-state"].textContent = state.previewMessage;
    elements["preview-details"].textContent = state.preview
      ? `Source ${state.preview.original_width} × ${state.preview.original_height} px · Captured ${new Date(state.preview.captured_at_ms).toLocaleTimeString()}`
      : "";
    const previewKey = `${state.previewToken}:${Boolean(state.preview)}`;
    if (renderedPreviewKey === previewKey) return;
    renderedPreviewKey = previewKey;
    if (state.preview) {
      const preview = state.preview;
      const image = document.createElement("img");
      image.alt = "One captured frame of the selected display; local preview only.";
      image.addEventListener("load", () => controller.previewDecoded(preview.token, image.naturalWidth, image.naturalHeight));
      image.addEventListener("error", () => controller.previewFailed(preview.token));
      image.src = preview.data_url;
      elements.preview.replaceChildren(image);
    } else {
      const placeholder = document.createElement("p");
      placeholder.className = "placeholder";
      placeholder.textContent = state.previewMessage === "Capturing one frame…"
        ? "Capturing the selected display…" : "Capture one frame to inspect this display.";
      elements.preview.replaceChildren(placeholder);
    }
  }

  controller = createController(invoke, render);
  for (const [id, action] of [
    ["connect", "connect"], ["disconnect", "disconnect"], ["refresh-monitors", "refresh_monitors"],
    ["show-overlay", "show_overlay"], ["hide-overlay", "hide_overlay"], ["capture", "capture"],
  ]) {
    elements[id].addEventListener("click", () => { void controller.perform(action); });
  }
  elements.describe.addEventListener("click", () => { void controller.describe(); });
  elements.monitor.addEventListener("change", () => {
    if (!controller.selectMonitor(elements.monitor.value)) elements.monitor.value = controller.view().selectedId || "";
  });
  render(controller.view());
  void controller.perform("connect");
  const pollTimer = timers.setInterval(() => { void controller.perform("status"); }, POLL_INTERVAL_MS);
  return { controller, dispose: () => timers.clearInterval(pollTimer) };
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = { createController, createNativeInvoker, mountDesktop, monitorFingerprint, isLocalPreview };
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  const core = window.__TAURI__?.core;
  const nativeInvoke = core?.invoke && core?.Channel ? createNativeInvoker(core)
    : () => Promise.reject(new Error("Tauri native bridge is unavailable."));
  const mounted = mountDesktop(document, nativeInvoke);
  window.addEventListener("pagehide", () => mounted.dispose(), { once: true });
}
