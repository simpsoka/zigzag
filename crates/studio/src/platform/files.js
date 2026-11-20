// The browser's side of ZZZ's files (see platform.rs).
//
// Two things are kept in IndexedDB between visits:
//   "files":   what the studio has in memory (zigzag::vfs), by path
//   "handles": the files on the computer that were opened or saved to
//              (File System Access handles), by name, so Save goes back
//              to the same file

const DB = "zzz";
const FILES = "files";
const HANDLES = "handles";

let db = null;
const handles = new Map();

function open() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => {
      r.result.createObjectStore(FILES);
      r.result.createObjectStore(HANDLES);
    };
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

function all(store) {
  return new Promise((resolve, reject) => {
    const out = [];
    const c = db.transaction(store).objectStore(store).openCursor();
    c.onsuccess = () => {
      const at = c.result;
      if (at) {
        out.push([at.key, at.value]);
        at.continue();
      } else {
        resolve(out);
      }
    };
    c.onerror = () => reject(c.error);
  });
}

function put(store, key, value) {
  if (!db) return;
  try {
    const t = db.transaction(store, "readwrite");
    t.objectStore(store).put(value, key);
    t.onabort = () => console.error("ZZZ: couldn't keep", key, t.error);
  } catch (e) {
    console.error("ZZZ: couldn't keep", key, e);
  }
}

// Everything kept from last time: [[path, bytes], …]. Also brings back
// the file handles. An empty list if the browser keeps nothing (private
// windows, storage turned off).
export async function kept_load() {
  try {
    db = await open();
    for (const [name, handle] of await all(HANDLES)) handles.set(name, handle);
    const kept = (await all(FILES)).map(([path, bytes]) => [path, new Uint8Array(bytes)]);
    for (const [path] of kept) if (!path.startsWith("/")) names.add(path);
    for (const name of handles.keys()) names.add(name);
    return kept;
  } catch (e) {
    console.error("ZZZ: nothing is kept between visits here", e);
    db = null;
    return [];
  }
}

// (`bytes` is a view of the app's memory: copied, since it's used later.)
export function kept_put(path, bytes) {
  put(FILES, path, bytes.slice());
}

export function kept_remove(path) {
  if (!db) return;
  try {
    db.transaction(FILES, "readwrite").objectStore(FILES).delete(path);
  } catch (e) {
    console.error("ZZZ: couldn't forget", path, e);
  }
}

// Ask the browser not to clear what's kept when space runs short.
export function keep_for_good() {
  if (navigator.storage && navigator.storage.persist) navigator.storage.persist();
}

export function can_pick() {
  return "showOpenFilePicker" in window && "showSaveFilePicker" in window;
}

function remember(name, handle) {
  handles.set(name, handle);
  put(HANDLES, name, handle);
}

// The names ZZZ knows the person's files by: a file is known by its name
// alone, as the person knows it, so two different files can't share one.
const names = new Set();

// The name to know a file by. The same file (the same entry on the
// computer, for one opened through a handle) keeps the name it had; a
// different file whose name is taken gets "logo (2).svg", and so on.
export async function claim_name(name, handle) {
  const had = handles.get(name);
  if (handle && had) {
    try {
      if (await had.isSameEntry(handle)) return name;
    } catch {
      // (A handle from an earlier visit may be gone: then it's another file.)
    }
  }
  if (!names.has(name)) {
    names.add(name);
    return name;
  }
  const dot = name.lastIndexOf(".");
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  for (let k = 2; ; k++) {
    const other = `${stem} (${k})${ext}`;
    if (!names.has(other)) {
      names.add(other);
      return other;
    }
  }
}

function types(description, exts) {
  if (!exts.length) return undefined;
  return [{ description, accept: { "application/octet-stream": exts.map((e) => "." + e) } }];
}

// The browser's Open panel: [[name, bytes], …] for what was chosen (none
// if it was closed).
export async function pick_open(description, exts, many) {
  try {
    const picked = await window.showOpenFilePicker({
      multiple: many,
      types: types(description, exts),
    });
    const out = [];
    for (const handle of picked) {
      const file = await handle.getFile();
      const name = await claim_name(file.name, handle);
      remember(name, handle);
      out.push([name, new Uint8Array(await file.arrayBuffer())]);
    }
    return out;
  } catch (e) {
    if (e.name !== "AbortError") console.error("ZZZ: couldn't open", e);
    return [];
  }
}

// The browser's Save panel: the name chosen (null if it was closed). What's
// written under that name later goes to the file (`deliver`).
export async function pick_save(name, description, exts) {
  try {
    const handle = await window.showSaveFilePicker({
      suggestedName: name,
      types: types(description, exts),
    });
    const named = await claim_name(handle.name, handle);
    remember(named, handle);
    return named;
  } catch (e) {
    if (e.name !== "AbortError") console.error("ZZZ: couldn't save", e);
    return null;
  }
}

function download(name, bytes) {
  const url = URL.createObjectURL(new Blob([bytes]));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}

// A file the studio wrote: put it where the person will find it. To the
// file on the computer it was opened from or saved to, when there is one;
// else the browser asks where (once: the answer is remembered); else, in
// a browser that can't, a download.
//
// One at a time, in the order they were written: two quick saves of the
// same file can't finish the wrong way round. (`view` is copied now: it's
// the app's memory, which moves on.)
let writing = Promise.resolve();
export function deliver(name, view) {
  const bytes = view.slice();
  writing = writing.then(() => deliver_now(name, bytes));
}

async function deliver_now(name, bytes) {
  try {
    let handle = handles.get(name);
    if (!handle && can_pick()) {
      handle = await window.showSaveFilePicker({ suggestedName: name });
      remember(name, handle);
    }
    if (handle) {
      const w = await handle.createWritable();
      await w.write(bytes);
      await w.close();
      return;
    }
  } catch (e) {
    // Closed without choosing: it stays in the browser, unsaved elsewhere.
    if (e.name === "AbortError") return;
    console.error("ZZZ: couldn't write", name, e);
  }
  download(name, bytes);
}

// The fonts that come with ZZZ to embroider in (assets/fonts/library,
// copied beside the page): [[file, bytes], …], fetched after the studio
// is up so it needn't wait for them. The browser keeps them in its cache
// for the next visit. A font that can't be fetched is left out.
export async function font_library() {
  try {
    const index = await (await fetch("library/library.json")).json();
    const got = await Promise.all(
      index.fonts.map(async ({ file }) => {
        try {
          const r = await fetch(`library/${file}`);
          if (!r.ok) throw new Error(`${r.status}`);
          return [file, new Uint8Array(await r.arrayBuffer())];
        } catch (e) {
          console.error("ZZZ: couldn't fetch the font", file, e);
          return null;
        }
      }),
    );
    return got.filter(Boolean);
  } catch (e) {
    console.error("ZZZ: couldn't fetch the font library", e);
    return [];
  }
}
