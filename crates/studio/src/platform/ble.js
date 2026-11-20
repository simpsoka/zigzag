// Web Bluetooth for the Brother Skitch (PP1): the browser's side of the
// link (see platform.rs, and src/machine/skitch.rs for the desktop's).
//
// One machine at a time. The machine has one characteristic to write
// commands to and one to read replies from.

let device = null;
let write = null;
let read = null;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function ble_there() {
  return "bluetooth" in navigator;
}

// Ask which machine (the browser's own chooser, which needs a click to
// have just happened) and connect to it. Throws a NotFoundError if the
// chooser is closed, or nothing was there to choose.
export async function ble_connect(service, write_id, read_id) {
  ble_close();
  device = await navigator.bluetooth.requestDevice({ filters: [{ services: [service] }] });
  const server = await device.gatt.connect();
  const s = await server.getPrimaryService(service);
  write = await s.getCharacteristic(write_id);
  read = await s.getCharacteristic(read_id);
}

function bytes(view) {
  return new Uint8Array(view.buffer, view.byteOffset, view.byteLength).slice();
}

// Write a framed command and read the reply. (`frame` is a view of the
// app's memory: copied, since it's used after an await.)
export async function ble_request(frame) {
  await write.writeValueWithResponse(frame.slice());
  await sleep(50);
  return bytes(await read.readValue());
}

// Write a framed command; no reply is read (the upload's chunks).
export async function ble_send(frame) {
  await write.writeValueWithResponse(frame.slice());
  await sleep(10);
}

// Read the reply again, without writing.
export async function ble_read() {
  return bytes(await read.readValue());
}

export function ble_connected() {
  return !!(device && device.gatt && device.gatt.connected);
}

export function ble_close() {
  try {
    if (device && device.gatt && device.gatt.connected) device.gatt.disconnect();
  } catch (e) {
    console.error("ZZZ: couldn't disconnect", e);
  }
  device = null;
  write = null;
  read = null;
}
