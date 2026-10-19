import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const { createController, createNativeInvoker, mountDesktop } = require("../src/app.js");

const displays = [
  { id: "left", device: "DISPLAY1", x: -1920, y: 0, width: 1920, height: 1080, scale_percent: 100, primary: false },
  { id: "primary", device: "DISPLAY2", x: 0, y: 0, width: 2560, height: 1440, scale_percent: 100, primary: true },
];

function snapshot(overrides = {}, preview = null) {
  return { status: { connected: true, error: null, monitors: displays, overlay: null, ...overrides }, preview };
}

function frame(monitorId = "primary", captureId = 1) {
  return {
    capture_id: captureId, monitor_id: monitorId, data_url: "data:image/png;base64,iVBORw0KGgo=",
    width: 1280, height: 720, original_width: 2560, original_height: 1440,
    captured_at_ms: 1_792_339_200_000,
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function nativeBridge(invoke = async () => {}, expectedDelay = 30_000) {
  const channels = [];
  const calls = [];
  const timeouts = new Map();
  let nextTimer = 0;
  const core = {
    Channel: class { constructor() { channels.push(this); } },
    invoke(command, args) { calls.push({ command, args }); return invoke(command, args); },
  };
  const timers = {
    setTimeout(callback, delay) {
      assert.equal(delay, expectedDelay);
      timeouts.set(++nextTimer, callback);
      return nextTimer;
    },
    clearTimeout(timer) { timeouts.delete(timer); },
  };
  return { invoke: createNativeInvoker(core, timers), channels, calls, timeouts };
}

test("native bridge enqueues synchronously and waits for Channel completion", async () => {
  const bridge = nativeBridge();
  let complete = false;
  const result = bridge.invoke("desktop_action", { action: { type: "capture", monitor_id: "primary" } });
  result.then(() => { complete = true; });
  assert.equal(bridge.calls.length, 1);
  assert.equal(bridge.calls[0].args.reply, bridge.channels[0]);
  assert.deepEqual(bridge.calls[0].args.action, { type: "capture", monitor_id: "primary" });
  await Promise.resolve();
  assert.equal(complete, false);
  const outcome = snapshot({}, frame());
  bridge.channels[0].onmessage(outcome);
  assert.equal(await result, outcome);
  assert.equal(bridge.timeouts.size, 0);
});

test("native bridge preserves invocation order without waiting for prior completion", async () => {
  const bridge = nativeBridge();
  const capturing = bridge.invoke("desktop_action", { action: { type: "capture" } });
  const disconnecting = bridge.invoke("desktop_action", { action: { type: "disconnect" } });
  assert.deepEqual(bridge.calls.map(({ args }) => args.action.type), ["capture", "disconnect"]);
  bridge.channels[0].onmessage(snapshot({}, frame()));
  bridge.channels[1].onmessage(snapshot({ connected: false, monitors: [] }));
  await Promise.all([capturing, disconnecting]);
  assert.equal(bridge.timeouts.size, 0);
});

test("native bridge rejects enqueue failure and ignores any late reply", async () => {
  const bridge = nativeBridge(async () => { throw new Error("native queue is closed"); });
  const result = bridge.invoke("desktop_action", { action: { type: "connect" } });
  await assert.rejects(result, /native queue is closed/);
  bridge.channels[0].onmessage(snapshot());
  await assert.rejects(result, /native queue is closed/);
  assert.equal(bridge.timeouts.size, 0);
});

test("native bridge bounds lost replies and ignores late completion after timeout", async () => {
  const bridge = nativeBridge();
  const result = bridge.invoke("desktop_action", { action: { type: "capture" } });
  const rejected = assert.rejects(result, /did not reply within 30 seconds/);
  bridge.timeouts.values().next().value();
  await rejected;
  bridge.channels[0].onmessage(snapshot({}, frame()));
  await assert.rejects(result, /did not reply within 30 seconds/);
  assert.equal(bridge.timeouts.size, 0);
});

test("native bridge clears the deadline for synchronous errors and one successful completion", async () => {
  const broken = nativeBridge(() => { throw new Error("invoke unavailable"); });
  await assert.rejects(broken.invoke("desktop_action", { action: { type: "connect" } }), /invoke unavailable/);
  assert.equal(broken.timeouts.size, 0);

  const enqueue = deferred();
  const bridge = nativeBridge(() => enqueue.promise);
  const result = bridge.invoke("desktop_action", { action: { type: "status" } });
  const outcome = snapshot();
  bridge.channels[0].onmessage(outcome);
  bridge.channels[0].onmessage(snapshot({ connected: false }));
  enqueue.reject(new Error("late enqueue failure"));
  assert.equal(await result, outcome);
  await Promise.resolve();
  assert.equal(bridge.timeouts.size, 0);
});

async function connected(handler = () => snapshot()) {
  const calls = [];
  const controller = createController(async (command, args) => {
    calls.push({ command, args });
    return handler(args.action, calls.length);
  });
  await controller.perform("connect");
  return { controller, calls };
}

test("connect uses the native action contract and chooses the primary display once", async () => {
  const { controller, calls } = await connected();
  assert.deepEqual(calls, [{ command: "desktop_action", args: { action: { type: "connect" } } }]);
  assert.equal(controller.view().selectedId, "primary");
  assert.equal(controller.view().canCapture, true);
  controller.selectMonitor("left");
  await controller.perform("refresh_monitors");
  assert.equal(controller.view().selectedId, "left");
});

test("a removed or replaced display needs explicit selection after refresh", async () => {
  let monitors = displays;
  const { controller } = await connected(() => snapshot({ monitors }));
  monitors = [{ ...displays[0], primary: true }];
  await controller.perform("refresh_monitors");
  assert.equal(controller.view().selectedId, null);
  assert.match(controller.view().selectionMessage, /changed or disappeared/);
  assert.equal(controller.view().canCapture, false);
  await controller.perform("refresh_monitors");
  assert.equal(controller.view().selectedId, null);
  assert.equal(controller.selectMonitor("left"), true);
});

test("a changed display geometry invalidates selection instead of widening capture silently", async () => {
  let monitors = displays;
  const { controller } = await connected(() => snapshot({ monitors }));
  monitors = [displays[0], { ...displays[1], width: 3840, scale_percent: 150 }];
  await controller.perform("status");
  assert.equal(controller.view().selectedId, null);
});

test("an overlay request blocks display changes until it is hidden", async () => {
  let overlay = null;
  const { controller } = await connected((action) => {
    if (action.type === "show_overlay") overlay = { monitor_id: action.monitor_id, remaining_secs: 60 };
    if (action.type === "hide_overlay") overlay = null;
    return snapshot({ overlay });
  });
  await controller.perform("show_overlay");
  assert.equal(controller.view().canSelect, false);
  assert.equal(controller.selectMonitor("left"), false);
  await controller.perform("hide_overlay");
  assert.equal(controller.selectMonitor("left"), true);
});

test("capture blocks concurrent work but preserves local hide and disconnect controls", async () => {
  const capture = deferred();
  const { controller } = await connected((action) => action.type === "capture" ? capture.promise : snapshot());
  const result = controller.perform("capture");
  assert.equal(controller.view().canCapture, false);
  assert.equal(controller.view().canShow, false);
  assert.equal(controller.view().canSelect, false);
  assert.equal(controller.view().canHide, true);
  assert.equal(controller.view().canDisconnect, true);
  assert.equal(controller.selectMonitor("left"), false);
  assert.equal(await controller.perform("status"), false);
  capture.resolve(snapshot({}, frame()));
  await result;
});

test("disconnect prevents a late capture from restoring status or preview", async () => {
  const capture = deferred();
  const { controller } = await connected((action) => {
    if (action.type === "capture") return capture.promise;
    return snapshot({ connected: action.type !== "disconnect", monitors: action.type === "disconnect" ? [] : displays });
  });
  const result = controller.perform("capture");
  await controller.perform("disconnect");
  capture.resolve(snapshot({}, frame()));
  await result;
  assert.equal(controller.view().status.connected, false);
  assert.equal(controller.view().preview, null);
  assert.equal(controller.view().selectedId, null);
});

test("hide invalidates an in-flight capture even if its result arrives first", async () => {
  const capture = deferred();
  const hide = deferred();
  const { controller } = await connected((action) => {
    if (action.type === "capture") return capture.promise;
    if (action.type === "hide_overlay") return hide.promise;
    return snapshot();
  });
  const capturing = controller.perform("capture");
  const hiding = controller.perform("hide_overlay");
  capture.resolve(snapshot({}, frame()));
  await capturing;
  assert.equal(controller.view().preview, null);
  hide.resolve(snapshot());
  await hiding;
});

test("disconnect can cancel connection startup without a late connected render", async () => {
  const connect = deferred();
  const controller = createController(async (_command, { action }) => action.type === "connect"
    ? connect.promise : snapshot({ connected: false, monitors: [] }));
  const connecting = controller.perform("connect");
  assert.equal(controller.view().canDisconnect, true);
  await controller.perform("disconnect");
  connect.resolve(snapshot());
  await connecting;
  assert.equal(controller.view().status.connected, false);
});

test("native rejection exposes an error and unknown status until a successful status check", async () => {
  const { controller } = await connected((action) => {
    if (action.type === "capture") throw new Error("sidecar unavailable");
    return snapshot();
  });
  assert.equal(await controller.perform("capture"), false);
  assert.match(controller.view().error, /sidecar unavailable/);
  assert.equal(controller.view().transportUnknown, true);
  assert.equal(controller.view().canCapture, false);
  assert.equal(controller.view().canDisconnect, true);
  await controller.perform("status");
  assert.equal(controller.view().transportUnknown, false);
});

test("native operation errors remain visible and do not fabricate a preview", async () => {
  const { controller } = await connected((action) => action.type === "capture"
    ? snapshot({ error: "Capture permission was denied." }, frame()) : snapshot());
  await controller.perform("capture");
  assert.equal(controller.view().error, "Capture permission was denied.");
  assert.equal(controller.view().preview, null);
  assert.equal(controller.view().previewMessage, "Frame was not captured. Try again.");
});

test("only an explicit capture response can introduce an image", async () => {
  const { controller } = await connected(() => snapshot({}, frame()));
  assert.equal(controller.view().preview, null);
  await controller.perform("status");
  assert.equal(controller.view().preview, null);
  await controller.perform("capture");
  assert.equal(controller.view().preview.monitor_id, "primary");
});

test("capture validates image origin and selected monitor", async () => {
  for (const preview of [
    { ...frame(), data_url: "https://example.test/screen.png" },
    { ...frame(), data_url: "data:image/svg+xml;base64,PHN2Zz4=" },
    { ...frame(), capture_id: undefined },
    { ...frame(), capture_id: 0 },
    frame("left"),
  ]) {
    const { controller } = await connected(() => snapshot({}, preview));
    await controller.perform("capture");
    assert.equal(controller.view().preview, null);
    assert.match(controller.view().error, /valid PNG/);
  }
});

test("image load reports decoded dimensions and stale decode cannot restore cleared preview", async () => {
  const { controller } = await connected(() => snapshot({}, frame()));
  await controller.perform("capture");
  const token = controller.view().preview.token;
  controller.previewDecoded(token, 1280, 720);
  assert.equal(controller.view().previewMessage, "Preview decoded: 1280 × 720");
  controller.selectMonitor("left");
  controller.previewDecoded(token, 1280, 720);
  assert.equal(controller.view().preview, null);
  assert.equal(controller.view().previewMessage, "No frame captured.");
});

test("decode failure and mismatched dimensions clear the image with visible errors", async () => {
  for (const dimensions of [null, [640, 360]]) {
    const { controller } = await connected(() => snapshot({}, frame()));
    await controller.perform("capture");
    const token = controller.view().preview.token;
    if (dimensions) controller.previewDecoded(token, ...dimensions);
    else controller.previewFailed(token);
    assert.equal(controller.view().preview, null);
    assert.equal(controller.view().previewMessage, "Preview could not be decoded.");
    assert.ok(controller.view().error);
  }
});

test("reconnect and connection loss discard prior captured pixels", async () => {
  let online = true;
  const { controller } = await connected(() => snapshot({ connected: online, monitors: online ? displays : [] }, frame()));
  await controller.perform("capture");
  assert.ok(controller.view().preview);
  online = false;
  await controller.perform("status");
  assert.equal(controller.view().preview, null);
  online = true;
  await controller.perform("connect");
  assert.equal(controller.view().preview, null);
});

class FakeElement {
  constructor(tagName = "div") {
    this.tagName = tagName;
    this.dataset = {};
    this.children = [];
    this.events = new Map();
  }
  replaceChildren(...children) { this.children = children; }
  addEventListener(name, handler) { this.events.set(name, handler); }
}

function fakeDocument() {
  const elements = new Map();
  return {
    getElementById(id) {
      if (!elements.has(id)) elements.set(id, new FakeElement());
      return elements.get(id);
    },
    createElement(tagName) { return new FakeElement(tagName); },
  };
}

test("bundled renderer mounts a returned frame and ignores late image events after selection", async () => {
  const document = fakeDocument();
  let poll;
  let disposed = false;
  const mounted = mountDesktop(document, async () => snapshot({}, frame()), {
    setInterval(callback, delay) { poll = callback; assert.equal(delay, 2000); return 42; },
    clearInterval(id) { disposed = id === 42; },
  });
  await Promise.resolve();
  assert.equal(document.getElementById("connection-state").textContent, "Connected · local stdio");
  assert.equal(document.getElementById("preview").children[0].tagName, "p");
  await mounted.controller.perform("capture");
  const image = document.getElementById("preview").children[0];
  assert.equal(image.tagName, "img");
  assert.equal(image.src, frame().data_url);
  image.naturalWidth = 1280;
  image.naturalHeight = 720;
  image.events.get("load")();
  assert.equal(document.getElementById("preview-state").textContent, "Preview decoded: 1280 × 720");
  mounted.controller.selectMonitor("left");
  image.events.get("error")();
  assert.equal(document.getElementById("preview").children[0].tagName, "p");
  assert.equal(document.getElementById("preview-state").textContent, "No frame captured.");
  assert.equal(document.getElementById("error").hidden, true);
  poll();
  mounted.dispose();
  assert.equal(disposed, true);
});

test("renderer uses text for untrusted monitor names and truthful overlay-request copy", async () => {
  const document = fakeDocument();
  const mounted = mountDesktop(document, async () => snapshot({
    monitors: [{ ...displays[1], device: "<img src=bad onerror=alert(1)>" }],
    overlay: { monitor_id: "primary", remaining_secs: 59.5 },
  }), { setInterval() { return 1; }, clearInterval() {} });
  await Promise.resolve();
  assert.match(document.getElementById("monitor").children[0].textContent, /<img src=bad/);
  assert.match(document.getElementById("overlay-state").textContent, /^Overlay requested/);
  assert.equal(document.getElementById("monitor").disabled, true);
  assert.equal(document.getElementById("hide-overlay").disabled, false);
  mounted.dispose();
});

test("describe uses its own native command, argument and longer reply deadline", async () => {
  const bridge = nativeBridge(async () => {}, 130_000);
  const result = bridge.invoke("describe_frame", { captureId: 4 });
  assert.deepEqual(bridge.calls.map(({ command, args }) => [command, args.captureId, args.reply === bridge.channels[0]]),
    [["describe_frame", 4, true]]);
  const rejected = assert.rejects(result, /did not reply within 130 seconds/);
  bridge.timeouts.values().next().value();
  await rejected;
  assert.equal(bridge.timeouts.size, 0);
});

const description = (overrides = {}) => ({
  capture_id: 1, monitor_id: "primary", model: "local:1", text: "A dialog with an OK button.",
  error: null, elapsed_ms: 1500, ...overrides,
});

async function captured(describeHandler) {
  const describeCalls = [];
  const controller = createController(async (command, args) => {
    if (command === "describe_frame") {
      describeCalls.push(args);
      return describeHandler(args);
    }
    return snapshot({}, args.action.type === "capture" ? frame() : null);
  });
  await controller.perform("connect");
  await controller.perform("capture");
  return { controller, describeCalls };
}

test("describe needs a captured frame and sends only its capture id", async () => {
  const { controller, describeCalls } = await captured(() => description());
  assert.equal(controller.view().canDescribe, true);
  assert.equal(await controller.describe(), true);
  assert.deepEqual(describeCalls, [{ captureId: 1 }]);
  assert.deepEqual(controller.view().description, {
    status: "done", captureId: 1, text: "A dialog with an OK button.", model: "local:1", elapsedMs: 1500,
  });

  const none = createController(async () => snapshot());
  await none.perform("connect");
  assert.equal(none.view().canDescribe, false);
  assert.equal(await none.describe(), false);
});

test("a running description blocks a second request and a new capture but not hide or disconnect", async () => {
  const pending = deferred();
  const { controller, describeCalls } = await captured(() => pending.promise);
  const running = controller.describe();
  assert.equal(controller.view().describing, true);
  assert.equal(controller.view().canDescribe, false);
  assert.equal(controller.view().canCapture, false);
  assert.equal(controller.view().canHide, true);
  assert.equal(controller.view().canDisconnect, true);
  assert.equal(await controller.describe(), false);
  assert.equal(describeCalls.length, 1);
  pending.resolve(description());
  assert.equal(await running, true);
  assert.equal(controller.view().canCapture, true);
});

test("a description for a replaced, hidden or disconnected frame is discarded", async () => {
  for (const clear of [
    (controller) => controller.selectMonitor("left"),
    (controller) => controller.perform("hide_overlay"),
    (controller) => controller.perform("disconnect"),
  ]) {
    const pending = deferred();
    const { controller } = await captured(() => pending.promise);
    const running = controller.describe();
    await clear(controller);
    assert.equal(controller.view().description, null);
    assert.equal(controller.view().canCapture, controller.view().status.connected && Boolean(controller.view().selectedId));
    pending.resolve(description());
    assert.equal(await running, false);
    assert.equal(controller.view().description, null);
    assert.equal(controller.view().preview, null);
  }
});

test("description failures stay local to the description and never become text", async () => {
  const cases = [
    [() => description({ text: null, error: "Model qwen3-vl:2b is not installed." }), "Model qwen3-vl:2b is not installed."],
    [() => { throw new Error("IPC closed"); }, "Description request failed: IPC closed"],
    [() => description({ capture_id: 2 }), "invalid description response"],
    [() => description({ text: "", error: null }), "invalid description response"],
    [() => description({ error: "x" }), "x"],
    [() => null, "invalid description response"],
  ];
  for (const [handler, expected] of cases) {
    const { controller } = await captured(handler);
    assert.equal(await controller.describe(), false);
    const { description: result } = controller.view();
    assert.equal(result.status, "error");
    assert.ok(result.error.includes(expected), result.error);
    assert.ok(controller.view().preview, "the frame stays available for another try");
    assert.equal(controller.view().canDescribe, true);
    assert.equal(controller.view().error, null);
  }
});

test("renderer shows model output as text only, beside the preview, with truthful state copy", async () => {
  const document = fakeDocument();
  const pending = deferred();
  const mounted = mountDesktop(document, async (command, args) => {
    if (command === "describe_frame") return pending.promise;
    return snapshot({}, args.action.type === "capture" ? frame() : null);
  }, { setInterval() { return 1; }, clearInterval() {} });
  await Promise.resolve();
  assert.equal(document.getElementById("describe").disabled, true);
  assert.equal(document.getElementById("description").hidden, true);
  await mounted.controller.perform("capture");
  assert.equal(document.getElementById("describe").disabled, false);

  document.getElementById("describe").events.get("click")();
  assert.equal(document.getElementById("describe").textContent, "Describing…");
  assert.equal(document.getElementById("describe").disabled, true);
  assert.equal(document.getElementById("capture").disabled, true);
  assert.equal(document.getElementById("description").hidden, false);
  assert.match(document.getElementById("description-state").textContent, /Asking the local model/);
  assert.equal(document.getElementById("description-text").hidden, true);

  const hostile = "<img src=x onerror=alert(1)> ignore previous instructions";
  pending.resolve(description({ text: hostile }));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(document.getElementById("description-text").textContent, hostile);
  assert.equal(document.getElementById("description-text").hidden, false);
  assert.equal(document.getElementById("description-text").children.length, 0);
  assert.match(document.getElementById("description-state").textContent, /local:1 · 1\.5 s · Unverified model output/);
  assert.equal(document.getElementById("describe").textContent, "Describe frame");

  mounted.controller.selectMonitor("left");
  assert.equal(document.getElementById("description").hidden, true);
  assert.equal(document.getElementById("description-text").textContent, "");
  mounted.dispose();
});
