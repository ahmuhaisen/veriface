// An image input: file upload, a testdata sample, a plain webcam snapshot, or (when sdk-web/dist exists)
// guided capture with <veriface-capture>.
import { clear, h } from "./dom";

export interface Samples {
  images: string[];
  documents: string[];
  notes: Record<string, string>;
}

export interface Slot {
  el: HTMLElement;
  blob(): Blob | null;
  imageUrl(): string | null;
  onChange(fn: () => void): void;
}

let sdkLoaded: Promise<boolean> | null = null;

/** Load the SDK's custom element once, if the dev server says sdk-web/dist exists. */
export function sdkAvailable(): Promise<boolean> {
  sdkLoaded ??= (async () => {
    try {
      const s = (await (await fetch("/sdk-status")).json()) as { available: boolean };
      if (!s.available) return false;
      const elementModule = "/sdk/element.js"; // served by the dev server from sdk-web/dist
      await import(/* @vite-ignore */ elementModule);
      return customElements.get("veriface-capture") !== undefined;
    } catch (e) {
      console.warn("[console] web SDK not usable, falling back to plain webcam", e);
      return false;
    }
  })();
  return sdkLoaded;
}

export function createSlot(label: string, mode: "selfie" | "document", samples: Samples, sdk: boolean): Slot {
  let current: Blob | null = null;
  let url: string | null = null;
  const listeners: (() => void)[] = [];

  const preview = h("div.preview", null, h("span.muted", null, "No image yet"));
  const caption = h("div.caption.muted");
  const stage = h("div.stage");

  function set(blob: Blob, name: string): void {
    current = blob;
    if (url) URL.revokeObjectURL(url);
    url = URL.createObjectURL(blob);
    clear(preview);
    preview.append(h("img", { src: url, alt: label }));
    caption.textContent = `${name} · ${(blob.size / 1024).toFixed(0)} KB`;
    for (const fn of listeners) fn();
  }

  const file = h("input", {
    type: "file",
    accept: "image/jpeg,image/png,image/webp",
    onchange: () => {
      const f = file.files?.[0];
      if (f) set(f, f.name);
    },
  });

  const list = mode === "document" ? [...samples.documents, ...samples.images] : [...samples.images, ...samples.documents];
  const picker = h(
    "select",
    {
      onchange: async () => {
        const v = picker.value;
        if (!v) return;
        const dir = samples.documents.includes(v) ? "documents" : "images";
        const blob = await (await fetch(`/samples/${dir}/${v}`)).blob();
        set(blob, `testdata/${dir}/${v}`);
      },
    },
    h("option", { value: "" }, "— testdata sample —"),
    ...list.map((f) => {
      const note = samples.notes[f.replace(/\.[^.]+$/, "")];
      return h("option", { value: f }, note ? `${f} (${note})` : f);
    }),
  );

  async function webcam(): Promise<void> {
    clear(stage);
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: mode === "selfie" ? "user" : "environment", width: { ideal: 1280 }, height: { ideal: 720 } },
      });
    } catch (e) {
      stage.append(h("div.error", null, `Camera not available: ${(e as Error).message}`));
      return;
    }
    const video = h("video", { autoplay: true, playsInline: true, muted: true });
    video.srcObject = stream;
    const stop = () => {
      stream.getTracks().forEach((t) => t.stop());
      clear(stage);
    };
    const snap = h(
      "button",
      {
        onclick: () => {
          const c = document.createElement("canvas");
          c.width = video.videoWidth;
          c.height = video.videoHeight;
          c.getContext("2d")!.drawImage(video, 0, 0);
          c.toBlob((b) => {
            if (b) set(b, `webcam ${c.width}×${c.height}`);
            stop();
          }, "image/jpeg", 0.92);
        },
      },
      "Take snapshot",
    );
    stage.append(video, h("div.row", null, snap, h("button.secondary", { onclick: stop }, "Cancel")));
  }

  async function guided(): Promise<void> {
    clear(stage);
    const el = document.createElement("veriface-capture") as HTMLElement & {
      start(): Promise<void>;
      stop(): void;
    };
    el.setAttribute("mode", mode);
    el.setAttribute("assets-base-url", "/face-capture/");
    el.addEventListener("capture", (ev) => {
      const detail = (ev as CustomEvent<{ blob: Blob }>).detail;
      set(detail.blob, `guided ${mode} capture`);
      el.stop();
      clear(stage);
    });
    el.addEventListener("error", (ev) => {
      stage.append(h("div.error", null, `Capture SDK error: ${JSON.stringify((ev as unknown as CustomEvent).detail)}`));
    });
    stage.append(
      el,
      h("button.secondary", { onclick: () => (el.stop(), clear(stage)) }, "Cancel"),
    );
    await el.start();
  }

  const el = h(
    "section.slot",
    null,
    h("h3", null, label),
    h(
      "div.row",
      null,
      file,
      picker,
      h("button.secondary", { onclick: webcam }, "Webcam snapshot"),
      sdk ? h("button", { onclick: guided }, mode === "selfie" ? "Guided selfie (SDK)" : "Guided document (SDK)") : null,
    ),
    stage,
    preview,
    caption,
  );

  return {
    el,
    blob: () => current,
    imageUrl: () => url,
    onChange: (fn) => listeners.push(fn),
  };
}
