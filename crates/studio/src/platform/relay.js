// The tab's end of the MCP relay (see platform.rs, and relay/ for the
// program at the other end).
//
// Nothing here runs unless the page was opened with the relay's link
// (`#relay=CODE`, or `#relay=CODE@PORT`): the code pairs this tab with
// that relay, and is kept for the tab's lifetime so a reload stays paired.

let ws = null;
let inbox = [];
let state = "off"; // "off" (no relay asked for), "looking", "on"
let pairing = null; // { code, port }
let wake = () => {};

function pairing_from_link() {
  const m = /[#&]relay=([0-9a-f]+)(?:@(\d+))?/i.exec(location.hash);
  if (m) {
    sessionStorage.setItem("zzz-relay", JSON.stringify({ code: m[1], port: m[2] || "7879" }));
    // The code needn't stay in the address bar (or in a copied link).
    history.replaceState(null, "", location.pathname + location.search);
  }
  try {
    return JSON.parse(sessionStorage.getItem("zzz-relay"));
  } catch {
    return null;
  }
}

// Another tab took this one's place with the relay.
let replaced = false;
// The relay tells a client a call failed after 180 s; a call older than
// this isn't run (the client has been told it wasn't).
const STALE_MS = 170000;

function connect() {
  replaced = false;
  state = "looking";
  const socket = new WebSocket(`ws://127.0.0.1:${pairing.port}`);
  ws = socket;
  socket.onopen = () => {
    socket.send(JSON.stringify({ hello: pairing.code }));
    state = "on";
    wake();
  };
  socket.onmessage = (e) => {
    // Another tab paired with the relay in this one's place: this one lets
    // it be (were it to come back, the two would take turns for ever).
    if (e.data === '{"replaced":true}') {
      replaced = true;
      return;
    }
    inbox.push({ at: Date.now(), text: e.data });
    wake();
  };
  socket.onclose = () => {
    if (ws !== socket) return;
    ws = null;
    if (replaced) {
      state = "off";
      sessionStorage.removeItem("zzz-relay");
      wake();
      return;
    }
    state = "looking";
    wake();
    // The relay may be started later, or again: keep looking.
    setTimeout(connect, 3000);
  };
}

// Start looking for the relay, if this tab was opened with its link.
// `on_news` is called when there's something to read, or the state changed.
export function relay_start(on_news) {
  wake = on_news;
  pairing = pairing_from_link();
  if (pairing) connect();
  // The relay's link opened in a tab that's already ZZZ changes only the
  // part after `#`, which doesn't load the page again: pair from here.
  window.addEventListener("hashchange", () => {
    const now = pairing_from_link();
    if (!now || (pairing && now.code === pairing.code && now.port === pairing.port)) return;
    pairing = now;
    const old = ws;
    ws = null;
    if (old) old.close();
    connect();
  });
}

// The messages that came in since the last call (less any too old to run).
export function relay_take() {
  const now = Date.now();
  const taken = inbox.filter((m) => now - m.at < STALE_MS).map((m) => m.text);
  inbox = [];
  return taken;
}

// Whether messages are waiting to be taken.
export function relay_pending() {
  return inbox.length > 0;
}

export function relay_send(text) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(text);
}

export function relay_state() {
  return state;
}
