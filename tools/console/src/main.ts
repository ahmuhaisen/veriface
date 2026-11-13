// Veriface dev console: Quality, Document portrait, Verify.
import "./style.css";
import { api, type ProfileRef } from "./api";
import { clear, h } from "./dom";
import { renderDocument, renderError, renderQuality, renderVerification } from "./render";
import { createSlot, sdkAvailable, type Samples } from "./slot";

const app = document.getElementById("app")!;

interface Controls {
  profile: HTMLSelectElement;
  operatingPoint: HTMLSelectElement;
  referenceKind: HTMLSelectElement;
  output: HTMLSelectElement;
}

function params(c: Controls, kind: "quality" | "document" | "verify"): URLSearchParams {
  const q = new URLSearchParams();
  if (c.profile.value) q.set("profile", c.profile.value);
  if (kind === "verify") {
    if (c.operatingPoint.value) q.set("operatingPoint", c.operatingPoint.value);
    q.set("referenceKind", c.referenceKind.value);
  } else if (c.output.value) {
    q.set("output", c.output.value);
  }
  return q;
}

async function main(): Promise<void> {
  const status = h("span.status-line.muted", null, "checking service…");
  const sdkNote = h("span.muted");
  app.append(
    h(
      "header",
      null,
      h("h1", null, "Veriface ", h("span.muted", null, "dev console")),
      h("div.row", null, status, sdkNote),
      h("p.note", null, "Development tool. Use only public-domain test images or your own photos; never other people's IDs or selfies."),
    ),
  );

  const samples: Samples = await (await fetch("/samples/index.json")).json();
  const sdk = await sdkAvailable();
  sdkNote.textContent = sdk ? " · web SDK guided capture available" : " · web SDK not built (plain webcam only)";

  const ready = await api.ready();
  let profiles: ProfileRef[] = [];
  try {
    const info = await api.info();
    profiles = (await api.profiles()).data.profiles;
    status.textContent = `service ${ready ? "ready" : "NOT READY"} · engine ${info.data.version} · catalogue v${info.data.catalogueVersion} · ${info.data.models.length} models`;
    status.className = `status-line ${ready && !info.data.version.endsWith("+stub") ? "ok-text" : "warn-text"}`;
  } catch (e) {
    status.textContent = "service unreachable or key rejected (see below)";
    status.className = "status-line bad-text";
    app.append(renderError(e));
  }

  const controls: Controls = {
    profile: h("select", null, h("option", { value: "" }, "(key default)"), ...profiles.map((p) => h("option", { value: p.name }, `${p.name} ${p.version}`))),
    operatingPoint: h("select", null, h("option", { value: "" }, "(profile default)")),
    referenceKind: h(
      "select",
      null,
      ...["auto", "document", "portrait"].map((k) => h("option", { value: k }, k)),
    ),
    output: h(
      "select",
      null,
      h("option", { value: "" }, "(profile default)"),
      h("option", { value: "true" }, "include"),
      h("option", { value: "false" }, "omit"),
    ),
  };
  const profileDesc = h("div.muted");

  async function loadOperatingPoints(): Promise<void> {
    clear(controls.operatingPoint);
    controls.operatingPoint.append(h("option", { value: "" }, "(profile default)"));
    const name = controls.profile.value || profiles.find((p) => p.name === "standard")?.name || profiles[0]?.name;
    if (!name) return;
    try {
      const { data } = await api.profile(name);
      const v = data.profile.verification;
      profileDesc.textContent = `${name}: ${data.profile.description ?? ""} · calibration ${v?.calibration?.status ?? "?"}`;
      for (const op of v?.operatingPoints ?? []) {
        controls.operatingPoint.append(
          h("option", { value: op.id }, `${op.id} (target FMR ${op.targetFmr}, t=${op.threshold}${op.id === v.defaultOperatingPoint ? ", default" : ""})`),
        );
      }
    } catch (e) {
      profileDesc.textContent = `could not load profile ${name}: ${(e as Error).message}`;
    }
  }
  controls.profile.addEventListener("change", loadOperatingPoints);
  await loadOperatingPoints();

  app.append(
    h(
      "section.controls",
      null,
      h("label", null, "Profile ", controls.profile),
      h("label", null, "Operating point (Verify) ", controls.operatingPoint),
      h("label", null, "Reference kind (Verify) ", controls.referenceKind),
      h("label", null, "Output image ", controls.output),
      profileDesc,
    ),
  );

  // Tabs
  const panes: Record<string, HTMLElement> = {};
  const tabBar = h("nav.tabs");
  const body = h("main");
  const tabs: [string, string][] = [
    ["quality", "Quality"],
    ["document", "Document portrait"],
    ["verify", "Verify"],
  ];
  for (const [id, label] of tabs) {
    const btn = h(
      "button.tab",
      {
        onclick: () => {
          for (const [k, p] of Object.entries(panes)) p.hidden = k !== id;
          for (const b of tabBar.querySelectorAll("button")) b.classList.toggle("active", b === btn);
          try {
            localStorage.setItem("dfc-tab", id);
          } catch {
            /* ignore */
          }
        },
      },
      label,
    );
    tabBar.append(btn);
  }

  // Quality
  {
    const slot = createSlot("Selfie", "selfie", samples, sdk);
    const out = h("div");
    const run = h("button.primary", { disabled: true }, "Assess quality");
    slot.onChange(() => (run.disabled = false));
    run.addEventListener("click", async () => {
      run.disabled = true;
      clear(out);
      out.append(h("p.muted", null, "Running…"));
      try {
        const r = await api.quality(slot.blob()!, params(controls, "quality"));
        clear(out);
        out.append(renderQuality(r.data, slot.imageUrl(), r.ms));
      } catch (e) {
        clear(out);
        out.append(renderError(e));
      } finally {
        run.disabled = false;
      }
    });
    panes.quality = h("div.pane", null, h("p.muted", null, "POST /v1/quality: assess a selfie against the profile."), slot.el, run, out);
  }

  // Document portrait
  {
    const slot = createSlot("Document", "document", samples, sdk);
    const out = h("div");
    const run = h("button.primary", { disabled: true }, "Extract portrait");
    slot.onChange(() => (run.disabled = false));
    run.addEventListener("click", async () => {
      run.disabled = true;
      clear(out);
      out.append(h("p.muted", null, "Running…"));
      try {
        const r = await api.document(slot.blob()!, params(controls, "document"));
        clear(out);
        out.append(renderDocument(r.data, slot.imageUrl(), r.ms));
      } catch (e) {
        clear(out);
        out.append(renderError(e));
      } finally {
        run.disabled = false;
      }
    });
    panes.document = h(
      "div.pane",
      null,
      h("p.muted", null, "POST /v1/document-portraits: find the document, the portrait on it, and assess it as a reference."),
      slot.el,
      run,
      out,
    );
  }

  // Verify
  {
    const probe = createSlot("Probe (selfie)", "selfie", samples, sdk);
    const ref = createSlot("Reference (document or portrait)", "document", samples, sdk);
    const out = h("div");
    const run = h("button.primary", { disabled: true }, "Verify");
    const update = () => (run.disabled = !(probe.blob() && ref.blob()));
    probe.onChange(update);
    ref.onChange(update);
    run.addEventListener("click", async () => {
      run.disabled = true;
      clear(out);
      out.append(h("p.muted", null, "Running…"));
      try {
        const r = await api.verify(probe.blob()!, ref.blob()!, params(controls, "verify"));
        clear(out);
        out.append(renderVerification(r.data, probe.imageUrl(), ref.imageUrl(), r.ms));
      } catch (e) {
        clear(out);
        out.append(renderError(e));
      } finally {
        update();
      }
    });
    panes.verify = h(
      "div.pane",
      null,
      h("p.muted", null, "POST /v1/verifications: compare a selfie with a document or portrait."),
      h("div.pair", null, probe.el, ref.el),
      run,
      out,
    );
  }

  body.append(...Object.values(panes));
  app.append(tabBar, body);
  let start = "quality";
  try {
    start = localStorage.getItem("dfc-tab") ?? "quality";
  } catch {
    /* ignore */
  }
  const idx = Math.max(0, tabs.findIndex(([id]) => id === start));
  (tabBar.querySelectorAll("button")[idx] as HTMLButtonElement).click();
}

main().catch((e) => app.append(renderError(e)));
