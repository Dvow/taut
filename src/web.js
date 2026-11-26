// taut's loader: runs a taut program built for wasm32-unknown-unknown in a page, with no
// other script. It makes the page's elements (a canvas, a hidden text field, the elements
// screen readers read), hands each DOM event to the program, and does what the program asks:
// show pixels, measure and draw text, keep the clipboard and storage; and the parts a program
// uses only when it asks (drops and drags, the clipboard's pictures, pickers, notifications, one
// running copy, printing, the person's fonts, WebGL2).
//
//   import { start } from "./taut.js";
//   start("app.wasm", { element: document.body, fonts: { regular: "Inter.ttf" } });
//
// Options: `element` (the page by default; `null` for no page at all, as in tests under Node),
// `fonts` (files by face: regular, bold, mono, italic; the browser's own otherwise), `sans` and
// `mono` (CSS families for the browser's text), `title` (false: leave the page's title alone),
// `renderer` and `draw` (what `TAUT_RENDERER` and `TAUT_DRAW` say on the desktops: "cpu",
// "gpu", "auto").

const CURSORS = ["default", "pointer", "text", "ew-resize", "ns-resize", "nwse-resize", "nesw-resize", "move", "not-allowed"];
const SANS = 'system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif';
const MONO = 'ui-monospace, "Cascadia Mono", Consolas, "SF Mono", Menlo, "DejaVu Sans Mono", monospace';
const FACES = ["regular", "bold", "mono", "italic"];
const KINDS = { mouse: 0, touch: 1, pen: 2 };
const POINTER = { pointermove: 0, pointerdown: 1, pointerup: 2, pointercancel: 3, pointerleave: 4 };
// What the program's parts hear (`taut_heard`), each as the part's number (`web_host::parts`)
// and the thing's number in that part's module.
const HEARD = {
    dropOver: [0, 0],
    dropLeft: [0, 1],
    droppedFile: [0, 2],
    droppedFiles: [0, 3],
    droppedText: [0, 4],
    dragStart: [1, 0],
    dragOver: [1, 1],
    dragDrop: [1, 2],
    dragEnd: [1, 3],
    dragLeft: [1, 4],
    dragOffered: [1, 5],
    clipboardPicture: [2, 0],
    pastedFile: [2, 1],
    pastedFiles: [2, 2],
    notificationClicked: [3, 0],
    laterTab: [3, 1],
    printClosed: [4, 0],
    printUnable: [4, 1],
    fontsListed: [5, 0],
    fontsRefused: [5, 1],
    contextLost: [6, 0],
    pickedMore: [7, 0],
    pickedLast: [7, 1],
    pickedFolder: [7, 2],
    saveChosen: [7, 3],
};
const encoder = new TextEncoder();
const decoder = new TextDecoder();

// Loads and starts `program` (an address, or its bytes). Resolves to the page.
export async function start(program, options = {}) {
    const page = new Page(options);
    const [module, fonts, locks] = await Promise.all([compiled(program), fontFiles(options.fonts), heldLocks()]);
    Object.assign(page, { fonts, locks });
    const instance = await WebAssembly.instantiate(module, { taut: page.imports(), taut_gl: page.glImports() });
    page.begin(instance.exports);
    return page;
}

async function compiled(program) {
    if (typeof program !== "string") {
        return WebAssembly.compile(program);
    }
    const response = await fetch(program);
    if (!response.ok) {
        throw new Error(`taut: ${program}: ${response.status}`);
    }
    return WebAssembly.compile(await response.arrayBuffer());
}

async function fontFiles(given) {
    const fonts = [];
    for (const [face, file] of Object.entries(given || {})) {
        const at = FACES.indexOf(face);
        const bytes = typeof file === "string" ? await (await fetch(file)).arrayBuffer() : file;
        if (at >= 0) {
            fonts.push([at, new Uint8Array(bytes.buffer || bytes, bytes.byteOffset || 0, bytes.byteLength)]);
        }
    }
    return fonts;
}

// The names of the locks the site's pages hold now (Web Locks), where the browser has them: what
// tells a program's first tab from a later one.
async function heldLocks() {
    try {
        return (await navigator.locks.query()).held.map((lock) => lock.name);
    } catch {
        return [];
    }
}

// The modifiers held, as the program counts them: Control or Command, Shift, Alt.
function modifiers(event) {
    return (event.ctrlKey || event.metaKey ? 1 : 0) | (event.shiftKey ? 2 : 0) | (event.altKey ? 4 : 0);
}

export class Page {
    constructor(options) {
        this.options = options;
        this.exports = null;
        this.busy = false;
        this.later = [];
        this.made = null;
        this.copied = null;
        this.pasting = 0;
        this.composing = false;
        this.timer = 0;
        this.frameAsked = false;
        this.nodes = new Map();
        this.told = new Map();
        this.focusKey = "";
        this.ownFocus = false;
        this.fieldOn = 0;
        this.size = [0, 0, 1];
        this.sized = false;
        this.pixels = null;
        this.measure = textCanvas();
        this.videos = new Map();
        this.videoCount = 0;
        this.dropping = 0;
        this.pointedAt = [0, 0];
        this.dragFrom = null;
        this.dragItems = [];
        this.draggingOut = false;
        this.droppedHere = false;
        this.clipboardParts = false;
        this.saved = null;
        this.printing = [];
        this.gl = null;
        this.glCanvas = null;
        this.glObjects = [null];
        if (options.element !== null && typeof document !== "undefined") {
            this.build(options.element || document.body);
        }
    }

    // The page's elements: a box holding the canvas, the text field, what an input method
    // composes, the readers' elements and a live region.
    build(parent) {
        const make = (tag, style, into) => {
            const made = document.createElement(tag);
            made.style.cssText = style;
            (into || this.box).appendChild(made);
            return made;
        };
        this.box = make("div", "position:relative;width:100%;height:100%;overflow:hidden;outline:none;", parent);
        this.box.tabIndex = 0;
        this.canvas = make("canvas", "position:absolute;left:0;top:0;touch-action:none;user-select:none;-webkit-user-select:none;");
        this.canvas.setAttribute("aria-hidden", "true");
        this.context = this.canvas.getContext("2d", { alpha: false });
        this.readers = make("div", "position:absolute;left:0;top:0;width:100%;height:100%;opacity:0;pointer-events:none;overflow:hidden;");
        this.field = make("input", "position:absolute;left:0;top:0;width:1px;height:1px;opacity:0;border:0;padding:0;margin:0;outline:none;background:transparent;color:transparent;caret-color:transparent;");
        this.field.setAttribute("autocomplete", "off");
        this.field.setAttribute("autocapitalize", "off");
        this.field.setAttribute("spellcheck", "false");
        this.field.tabIndex = -1;
        this.compose = make("div", "position:absolute;display:none;white-space:pre;pointer-events:none;background:Canvas;color:CanvasText;border-bottom:2px solid;");
        this.live = make("div", "position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);");
        this.live.setAttribute("aria-live", "polite");
        this.listen();
    }

    listen() {
        const box = this.box;
        for (const name of Object.keys(POINTER)) {
            this.canvas.addEventListener(name, (event) => this.pointer(event));
        }
        this.canvas.addEventListener("wheel", (event) => this.wheel(event), { passive: false });
        this.canvas.addEventListener("contextmenu", (event) => event.preventDefault());
        this.canvas.addEventListener("contextrestored", () => this.restored());
        box.addEventListener("keydown", (event) => this.key(event, 1));
        box.addEventListener("keyup", (event) => this.key(event, 0));
        box.addEventListener("focusin", (event) => this.focused(event, 1));
        box.addEventListener("focusout", (event) => this.focused(event, 0));
        box.addEventListener("copy", (event) => this.copy(event));
        box.addEventListener("cut", (event) => this.copy(event));
        box.addEventListener("paste", (event) => this.paste(event));
        box.addEventListener("dragenter", (event) => this.dragOver(event));
        box.addEventListener("dragover", (event) => this.dragOver(event));
        box.addEventListener("dragleave", (event) => this.dragLeft(event));
        box.addEventListener("drop", (event) => this.dropped(event));
        this.canvas.addEventListener("dragstart", (event) => this.dragStart(event));
        this.canvas.addEventListener("dragend", (event) => this.dragEnd(event));
        this.field.addEventListener("input", (event) => this.input(event));
        this.field.addEventListener("compositionstart", () => this.composed(""));
        this.field.addEventListener("compositionupdate", (event) => this.composed(event.data || ""));
        this.field.addEventListener("compositionend", (event) => this.composeEnd(event));
        this.readers.addEventListener("click", (event) => this.readerAct(event, 0));
        this.readers.addEventListener("focusin", (event) => this.readerAct(event, 1));
        new ResizeObserver(() => this.resized()).observe(box);
        for (const query of ["(prefers-color-scheme: dark)", "(prefers-contrast: more)", "(forced-colors: active)", "(prefers-reduced-motion: reduce)"]) {
            matchMedia(query).addEventListener("change", () => this.looks());
        }
        addEventListener("pagehide", () => this.call("taut_close"));
    }

    // The program is ready: it starts, and hears the page's size and looks.
    begin(exports) {
        this.exports = exports;
        this.busy = true;
        try {
            exports.main(0, 0);
        } finally {
            this.busy = false;
        }
        if (this.box) {
            this.resized();
            this.looks();
        }
        this.flush();
    }

    // Calls a function of the program's, or keeps the call for later while one runs (an
    // element that takes focus from inside a call says so at once).
    call(name, ...args) {
        if (!this.exports) {
            return 0;
        }
        if (this.busy) {
            this.later.push([name, args]);
            return 0;
        }
        this.busy = true;
        let answer = 0;
        try {
            answer = this.exports[name](...args.map((arg) => (typeof arg === "string" || arg instanceof Uint8Array ? this.pass(arg) : [arg])).flat());
        } finally {
            this.busy = false;
        }
        this.flush();
        return answer;
    }

    flush() {
        while (!this.busy && this.later.length > 0) {
            const [name, args] = this.later.shift();
            this.call(name, ...args);
        }
    }

    // Text or bytes copied into the program's memory: their place and length.
    pass(value) {
        const bytes = typeof value === "string" ? encoder.encode(value) : value;
        const at = this.exports.taut_alloc(bytes.length);
        new Uint8Array(this.exports.memory.buffer, at, bytes.length).set(bytes);
        return [at, bytes.length];
    }

    bytes(at, length) {
        return new Uint8Array(this.exports.memory.buffer, at, length);
    }

    text(at, length) {
        return decoder.decode(this.bytes(at, length));
    }

    // What the program imports from the page.
    imports() {
        const page = this;
        const floats = (at, values) => new Float32Array(page.exports.memory.buffer, at, values.length).set(values);
        return {
            present: (at, x, y, width, height) => page.present(at, x, y, width, height),
            wake: (after) => page.wake(after),
            set_cursor: (shape) => page.canvas && (page.canvas.style.cursor = CURSORS[shape] || "default"),
            set_title: (at, length) => page.options.title !== false && typeof document !== "undefined" && (document.title = page.text(at, length)),
            text_field: (x, y, width, height, on) => page.textField(x, y, width, height, on),
            clipboard_set: (at, length) => page.copyOut(page.text(at, length)),
            mirror: (at, length) => page.mirror(JSON.parse(page.text(at, length))),
            log: (at, length) => console.error(page.text(at, length)),
            now: () => performance.now(),
            clock: () => Date.now(),
            zone: () => -new Date().getTimezoneOffset(),
            storage_get: (at, length) => page.storageGet(page.text(at, length)),
            storage_set: (at, length, value, valueLength) => page.storageSet(page.text(at, length), page.text(value, valueLength)),
            open_link: (at, length) => typeof open === "function" && open(page.text(at, length), "_blank", "noopener,noreferrer"),
            ask_file: (at, length, name, nameLength, kind) => page.askFile(page.text(at, length), page.text(name, nameLength), kind),
            download: (name, length, at, size) => page.download(page.text(name, length), page.bytes(at, size).slice()),
            save_file: (at, length) => page.saveFile(page.bytes(at, length).slice()),
            font_file: (nth, face) => page.fontFile(nth, face),
            take: (into) => page.take(into),
            text_metrics: (at, length, out) => floats(out, page.metrics(page.text(at, length))),
            text_measure: (at, length, text, textLength, stops, count) => page.measureText(page.text(at, length), page.text(text, textLength), stops, count),
            text_draw: (at, length, text, textLength, place) => page.drawText(page.text(at, length), page.text(text, textLength), place),
            video_open: (at, length, kind, kindLength, bytes, sound) => page.videoOpen(bytes ? page.bytes(at, length).slice() : page.text(at, length), page.text(kind, kindLength), sound),
            video_close: (id) => page.videoClose(id),
            video_play: (id) => page.videoPlay(id),
            video_pause: (id) => page.videoPause(id),
            video_seek: (id, seconds) => page.videoSeek(id, seconds),
            video_sound: (id, volume, muted) => page.videoSound(id, volume, muted),
            video_state: (id, out) => page.videoState(id, out),
            video_frame: (id, out) => page.videoFrame(id, out),
            video_speed: (id, speed) => page.videoSpeed(id, speed),
            video_tracks: (id) => page.videoTracks(id),
            video_choose_sound: (id, nth) => page.videoChooseSound(id, nth),
            video_choose_captions: (id, nth) => page.videoChooseCaptions(id, nth),
            video_caption: (id) => page.videoCaption(id),
            drag_from: (left, top, width, height) => (page.dragFrom = [left, top, width, height]),
            drag_item: (kind, name, nameLength, at, length) => page.dragItems.push([kind, page.text(name, nameLength), page.bytes(at, length).slice()]),
            clipboard_watch: () => (page.clipboardParts = true),
            clipboard_picture_set: (at, length) => page.copyPicture(page.bytes(at, length).slice()),
            notify: (number, title, titleLength, text, textLength, png, pngLength) => page.notify(number, page.text(title, titleLength), page.text(text, textLength), page.bytes(png, pngLength).slice()),
            first_copy: (at, length) => page.firstCopy(page.text(at, length)),
            region: () => page.region(),
            print_page: (at, length, width, height, pageWidth, pageHeight) => page.printing.push({ rgba: page.bytes(at, length).slice(), width, height, size: [pageWidth, pageHeight] }),
            print_pages: (at, length, job) => page.printPages(page.text(at, length), job),
            ask_fonts: (again) => page.askFonts(again !== 0),
            gpu_open: () => page.gpuOpen(),
            gpu_close: () => page.gpuClose(),
            setting: (at, length) => page.setting(page.text(at, length)),
        };
    }

    // Tells a part of the program what the page heard (`HEARD`): its answer. `name` is a file's.
    heard([part, what], x = 0, y = 0, number = 0, bytes = new Uint8Array(), name = "") {
        return this.call("taut_heard", part, what, x, y, number, bytes, name);
    }

    // Text kept for the program to `take`: its length in bytes.
    kept(text) {
        this.made = encoder.encode(text);
        return this.made.length;
    }

    // A place of a DOM event, in CSS pixels from the canvas's corner.
    placeOf(event) {
        const place = this.canvas.getBoundingClientRect();
        return [event.clientX - place.left, event.clientY - place.top];
    }

    // ---- pixels and frames ----

    present(at, x, y, width, height) {
        const pixels = new Uint8ClampedArray(this.exports.memory.buffer, at, width * height * 4);
        if (this.context) {
            this.context.putImageData(new ImageData(pixels, width, height), x, y);
            return;
        }
        const [wide] = this.pixelSize();
        for (let row = 0; row < height; row++) {
            this.pixels.set(pixels.subarray(row * width * 4, (row + 1) * width * 4), ((y + row) * wide + x) * 4);
        }
    }

    pixelSize() {
        const [width, height, scale] = this.size;
        return [Math.round(width * scale), Math.round(height * scale)];
    }

    // A frame is wanted: now (0), after a while (in ms), or never (-1).
    wake(after) {
        clearTimeout(this.timer);
        if (after < 0 || typeof requestAnimationFrame !== "function") {
            this.frameAsked = after >= 0;
            return;
        }
        if (after > 0) {
            this.timer = setTimeout(() => this.wake(0), after);
            return;
        }
        if (!this.frameAsked) {
            this.frameAsked = true;
            requestAnimationFrame(() => this.frame());
        }
    }

    frame() {
        this.frameAsked = false;
        this.call("taut_frame");
    }

    // The page's element changed size, or the screen's pixel ratio did: the canvas takes whole
    // device pixels, and the program the size they make. (A canvas given its size again is
    // cleared: only a new size is given. The first may come before the program is there: it
    // hears it once it is.)
    resized() {
        const scale = devicePixelRatio || 1;
        const width = Math.round(this.box.clientWidth * scale);
        const height = Math.round(this.box.clientHeight * scale);
        const same = width === this.canvas.width && height === this.canvas.height && scale === this.size[2];
        if (same && this.sized) {
            return;
        }
        for (const canvas of [this.canvas, this.glCanvas].filter(Boolean)) {
            if (canvas.width !== width || canvas.height !== height) {
                canvas.width = width;
                canvas.height = height;
            }
            canvas.style.width = `${width / scale}px`;
            canvas.style.height = `${height / scale}px`;
        }
        this.resize(width / scale, height / scale, scale);
    }

    // The browser dropped the canvas's pixels (its context was lost and is back): all of it is
    // drawn again, as for a new size.
    restored() {
        this.call("taut_size", 0, 0, 1);
        this.resize(...this.size);
    }

    // The interface's size in CSS pixels, at `scale` device pixels to one (with no page, this
    // is how a test sizes it).
    resize(width, height, scale) {
        this.size = [width, height, scale];
        this.sized = this.exports !== null;
        if (!this.context) {
            const [wide, high] = this.pixelSize();
            this.pixels = new Uint8ClampedArray(wide * high * 4);
        }
        this.call("taut_size", width, height, scale);
    }

    // Dark, contrast, motion and accent colour, as the person set them.
    looks() {
        const asks = (query) => matchMedia(query).matches;
        const contrast = asks("(forced-colors: active)") ? 2 : asks("(prefers-contrast: more)") ? 1 : 0;
        this.call("taut_looks", asks("(prefers-color-scheme: dark)") ? 1 : 0, contrast, asks("(prefers-reduced-motion: reduce)") ? 1 : 0, accent());
    }

    // ---- input ----

    pointer(event) {
        const kind = KINDS[event.pointerType] ?? 0;
        // (The browser drags what the program hands out: its own events come instead.)
        if (this.draggingOut && event.type === "pointercancel") {
            return;
        }
        // (What a press may drag is what it pressed, wherever it moves since.)
        if (event.type === "pointerdown" || (event.type === "pointermove" && event.buttons === 0)) {
            this.pointedAt = this.placeOf(event);
            this.canvas.draggable = this.heard(HEARD.dragOffered, ...this.pointedAt) !== 0;
        }
        if (event.type === "pointerdown") {
            // (Over a box handing something out, the browser may start a drag.)
            if (!this.canvas.draggable) {
                event.preventDefault();
            }
            // (A pointer the browser no longer has can't be held: a made-up one, say.)
            try {
                this.canvas.setPointerCapture(event.pointerId);
            } catch {}
            if (!this.box.contains(document.activeElement)) {
                this.box.focus({ preventScroll: true });
            }
        }
        const place = this.canvas.getBoundingClientRect();
        const args = [POINTER[event.type], kind, event.pointerId >>> 0, event.isPrimary ? 1 : 0, event.clientX - place.left, event.clientY - place.top, event.button, event.buttons];
        this.call("taut_pointer", ...args, event.pressure || 0, event.tiltX || 0, event.tiltY || 0, event.twist || 0, modifiers(event));
    }

    wheel(event) {
        event.preventDefault();
        const place = this.canvas.getBoundingClientRect();
        this.call("taut_wheel", event.clientX - place.left, event.clientY - place.top, event.deltaX, event.deltaY, event.deltaMode, modifiers(event));
    }

    key(event, down) {
        if (event.isComposing || event.keyCode === 229 || this.composing) {
            return;
        }
        const pastes = down && (event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "v";
        if (pastes) {
            // The browser's paste event brings the system clipboard's text: it presses Ctrl+V.
            // Without one (nothing to paste, or it was refused), the key goes on alone.
            this.pasting = setTimeout(() => this.pasteless(event), 100);
            return;
        }
        if (this.call("taut_key", down, event.key, event.code, modifiers(event), document.activeElement === this.field ? 1 : 0)) {
            event.preventDefault();
        }
    }

    pasteless(event) {
        this.pasting = 0;
        this.call("taut_key", 1, event.key, event.code, modifiers(event), 1);
    }

    paste(event) {
        if (!this.pasting && document.activeElement !== this.field) {
            return;
        }
        clearTimeout(this.pasting);
        this.pasting = 0;
        event.preventDefault();
        const text = event.clipboardData?.getData("text/plain") || "";
        if (!this.clipboardParts) {
            this.call("taut_paste", text);
            return;
        }
        this.pastedFiles([...(event.clipboardData?.files || [])]).then(() => this.call("taut_paste", text));
    }

    // Files pasted (the picture among them, as the clipboard's), read whole before Control+V is
    // pressed.
    async pastedFiles(files) {
        for (const file of files) {
            const bytes = new Uint8Array(await file.arrayBuffer());
            if (file.type === "image/png") {
                this.heard(HEARD.clipboardPicture, 0, 0, 0, bytes);
            }
            this.heard(HEARD.pastedFile, 0, 0, 0, bytes, file.name);
        }
        this.heard(HEARD.pastedFiles);
    }

    // A picture copied: on the system clipboard as PNG (after a press or a key, as the browser
    // allows).
    copyPicture(png) {
        this.copied = null;
        if (typeof ClipboardItem === "function") {
            navigator.clipboard?.write([new ClipboardItem({ "image/png": new Blob([png], { type: "image/png" }) })]).catch(() => {});
        }
    }

    // The program copied: the browser's copy event (which follows Ctrl+C) puts it on the
    // system clipboard; from a menu, the asynchronous clipboard does.
    copyOut(text) {
        this.copied = text;
        navigator.clipboard?.writeText(text).catch(() => {});
    }

    copy(event) {
        if (this.copied === null) {
            return;
        }
        event.clipboardData.setData("text/plain", this.copied);
        event.preventDefault();
        this.copied = null;
    }

    // What the text field took: typed text; and where keys came as no key (a phone's keyboard
    // says "Unidentified"), the deleting and new lines they made, as those keys.
    input(event) {
        const kind = event.inputType;
        if (event.isComposing || this.composing || kind === "insertCompositionText") {
            return;
        }
        const keys = { deleteContentBackward: "Backspace", deleteContentForward: "Delete", insertLineBreak: "Enter", insertParagraph: "Enter" };
        if (keys[kind]) {
            this.call("taut_key", 1, keys[kind], keys[kind], 0, 1);
        } else if (event.data && kind.startsWith("insert")) {
            this.call("taut_text", event.data);
        }
    }

    composed(text) {
        this.composing = true;
        const shown = this.compose.style;
        this.compose.textContent = text;
        shown.display = text ? "block" : "none";
        shown.left = this.field.style.left;
        shown.top = this.field.style.top;
        shown.font = `${parseFloat(this.field.style.height) * 0.8}px ${SANS}`;
    }

    composeEnd(event) {
        this.composing = false;
        this.compose.style.display = "none";
        this.call("taut_text", event.data || "");
    }

    focused(event, on) {
        if (this.box.contains(event.relatedTarget)) {
            return;
        }
        this.call("taut_focus", on);
        if (on) {
            this.clipboardIn();
        }
    }

    // The system clipboard's text, for a menu's Paste: read only where the person already let
    // the page (never asked for here).
    async clipboardIn() {
        try {
            const allowed = await navigator.permissions.query({ name: "clipboard-read" });
            if (allowed.state !== "granted") {
                return;
            }
            if (this.clipboardParts) {
                const items = await navigator.clipboard.read();
                const picture = items.find((item) => item.types.includes("image/png"));
                const bytes = picture ? new Uint8Array(await (await picture.getType("image/png")).arrayBuffer()) : new Uint8Array();
                this.heard(HEARD.clipboardPicture, 0, 0, 0, bytes);
            }
            this.call("taut_clipboard", await navigator.clipboard.readText());
        } catch {}
    }

    // Where text is typed now (CSS pixels), if anywhere: 1 text, 2 a password (no input
    // method sees it).
    textField(x, y, width, height, on) {
        const field = this.field;
        if (!field) {
            return;
        }
        Object.assign(field.style, { left: `${x}px`, top: `${y}px`, width: `${Math.max(width, 1)}px`, height: `${Math.max(height, 1)}px`, fontSize: `${height * 0.8}px` });
        if (on !== this.fieldOn) {
            field.type = on === 2 ? "password" : "text";
            field.value = "";
        }
        this.fieldOn = on;
        this.placeFocus();
    }

    // ---- what screen readers read ----

    // The program's nodes, as far as they changed: each an element with its role, name, states
    // and place, nested as the nodes are.
    mirror(said) {
        if (!this.readers) {
            this.heldOnly(said);
            return;
        }
        for (const node of said.nodes) {
            this.node(node);
        }
        if (said.order) {
            this.reorder(said.order);
        }
        for (const node of said.nodes) {
            this.place(this.nodes.get(node.k));
        }
        this.focusKey = said.focus;
        this.placeFocus();
        this.placeSelection();
        if (said.say.length > 0) {
            this.live.textContent = "";
            setTimeout(() => (this.live.textContent = said.say.join(". ")), 50);
        }
    }

    // With no page: the nodes kept as told, for a test to read (`told`, `focusKey`).
    heldOnly(said) {
        for (const node of said.nodes) {
            this.told.set(node.k, node);
        }
        const kept = new Set((said.order || []).map(([key]) => key));
        for (const key of this.told.keys()) {
            if (said.order && !kept.has(key)) {
                this.told.delete(key);
            }
        }
        this.focusKey = said.focus;
    }

    element(key) {
        let made = this.nodes.get(key);
        if (!made) {
            made = document.createElement("div");
            made.style.cssText = "position:absolute;overflow:hidden;white-space:pre;";
            made.dataset.key = key;
            made.appendChild(document.createTextNode(""));
            made.said = { a: [] };
            this.nodes.set(key, made);
        }
        return made;
    }

    node(node) {
        const made = this.element(node.k);
        const was = made.said;
        if (node.r) {
            made.setAttribute("role", node.r);
        } else {
            made.removeAttribute("role");
        }
        if (node.n) {
            made.setAttribute("aria-label", node.n);
        } else {
            made.removeAttribute("aria-label");
        }
        for (const [name] of was.a) {
            made.removeAttribute(name);
        }
        for (const [name, value] of node.a) {
            made.setAttribute(name, value);
        }
        made.firstChild.data = node.t;
        if (node.f) {
            made.tabIndex = -1;
        } else {
            made.removeAttribute("tabindex");
        }
        made.said = node;
        if (node.k === this.focusKey && this.fieldOn) {
            this.fieldText(node);
        }
    }

    // The nodes in order, each after its parent: elements put in their parents, those gone
    // taken away. Only elements out of place move (a focused element moved loses focus).
    reorder(order) {
        const kept = new Set(order.map(([key]) => key));
        for (const [key, made] of this.nodes) {
            if (!kept.has(key)) {
                made.remove();
                this.nodes.delete(key);
            }
        }
        const last = new Map();
        for (const [key, parent] of order) {
            const made = this.element(key);
            made.parentKey = parent;
            const into = parent ? this.element(parent) : this.readers;
            // (After an element's own text, or after the child put there last.)
            const next = last.has(into) ? last.get(into).nextSibling : parent ? into.firstChild.nextSibling : into.firstChild;
            if (made !== next) {
                into.insertBefore(made, next);
            }
            last.set(into, made);
        }
        for (const [key] of order) {
            this.place(this.nodes.get(key));
        }
    }

    // An element where its part is drawn: from its parent's corner.
    place(made) {
        const node = made.said;
        const parent = made.parentKey ? this.nodes.get(made.parentKey)?.said : null;
        const [left, top] = parent ? [parent.x ?? 0, parent.y ?? 0] : [0, 0];
        Object.assign(made.style, { left: `${(node.x ?? 0) - left}px`, top: `${(node.y ?? 0) - top}px`, width: `${node.w ?? 0}px`, height: `${node.h ?? 0}px` });
    }

    // A document's selection, as the program tells it: the page's selection, from the first
    // element holding some of it to the last, so a reader reads it. Not while the text field is
    // in use: it holds its own.
    placeSelection() {
        if (this.fieldOn) {
            return;
        }
        const held = [...this.readers.querySelectorAll("[data-key]")].filter((made) => made.said.s);
        const chosen = document.getSelection();
        if (held.length > 0) {
            const [first, last] = [held[0], held[held.length - 1]];
            chosen.setBaseAndExtent(first.firstChild, first.said.s[0], last.firstChild, last.said.s[1]);
        } else if (this.selected) {
            chosen.removeAllRanges();
        }
        this.selected = held.length > 0;
    }

    // The keyboard goes where the interface has it: the text field while text is typed, else
    // the focused node's element, else the box. Only while the page's keyboard is in the box.
    placeFocus() {
        const here = document.activeElement;
        if (!this.box || !this.box.contains(here)) {
            return;
        }
        const node = this.nodes.get(this.focusKey);
        const wanted = this.fieldOn ? this.field : node && node.tabIndex === -1 ? node : this.box;
        if (wanted === this.field && node) {
            this.field.setAttribute("aria-label", node.said.n || "");
            this.fieldText(node.said);
        }
        if (wanted !== here) {
            this.ownFocus = true;
            wanted.focus({ preventScroll: true });
            this.ownFocus = false;
        }
    }

    // The text field holds the focused text box's text, the selection where it is, so a reader
    // reads it; never while an input method composes in it.
    fieldText(node) {
        if (this.composing || this.fieldOn === 2) {
            return;
        }
        if (this.field.value !== node.t) {
            this.field.value = node.t;
        }
    }

    readerAct(event, action) {
        if (this.ownFocus) {
            return;
        }
        const made = event.target.closest?.("[data-key]");
        if (made) {
            this.call("taut_reader", made.dataset.key, action);
        }
    }

    // ---- text ----

    font(css) {
        const [style, family] = css.split("\n");
        const named = family === "sans" ? this.options.sans || SANS : family === "mono" ? this.options.mono || MONO : `"${family}", ${this.options.sans || SANS}`;
        return `${style} ${named}`;
    }

    metrics(css) {
        const context = this.measure;
        if (!context) {
            return [0, 0, 0, 0];
        }
        context.font = this.font(css);
        const line = context.measureText("Hg");
        return [line.fontBoundingBoxAscent, line.fontBoundingBoxDescent, context.measureText("H").actualBoundingBoxAscent, context.measureText("x").actualBoundingBoxAscent];
    }

    // A line's width, and each character's end into `stops` (from the line's start: so kerning
    // and shaping count, as drawn). Past 256 characters, each one's own width added up.
    measureText(css, text, stops, count) {
        const context = this.measure;
        if (!context) {
            return 0;
        }
        context.font = this.font(css);
        const ends = new Float32Array(this.exports.memory.buffer, stops, count);
        let [so_far, at, sum] = ["", 0, 0];
        for (const letter of text) {
            so_far += letter;
            sum += count > 256 ? context.measureText(letter).width : 0;
            ends[at++] = count > 256 ? sum : context.measureText(so_far).width;
        }
        return count > 256 ? sum : context.measureText(text).width;
    }

    // Draws a line in white on clear, keeps its pixels for `take`, and says where its left
    // edge and baseline are, and its size.
    drawText(css, text, place) {
        const context = this.measure;
        if (!context) {
            return 0;
        }
        context.font = this.font(css);
        const sizes = context.measureText(text);
        const left = Math.ceil(Math.max(0, sizes.actualBoundingBoxLeft)) + 1;
        const width = Math.ceil(left + Math.max(sizes.width, sizes.actualBoundingBoxRight)) + 2;
        const ascent = Math.ceil(Math.max(sizes.fontBoundingBoxAscent, sizes.actualBoundingBoxAscent)) + 1;
        const height = ascent + Math.ceil(Math.max(sizes.fontBoundingBoxDescent, sizes.actualBoundingBoxDescent)) + 1;
        const canvas = context.canvas;
        canvas.width = Math.max(canvas.width, width);
        canvas.height = Math.max(canvas.height, height);
        context.clearRect(0, 0, width, height);
        context.font = this.font(css);
        context.fillStyle = "#fff";
        context.textBaseline = "alphabetic";
        context.fillText(text, left, ascent);
        this.made = context.getImageData(0, 0, width, height).data;
        new Float32Array(this.exports.memory.buffer, place, 4).set([left, ascent, width, height]);
        return this.made.length;
    }

    take(into) {
        if (this.made) {
            this.bytes(into, this.made.length).set(this.made);
        }
        this.made = null;
    }

    fontFile(nth, face) {
        const font = (this.fonts || [])[nth];
        if (!font) {
            return 0;
        }
        new Uint32Array(this.exports.memory.buffer, face, 1)[0] = font[0];
        this.made = font[1];
        return font[1].length;
    }

    // ---- storage and files ----

    storageGet(key) {
        try {
            const value = localStorage.getItem(key);
            if (value === null) {
                return -1;
            }
            this.made = encoder.encode(value);
            return this.made.length;
        } catch {
            return -1;
        }
    }

    storageSet(key, value) {
        try {
            localStorage.setItem(key, value);
            return 1;
        } catch {
            return 0;
        }
    }

    // The browser's file picker (`kind`: 1 saving, 2 several files, 4 a folder): what is picked
    // is read and handed over, or nothing. Says whether it answers a save itself (with the save
    // picker, where the browser has one); else the program saves as a download.
    askFile(accept, name, kind) {
        if (kind & 1) {
            return this.askSave(accept, name);
        }
        const picker = document.createElement("input");
        Object.assign(picker, { type: "file", accept, multiple: (kind & 2) !== 0, webkitdirectory: (kind & 4) !== 0 });
        picker.addEventListener("change", () => this.picked([...picker.files], kind));
        picker.addEventListener("cancel", () => this.call("taut_file", "", new Uint8Array()));
        picker.click();
        return 0;
    }

    // Hands over what was picked, each file read whole: one file alone; several one by one; a
    // folder's files, then the folder.
    async picked(files, kind) {
        if (files.length === 0) {
            this.call("taut_file", "", new Uint8Array());
            return;
        }
        if (kind === 0) {
            this.call("taut_file", files[0].name, new Uint8Array(await files[0].arrayBuffer()));
            return;
        }
        const folder = (kind & 4) !== 0;
        for (const [at, file] of files.entries()) {
            const last = !folder && at === files.length - 1;
            const name = (folder && file.webkitRelativePath) || file.name;
            this.heard(last ? HEARD.pickedLast : HEARD.pickedMore, 0, 0, 0, new Uint8Array(await file.arrayBuffer()), name);
        }
        if (folder) {
            this.heard(HEARD.pickedFolder);
        }
    }

    // The save picker, for a file named `name` ending in one of `accept`: the file chosen is
    // kept to write (`saveFile`). Says whether the browser has one.
    askSave(accept, name) {
        if (typeof showSaveFilePicker !== "function") {
            return 0;
        }
        const endings = accept ? accept.split(",") : [];
        const types = endings.length > 0 ? [{ description: endings.join(", "), accept: { "application/octet-stream": endings } }] : undefined;
        showSaveFilePicker({ suggestedName: name, types }).then(
            (handle) => {
                this.saved = handle;
                this.heard(HEARD.saveChosen, 0, 0, 0, handle.name);
            },
            () => this.call("taut_file", "", new Uint8Array()),
        );
        return 1;
    }

    // Writes `bytes` into the file the save picker chose last.
    saveFile(bytes) {
        const handle = this.saved;
        const written = handle.createWritable().then(async (into) => {
            await into.write(bytes);
            await into.close();
        });
        written.catch((error) => console.error(`taut: ${handle.name} was not saved: ${error.message}`));
    }

    download(name, bytes) {
        const link = document.createElement("a");
        link.href = URL.createObjectURL(new Blob([bytes]));
        link.download = name;
        link.click();
        setTimeout(() => URL.revokeObjectURL(link.href), 1000);
    }

    // ---- drag and drop with other programs ----

    // What a drag holds: 1 files, 2 text, 3 both.
    holds(data) {
        const types = [...data.types];
        return (types.includes("Files") ? 1 : 0) | (types.includes("text/plain") || types.includes("text/uri-list") ? 2 : 0);
    }

    // A drag over the page: the program's own, or one from elsewhere, which the program answers
    // with what it takes of it (1 files, 2 text, 0 nothing), kept for the drop.
    dragOver(event) {
        const [x, y] = this.placeOf(event);
        if (this.draggingOut) {
            event.preventDefault();
            event.dataTransfer.dropEffect = "move";
            this.heard(HEARD.dragOver, x, y);
            return;
        }
        this.dropping = this.heard(HEARD.dropOver, x, y, this.holds(event.dataTransfer));
        if (this.dropping) {
            event.preventDefault();
            event.dataTransfer.dropEffect = "copy";
        }
    }

    dragLeft(event) {
        if (this.box.contains(event.relatedTarget)) {
            return;
        }
        this.heard(this.draggingOut ? HEARD.dragLeft : HEARD.dropLeft);
    }

    // Dropped here: the program's own drag drops inside it; files are read whole, then handed
    // over with where they were dropped; text is handed over as it is.
    async dropped(event) {
        event.preventDefault();
        const [x, y] = this.placeOf(event);
        if (this.draggingOut) {
            this.droppedHere = true;
            this.heard(HEARD.dragDrop, x, y);
            return;
        }
        const data = event.dataTransfer;
        if (this.dropping === 2) {
            this.heard(HEARD.droppedText, x, y, 0, data.getData("text/plain") || data.getData("text/uri-list"));
            return;
        }
        const files = this.dropping === 1 ? [...data.files] : [];
        for (const file of files) {
            this.heard(HEARD.droppedFile, 0, 0, 0, new Uint8Array(await file.arrayBuffer()), file.name);
        }
        this.heard(HEARD.droppedFiles, x, y);
    }

    // The browser starts dragging from the canvas: the box there hands out the drag's items, and
    // its place for the picture (`dragFrom`), or there is no drag.
    dragStart(event) {
        const [x, y] = this.pointedAt;
        Object.assign(this, { dragItems: [], dragFrom: null });
        if (!this.heard(HEARD.dragStart, x, y) || this.dragItems.length === 0) {
            event.preventDefault();
            return;
        }
        Object.assign(this, { draggingOut: true, droppedHere: false });
        const data = event.dataTransfer;
        for (const [kind, name, bytes] of this.dragItems) {
            const text = decoder.decode(bytes);
            if (kind === 0) {
                data.setData("text/plain", text);
            } else if (kind === 1) {
                data.setData("text/uri-list", text);
                data.setData("text/plain", text);
            } else {
                this.dragFile(data, name, bytes, kind === 3 ? "image/png" : "application/octet-stream");
            }
        }
        data.effectAllowed = "copyMove";
        this.dragPicture(data, x, y);
    }

    // A file dragged out (`type` its MIME type): as a file, and Chrome saves the first where it is
    // dropped on the desktop.
    dragFile(data, name, bytes, type) {
        const file = new File([bytes], name, { type });
        data.items.add(file);
        if (!data.types.includes("downloadurl")) {
            data.setData("DownloadURL", `${file.type}:${name}:${URL.createObjectURL(file)}`);
        }
    }

    // What follows the pointer: the box dragged, as the canvas shows it.
    dragPicture(data, x, y) {
        if (!this.dragFrom || this.glCanvas) {
            return;
        }
        const [left, top, width, height] = this.dragFrom;
        const scale = this.size[2];
        const picture = Object.assign(document.createElement("canvas"), { width: Math.ceil(width * scale), height: Math.ceil(height * scale) });
        picture.style.cssText = `position:fixed;left:-10000px;top:0;width:${width}px;height:${height}px;`;
        picture.getContext("2d").drawImage(this.canvas, left * scale, top * scale, picture.width, picture.height, 0, 0, picture.width, picture.height);
        document.body.appendChild(picture);
        data.setDragImage(picture, x - left, y - top);
        setTimeout(() => picture.remove());
    }

    // The drag out ended: what the other side did (nothing, where it was dropped back here).
    dragEnd(event) {
        const effect = this.droppedHere ? 0 : ({ copy: 1, move: 2 }[event.dataTransfer.dropEffect] ?? 0);
        Object.assign(this, { draggingOut: false, droppedHere: false });
        this.heard(HEARD.dragEnd, 0, 0, effect);
    }

    // ---- notifications and one running copy ----

    // Shows a notification once the person allows them (the first asks); a click brings the
    // page's tab forward and is heard.
    notify(number, title, text, png) {
        if (typeof Notification !== "function" || Notification.permission === "denied") {
            return;
        }
        const show = () => this.showNotification(number, title, text, png);
        if (Notification.permission === "granted") {
            show();
        } else {
            Notification.requestPermission().then((said) => said === "granted" && show());
        }
    }

    showNotification(number, title, text, png) {
        const icon = png.length > 0 ? URL.createObjectURL(new Blob([png], { type: "image/png" })) : undefined;
        let shown;
        try {
            shown = new Notification(title, { body: text, icon });
        } catch (error) {
            // (A phone's browser shows notifications only through a service worker.)
            console.error(`taut: no notification: ${error.message}`);
            return;
        }
        shown.addEventListener("click", () => {
            focus();
            shown.close();
            this.heard(HEARD.notificationClicked, 0, 0, number);
        });
    }

    // Whether this tab is the first of program `name`: a later one hands its address to the
    // first, which holds the lock from now on and hears later tabs.
    firstCopy(name) {
        const lock = `taut:${name}`;
        if (!navigator.locks || typeof BroadcastChannel !== "function") {
            return 1;
        }
        const channel = new BroadcastChannel(lock);
        if (this.locks.includes(lock)) {
            channel.postMessage(location.href);
            channel.close();
            return 0;
        }
        let held = false;
        navigator.locks.request(lock, () => {
            held = true;
            return new Promise(() => {});
        });
        channel.addEventListener("message", (event) => held && this.heard(HEARD.laterTab, 0, 0, 0, String(event.data)));
        return 1;
    }

    // ---- printing ----

    // The country the browser's language names ("US"; none where it can't tell), kept for
    // `take`: its length.
    region() {
        try {
            return this.kept(new Intl.Locale(navigator.language).maximize().region || "");
        } catch {
            return this.kept("");
        }
    }

    // Prints the pages handed over (`print_page`), each alone on a sheet of its size, through the
    // browser's dialog; job `job` is told when the dialog closes, or that it can't print.
    printPages(title, job) {
        const pages = this.printing;
        this.printing = [];
        if (typeof document === "undefined" || pages.length === 0) {
            setTimeout(() => this.heard(HEARD.printUnable, 0, 0, job));
            return;
        }
        const sheets = document.createElement("div");
        sheets.className = "taut-print";
        const rules = ["@media screen { .taut-print { display: none; } }", "@media print { body > :not(.taut-print) { display: none !important; } .taut-print canvas:not(:last-of-type) { break-after: page; } }"];
        for (const [at, page] of pages.entries()) {
            const [width, height] = page.size;
            rules.push(`@page taut-${job}-${at} { size: ${width}px ${height}px; margin: 0; }`);
            const sheet = Object.assign(document.createElement("canvas"), { width: page.width, height: page.height });
            sheet.style.cssText = `display:block;width:${width}px;height:${height}px;page:taut-${job}-${at};`;
            sheet.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(page.rgba.buffer), page.width, page.height), 0, 0);
            sheets.appendChild(sheet);
        }
        sheets.appendChild(Object.assign(document.createElement("style"), { textContent: rules.join("\n") }));
        document.body.appendChild(sheets);
        const done = () => {
            removeEventListener("afterprint", done);
            sheets.remove();
            this.heard(HEARD.printClosed, 0, 0, job);
        };
        addEventListener("afterprint", done);
        setTimeout(() => {
            const named = document.title;
            document.title = title || named;
            print();
            document.title = named;
        });
    }

    // ---- the person's fonts ----

    // Lists the person's fonts, where the browser can (asking the person the first time): a line
    // a font, its family and style. Asked `again` after a refusal, only from a press or a key.
    askFonts(again) {
        if (typeof queryLocalFonts !== "function" || (again && !navigator.userActivation?.isActive)) {
            return;
        }
        queryLocalFonts().then(
            (fonts) => this.heard(HEARD.fontsListed, 0, 0, 0, fonts.map((font) => `${font.family}\t${font.style}`).join("\n")),
            () => this.heard(HEARD.fontsRefused),
        );
    }

    // ---- WebGL2 (feature `gpu`) ----

    // A WebGL2 canvas over the 2D one, the program drawing into it from now on: its device's
    // name kept for `take`, and that name's length; -1 where there is none.
    gpuOpen() {
        if (!this.box) {
            return -1;
        }
        const canvas = document.createElement("canvas");
        canvas.style.cssText = `${this.canvas.style.cssText}pointer-events:none;`;
        canvas.setAttribute("aria-hidden", "true");
        const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, depth: false, stencil: false, preserveDrawingBuffer: false });
        if (!gl) {
            return -1;
        }
        [canvas.width, canvas.height] = [this.canvas.width, this.canvas.height];
        canvas.addEventListener("webglcontextlost", (event) => {
            event.preventDefault();
            this.heard(HEARD.contextLost);
        });
        this.canvas.after(canvas);
        Object.assign(this, { gl, glCanvas: canvas, glObjects: [null] });
        const named = gl.getExtension("WEBGL_debug_renderer_info");
        this.glName = String(gl.getParameter(named ? named.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
        return this.kept(this.glName);
    }

    gpuClose() {
        this.glCanvas?.remove();
        Object.assign(this, { gl: null, glCanvas: null, glObjects: [null] });
    }

    // A setting the page gave (`renderer`, `draw`), kept for `take`: its length, or -1.
    setting(name) {
        const value = this.options[name];
        return typeof value === "string" ? this.kept(value) : -1;
    }

    // WebGL2's functions as OpenGL ES 3's, for the program: objects by number (0 none).
    glImports() {
        const page = this;
        const gl = () => page.gl;
        const memory = () => page.exports.memory.buffer;
        const ints = (at, count) => new Int32Array(memory(), at, count);
        const of = (number) => page.glObjects[number] || null;
        const number = (made) => {
            if (!made) {
                return 0;
            }
            if (!made.tautNumber) {
                made.tautNumber = page.glObjects.push(made) - 1;
            }
            return made.tautNumber;
        };
        const make = (count, at, made) => ints(at, count).set(Array.from({ length: count }, () => number(made())));
        const text = (at) => {
            const bytes = new Uint8Array(memory(), at);
            return decoder.decode(bytes.subarray(0, bytes.indexOf(0)));
        };
        const answer = (value) => (typeof value === "object" && !(value instanceof Int32Array) ? number(value) : value);
        return {
            glGetError: () => gl().getError(),
            glGetIntegerv: (name, at) => {
                const value = answer(gl().getParameter(name));
                value instanceof Int32Array ? ints(at, value.length).set(value) : (ints(at, 1)[0] = Number(value));
            },
            glEnable: (what) => gl().enable(what),
            glDisable: (what) => gl().disable(what),
            glBlendFuncSeparate: (a, b, c, d) => gl().blendFuncSeparate(a, b, c, d),
            glViewport: (x, y, width, height) => gl().viewport(x, y, width, height),
            glScissor: (x, y, width, height) => gl().scissor(x, y, width, height),
            glActiveTexture: (unit) => gl().activeTexture(unit),
            glGenTextures: (count, at) => make(count, at, () => gl().createTexture()),
            glBindTexture: (target, texture) => gl().bindTexture(target, of(texture)),
            glTexParameteri: (target, name, value) => gl().texParameteri(target, name, value),
            glTexImage2D: (target, level, inner, width, height, border, format, kind, at) =>
                gl().texImage2D(target, level, inner, width, height, border, format, kind, at ? new Uint8Array(memory(), at) : null),
            glTexSubImage2D: (target, level, x, y, width, height, format, kind, at) => gl().texSubImage2D(target, level, x, y, width, height, format, kind, new Uint8Array(memory()), at),
            glPixelStorei: (name, value) => gl().pixelStorei(name, value),
            glGenFramebuffers: (count, at) => make(count, at, () => gl().createFramebuffer()),
            glBindFramebuffer: (target, frame) => gl().bindFramebuffer(target, of(frame)),
            glFramebufferTexture2D: (target, attachment, textureTarget, texture, level) => gl().framebufferTexture2D(target, attachment, textureTarget, of(texture), level),
            glCheckFramebufferStatus: (target) => gl().checkFramebufferStatus(target),
            glGenBuffers: (count, at) => make(count, at, () => gl().createBuffer()),
            glBindBuffer: (target, buffer) => gl().bindBuffer(target, of(buffer)),
            glBufferData: (target, size, at, usage) => (at ? gl().bufferData(target, new Uint8Array(memory(), at, size), usage) : gl().bufferData(target, size, usage)),
            glBufferSubData: (target, offset, size, at) => gl().bufferSubData(target, offset, new Uint8Array(memory()), at, size),
            glGenVertexArrays: (count, at) => make(count, at, () => gl().createVertexArray()),
            glBindVertexArray: (array) => gl().bindVertexArray(of(array)),
            glEnableVertexAttribArray: (index) => gl().enableVertexAttribArray(index),
            glVertexAttribPointer: (index, size, kind, normalized, stride, offset) => gl().vertexAttribPointer(index, size, kind, normalized !== 0, stride, offset),
            glVertexAttribDivisor: (index, divisor) => gl().vertexAttribDivisor(index, divisor),
            glDrawArrays: (mode, first, count) => gl().drawArrays(mode, first, count),
            glDrawArraysInstanced: (mode, first, count, instances) => gl().drawArraysInstanced(mode, first, count, instances),
            glCreateShader: (kind) => number(gl().createShader(kind)),
            glShaderSource: (shader, count, sources, lengths) => {
                const starts = new Uint32Array(memory(), sources, count);
                const sizes = lengths ? new Int32Array(memory(), lengths, count) : null;
                const parts = Array.from(starts, (at, n) => (sizes ? decoder.decode(new Uint8Array(memory(), at, sizes[n])) : text(at)));
                gl().shaderSource(of(shader), parts.join(""));
            },
            glCompileShader: (shader) => gl().compileShader(of(shader)),
            glGetShaderiv: (shader, name, at) => (ints(at, 1)[0] = Number(gl().getShaderParameter(of(shader), name))),
            glDeleteShader: (shader) => gl().deleteShader(of(shader)),
            glCreateProgram: () => number(gl().createProgram()),
            glAttachShader: (program, shader) => gl().attachShader(of(program), of(shader)),
            glLinkProgram: (program) => gl().linkProgram(of(program)),
            glGetProgramiv: (program, name, at) => (ints(at, 1)[0] = Number(gl().getProgramParameter(of(program), name))),
            glDeleteProgram: (program) => gl().deleteProgram(of(program)),
            glUseProgram: (program) => gl().useProgram(of(program)),
            glGetUniformLocation: (program, at) => {
                const place = gl().getUniformLocation(of(program), text(at));
                return place ? number(place) : -1;
            },
            glUniform1i: (place, value) => place >= 0 && gl().uniform1i(of(place), value),
        };
    }

    // ---- videos (feature `video`) ----

    // A video element for an address, or for bytes (a Blob of type `kind`): its number; 0 with
    // no page (under Node). Muted unless `sound`. Each frame the browser shows is noted
    // (`requestVideoFrameCallback`, where it has it) to be read once.
    videoOpen(source, kind, sound) {
        if (typeof document === "undefined") {
            return 0;
        }
        const element = document.createElement("video");
        const blob = typeof source === "string" ? null : URL.createObjectURL(new Blob([source], { type: kind }));
        const id = ++this.videoCount;
        const made = { element, blob, canvas: null, context: null, fresh: false, given: -1, shown: -1, wanted: false, ended: false, error: "", clearsOnPlay: false };
        Object.assign(element, { muted: !sound, defaultMuted: !sound, playsInline: true, preload: "auto", crossOrigin: "anonymous" });
        element.addEventListener("loadeddata", () => (made.fresh = true));
        element.addEventListener("seeked", () => (made.fresh = true));
        element.addEventListener("ended", () => Object.assign(made, { ended: true, wanted: false }));
        element.addEventListener("error", () => this.videoFailed(made, `the player stopped: ${element.error?.message || `error ${element.error?.code}`}`));
        if (element.requestVideoFrameCallback) {
            const shown = (_, frame) => {
                made.shown = frame.mediaTime;
                this.videos.has(id) && element.requestVideoFrameCallback(shown);
            };
            element.requestVideoFrameCallback(shown);
        }
        element.src = blob || source;
        this.videos.set(id, made);
        return id;
    }

    videoFailed(made, why, clearsOnPlay = false) {
        made.wanted = false;
        made.error = why;
        made.clearsOnPlay = clearsOnPlay;
    }

    videoClose(id) {
        const made = this.videos.get(id);
        if (!made) {
            return;
        }
        this.videos.delete(id);
        made.element.pause();
        made.element.removeAttribute("src");
        made.element.load();
        if (made.blob) {
            URL.revokeObjectURL(made.blob);
        }
    }

    // Plays; a browser that wants a press first refuses, told as an error that clears on the
    // next play.
    videoPlay(id) {
        const made = this.videos.get(id);
        if (!made) {
            return;
        }
        made.wanted = true;
        made.ended = false;
        made.element.play()?.catch((error) => {
            if (error.name === "NotAllowedError") {
                this.videoFailed(made, "the browser plays sound only after a press on the page", true);
            } else if (error.name !== "AbortError") {
                this.videoFailed(made, `the player stopped: ${error.message}`);
            }
        });
    }

    videoPause(id) {
        const made = this.videos.get(id);
        if (made) {
            made.wanted = false;
            made.element.pause();
        }
    }

    // Goes to a time; its frame is read once the browser has it (`seeked`).
    videoSeek(id, seconds) {
        const made = this.videos.get(id);
        if (made) {
            made.fresh = false;
            made.element.currentTime = seconds;
        }
    }

    videoSound(id, volume, muted) {
        const made = this.videos.get(id);
        if (made) {
            Object.assign(made.element, { volume: Math.min(Math.max(volume, 0), 1), muted: muted !== 0 });
        }
    }

    // Plays at this speed from now on, the pitch kept.
    videoSpeed(id, speed) {
        const made = this.videos.get(id);
        if (made) {
            Object.assign(made.element, { defaultPlaybackRate: speed, playbackRate: speed, preservesPitch: true });
        }
    }

    // The element's sound tracks (where the browser lists them) and caption tracks, a line each,
    // kept for `take`: "sound" or "captions", its language, its label, whether it plays, by tabs.
    // Answers their length; 0 before the element has read the source's metadata.
    videoTracks(id) {
        const element = this.videos.get(id)?.element;
        if (!element || element.readyState < 1) {
            return 0;
        }
        const line = (kind, track, on) => [kind, track.language, track.label, on ? 1 : 0].join("\t");
        const sound = [...(element.audioTracks || [])].map((track) => line("sound", track, track.enabled));
        const captions = captionTracks(element).map((track) => line("captions", track, false));
        this.made = encoder.encode(["tracks", ...sound, ...captions].join("\n"));
        return this.made.length;
    }

    videoChooseSound(id, nth) {
        const tracks = this.videos.get(id)?.element.audioTracks || [];
        [...tracks].forEach((track, at) => (track.enabled = at === nth));
    }

    // Shows the `nth` caption track's cues to `videoCaption` (hidden: the page draws them), or
    // none for -1.
    videoChooseCaptions(id, nth) {
        const element = this.videos.get(id)?.element;
        if (element) {
            captionTracks(element).forEach((track, at) => (track.mode = at === nth ? "hidden" : "disabled"));
        }
    }

    // The chosen caption track's cues on show, a line each, kept for `take`: their length.
    videoCaption(id) {
        const element = this.videos.get(id)?.element;
        const track = element && captionTracks(element).find((track) => track.mode === "hidden");
        const shown = [...(track?.activeCues || [])].map((cue) => cue.text);
        this.made = encoder.encode(shown.join("\n"));
        return this.made.length;
    }

    // Writes the time, the length (NaN unknown), the end (once) and an error's kind (1, or 2 for
    // one that clears on the next play); answers the error's length, its words kept for `take`.
    // (A browser that paused a video it was unmuting before any press says nothing: that is
    // told as a refused play.)
    videoState(id, out) {
        const made = this.videos.get(id);
        if (!made) {
            return 0;
        }
        const element = made.element;
        if (made.wanted && element.paused && !element.ended && !made.error) {
            this.videoFailed(made, "the browser plays sound only after a press on the page", true);
        }
        const length = Number.isFinite(element.duration) ? element.duration : NaN;
        const error = made.error ? (made.clearsOnPlay ? 2 : 1) : 0;
        new Float64Array(this.exports.memory.buffer, out, 4).set([element.currentTime, length, made.ended ? 1 : 0, error]);
        made.ended = false;
        if (!error) {
            return 0;
        }
        this.made = encoder.encode(made.error);
        made.error = "";
        return this.made.length;
    }

    // The frame on show, where it was not read yet: drawn into a canvas of its size, its
    // pixels kept for `take`; writes its width, height and time. 0 where there is none.
    videoFrame(id, out) {
        const made = this.videos.get(id);
        const element = made?.element;
        if (!element || element.seeking || element.readyState < 2 || !element.videoWidth) {
            return 0;
        }
        const time = made.shown >= 0 && !element.paused ? made.shown : element.currentTime;
        if (!made.fresh && time === made.given) {
            return 0;
        }
        const [width, height] = [element.videoWidth, element.videoHeight];
        if (made.canvas?.width !== width || made.canvas?.height !== height) {
            made.canvas = Object.assign(document.createElement("canvas"), { width, height });
            made.context = made.canvas.getContext("2d", { willReadFrequently: true });
        }
        made.context.drawImage(element, 0, 0, width, height);
        try {
            this.made = made.context.getImageData(0, 0, width, height).data;
        } catch {
            this.videoFailed(made, "the page may not read this video's frames: its server allows no other pages (CORS)");
            return 0;
        }
        made.fresh = false;
        made.given = time;
        new Float64Array(this.exports.memory.buffer, out, 3).set([width, height, time]);
        return this.made.length;
    }
}

// A video element's subtitle and caption tracks.
function captionTracks(element) {
    return [...element.textTracks].filter((track) => track.kind === "subtitles" || track.kind === "captions");
}

// The canvas text is measured and drawn on: one of its own, read back often.
function textCanvas() {
    if (typeof OffscreenCanvas === "function") {
        return new OffscreenCanvas(64, 32).getContext("2d", { willReadFrequently: true });
    }
    if (typeof document === "undefined") {
        return null;
    }
    const canvas = document.createElement("canvas");
    return canvas.getContext("2d", { willReadFrequently: true });
}

// The person's accent colour as 0xRRGGBB, where the browser knows it (CSS `AccentColor`);
// else -1.
function accent() {
    if (typeof CSS === "undefined" || !CSS.supports("color", "AccentColor")) {
        return -1;
    }
    const probe = document.createElement("div");
    probe.style.color = "AccentColor";
    document.body.appendChild(probe);
    const [red, green, blue] = getComputedStyle(probe).color.match(/\d+/g).map(Number);
    probe.remove();
    return (red << 16) | (green << 8) | blue;
}
